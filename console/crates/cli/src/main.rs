use mechrevo_console_core::{
    fan_curve::{MinimumDutyPolicy, ValidatedFanCurve, OWNED_TABLE_BYTES},
    registry, ControlError, OemControl, OemControlKind, OemDomain, OemValue, PowerProfile, Service,
    Snapshot, ThermalSample,
};
use serde_json::{json, Value};
mod windows_fn_cli;
#[cfg(any(target_os = "linux", test))]
use std::io::Read;
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
    str::FromStr,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const HELP:&str="mechrevo-console 0.1.0 — Windows/Linux OS controls and evidence\n\nUsage:\n  mechrevo-console status [--json]\n  mechrevo-console thermal [--samples COUNT] [--interval-ms MS]\n  mechrevo-console capabilities\n  mechrevo-console models [query]\n  mechrevo-console profile <balanced|performance|power-saver> [--dry-run] [--expected-current RAW]\n  mechrevo-console brightness [percent] [--device ID] [--expected-current RAW] [--dry-run]\n  mechrevo-console oem list\n  mechrevo-console oem set ID VALUE [--dry-run]\n  mechrevo-console fan validate --input PATH [--cpu-min-percent N] [--gpu-min-percent N]\n      [--raise-stopped-stages]\n  mechrevo-console fan hold --input PATH [--cpu-min-percent N] [--gpu-min-percent N]\n      [--raise-stopped-stages]\n  mechrevo-console export --output PATH\n  mechrevo-console --help\n\nPower profiles use the active Linux system power-profile service, otherwise\nACPI platform_profile, or installed Windows OS power schemes. A Linux system\nservice can apply its configured device actions and release application holds;\nthe preview reports those holds. OS profiles are separate from OEM presets.\nProfile --dry-run never sets a profile; --expected-current guards the exact raw\nprofile name/GUID before applying. This is not an atomic OS compare-and-swap.\nBrightness requires an enumerated OS device; multiple devices require --device.\nBrightness --dry-run never writes or invokes a brightness setter. Applied output\nseparates source, selected write_backend, requested_control and actual_value.\n--expected-current RAW requires a target: RAW is the raw OS control value, not\na percent. This optional stale-request guard is not atomic kernel CAS.\nOEM controls use only enumerated, qualified driver interfaces. Use oem list\nfor exact IDs, choices, brightness levels and readback limits. Boolean values\nare on/off; colors use '#RRGGBB'. oem set --dry-run never writes or elevates.\nAn OEM write may request authentication through the installed limited helper.\nFan validation checks a curve file; it does not apply or qualify it on hardware.\nMinimum fan output sets optional whole-percent floors (1..100) on that curve.\nNo floor is assumed. Zero-duty stages keep zero unless --raise-stopped-stages\nis given; only channels with a floor change. Temperature boundaries stay as supplied.\nThese are output duty limits, not RPM targets or noise/cooling guarantees.\nFan hold is a foreground Linux session requiring a terminal and a qualified\ndriver. It retains exclusive control until Enter/exit, with explicit restore\non Enter. It does not save a startup setting or elevate automatically.\nExport writes UTF-8 JSON, requires an explicit path and refuses overwrites.\nThermal emits bounded JSONL observations from Linux hwmon only (default 1 sample;\n1..3600 samples, 250..60000 ms interval, default 1000 ms). It does not authenticate\nor acquire fan control. Collection or output failure stops the stream.\nMissing telemetry is unknown, never a simulated zero. No automatic sudo.\n";
const CPU_POWER_HELP: &str = "\nCPU PL1 policy (independent of OS power profiles):\n  mechrevo-console cpu-power probe\n  mechrevo-console cpu-power set WATTS [--dry-run]\n      [--target-celsius native|90] [--polling-seconds native|1]\n      [--rapl-debounce-seconds native|0]\n  mechrevo-console cpu-power <clear|recover|reapply> [--dry-run]\n\nCPU commands require explicit authorization for their read-only probe. A CPU\n--dry-run probes, then previews locally without executing a policy change.\nSet preserves every omitted option from the observed complete policy; on a\nnative unmanaged host, absent options remain explicit null. Native and 90/1/0\nare distinct settings. Allowed watts come from the qualified controller.\nClear/recover return to native thermald policy, not a fixed watt value.\nExecuted CPU policies persist after this command exits. JSON output is default.\n";
#[derive(Debug, PartialEq)]
enum Action {
    Help,
    Version,
    Status(bool),
    Thermal { samples: u16, interval_ms: u32 },
    Capabilities,
    Models(String),
    Profile(PowerProfile, bool, Option<String>),
    Brightness(Option<u8>, Option<String>, bool, Option<u32>),
    OemList,
    OemSet(String, String, bool),
    WindowsFn(windows_fn_cli::Action),
    FanValidate(PathBuf, MinimumDutyPolicy),
    FanHold(PathBuf, MinimumDutyPolicy),
    CpuPower(CliCpuPowerAction),
    Export(PathBuf),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliCpuPolicySetting {
    Native,
    Qualified,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CliCpuPolicyOptions {
    target_celsius: Option<CliCpuPolicySetting>,
    polling_seconds: Option<CliCpuPolicySetting>,
    rapl_debounce_seconds: Option<CliCpuPolicySetting>,
}
#[derive(Debug, PartialEq, Eq)]
enum CliCpuPowerAction {
    Probe,
    Set {
        watts: u64,
        options: CliCpuPolicyOptions,
        dry_run: bool,
    },
    Clear {
        dry_run: bool,
    },
    Recover {
        dry_run: bool,
    },
    Reapply {
        dry_run: bool,
    },
}
fn text(arg: &OsString) -> Result<&str, ControlError> {
    arg.to_str()
        .ok_or_else(|| ControlError::new("invalid_argument", "This argument must be valid Unicode"))
}
fn usage(message: impl Into<String>) -> ControlError {
    ControlError::new("invalid_arguments", message)
}
fn parse(args: Vec<OsString>) -> Result<Action, ControlError> {
    let Some(command) = args.first() else {
        return Ok(Action::Help);
    };
    let command = text(command)?;
    let rest = &args[1..];
    match command {
        "--help" | "-h" if rest.is_empty() => Ok(Action::Help),
        "--version" | "-V" if rest.is_empty() => Ok(Action::Version),
        "status" if rest.is_empty() => Ok(Action::Status(false)),
        "status" if rest.len() == 1 && rest[0] == "--json" => Ok(Action::Status(true)),
        "thermal" => {
            let (mut samples, mut interval_ms) = (None, None);
            let mut values = rest.chunks_exact(2);
            for pair in &mut values {
                let value = text(&pair[1])?;
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(usage("Thermal counts and intervals must be unsigned decimal integers"));
                }
                match text(&pair[0])? {
                    "--samples" if samples.is_none() => {
                        let count = value.parse::<u16>().ok().filter(|n| (1..=3600).contains(n))
                            .ok_or_else(|| usage("--samples must be 1 through 3600"))?;
                        samples = Some(count);
                    }
                    "--interval-ms" if interval_ms.is_none() => {
                        let interval = value.parse::<u32>().ok().filter(|n| (250..=60_000).contains(n))
                            .ok_or_else(|| usage("--interval-ms must be 250 through 60000"))?;
                        interval_ms = Some(interval);
                    }
                    _ => return Err(usage("Unknown or repeated thermal option")),
                }
            }
            if !values.remainder().is_empty() {
                return Err(usage("Each thermal option requires a value"));
            }
            Ok(Action::Thermal { samples: samples.unwrap_or(1), interval_ms: interval_ms.unwrap_or(1000) })
        }
        "capabilities" if rest.is_empty() => Ok(Action::Capabilities),
        "cpu-power" => parse_cpu_power(rest).map(Action::CpuPower),
        "models" if rest.len() <= 1 => Ok(Action::Models(
            rest.first().map(text).transpose()?.unwrap_or("").into(),
        )),
        "profile" => {
            if rest.is_empty() {
                return Err(usage("profile requires one profile and optional --dry-run / --expected-current RAW"));
            }
            let profile = PowerProfile::from_str(text(&rest[0])?)?;
            let mut dry = false;
            let mut expected = None;
            let mut index = 1;
            while index < rest.len() {
                match text(&rest[index])? {
                    "--dry-run" if !dry => dry = true,
                    "--expected-current" if expected.is_none() => {
                        index += 1;
                        let value = text(rest.get(index).ok_or_else(|| usage("--expected-current needs a raw profile value"))?)?;
                        if value.is_empty() || value.len() > 64 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
                            return Err(usage("Expected profile must be an exact bounded raw profile name or GUID"));
                        }
                        expected = Some(value.to_owned());
                    }
                    _ => return Err(usage("Unknown or repeated profile option")),
                }
                index += 1;
            }
            Ok(Action::Profile(profile, dry, expected))
        }
        "brightness" => {
            let mut percent = None;
            let mut device = None;
            let mut dry = false;
            let mut expected_current = None;
            let mut i = 0;
            while i < rest.len() {
                let token = text(&rest[i])?;
                match token {
                    "--dry-run" if !dry => dry = true,
                    "--device" if device.is_none() => {
                        i += 1;
                        let arg = rest
                            .get(i)
                            .ok_or_else(|| usage("--device needs a device ID"))?;
                        let id = text(arg)?;
                        if id.is_empty() || id.starts_with("--") {
                            return Err(usage("--device needs an explicit device ID, not an option"));
                        }
                        device = Some(id.into());
                    }
                    "--expected-current" if expected_current.is_none() => {
                        i += 1;
                        let raw = rest.get(i).map(text).transpose()?.ok_or_else(|| {
                            usage("--expected-current needs an unsigned raw OS value")
                        })?;
                        if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                            return Err(usage("--expected-current must be a whole raw OS value from 0 through 4294967295"));
                        }
                        expected_current = Some(raw.parse::<u32>().map_err(|_| {
                            usage("--expected-current exceeds the unsigned 32-bit range")
                        })?);
                    }
                    value if percent.is_none() => {
                        let value = value.parse::<u16>().map_err(|_| {
                            usage("Brightness percent must be an integer from 0 through 100")
                        })?;
                        if value > 100 {
                            return Err(usage("Brightness percent must be 0 through 100"));
                        }
                        percent = Some(value as u8);
                    }
                    _ => return Err(usage("Unknown or repeated brightness argument")),
                }
                i += 1;
            }
            if dry && percent.is_none() {
                return Err(usage("--dry-run requires a target percent"));
            }
            if expected_current.is_some() && percent.is_none() {
                return Err(usage("--expected-current requires a target percent"));
            }
            Ok(Action::Brightness(percent, device, dry, expected_current))
        }
        "oem" if rest.first().is_some_and(|arg| arg == "windows-fn") => {
            windows_fn_cli::parse(&rest[1..]).map(Action::WindowsFn)
        }
        "oem" if rest.len() == 1 && rest[0] == "list" => Ok(Action::OemList),
        "oem" if (rest.len() == 3 || rest.len() == 4) && rest[0] == "set" => {
            let id = text(&rest[1])?;
            let value = text(&rest[2])?;
            if id.is_empty() || value.is_empty() || id.starts_with("--") || value.starts_with("--")
            {
                return Err(usage("oem set requires a control ID and a value"));
            }
            let dry = rest.len() == 4;
            if dry && rest[3] != "--dry-run" {
                return Err(usage("Unknown OEM option; only --dry-run is accepted"));
            }
            Ok(Action::OemSet(id.into(), value.into(), dry))
        }
        "oem" => Err(usage("Use oem list, oem set ID VALUE [--dry-run], or oem windows-fn probe|set on|off [--dry-run]")),
        "fan" => parse_fan(rest),
        "export" if rest.len() == 2 && rest[0] == "--output" && !rest[1].is_empty() => {
            Ok(Action::Export(PathBuf::from(&rest[1])))
        }
        "export" => Err(usage(
            "export requires --output PATH; existing files are never replaced",
        )),
        _ => Err(usage("Unknown command or invalid options; use --help")),
    }
}
fn parse_fan(args: &[OsString]) -> Result<Action, ControlError> {
    let command = args
        .first()
        .map(text)
        .transpose()?
        .ok_or_else(|| usage("Use fan validate --input PATH or fan hold --input PATH"))?;
    if !["validate", "hold"].contains(&command) {
        return Err(usage(
            "Use fan validate --input PATH or fan hold --input PATH",
        ));
    }
    let mut input = None;
    let mut policy = MinimumDutyPolicy::default();
    let mut index = 1;
    while index < args.len() {
        match text(&args[index])? {
            "--input" if input.is_none() => {
                index += 1;
                let path = args
                    .get(index)
                    .filter(|path| !path.is_empty() && !path.to_string_lossy().starts_with("--"))
                    .ok_or_else(|| usage("--input requires an explicit curve file path"))?;
                input = Some(PathBuf::from(path));
            }
            option @ ("--cpu-min-percent" | "--gpu-min-percent") => {
                let destination = if option == "--cpu-min-percent" {
                    &mut policy.cpu_min_percent
                } else {
                    &mut policy.gpu_min_percent
                };
                if destination.is_some() {
                    return Err(usage(format!("Repeated fan option {option}")));
                }
                index += 1;
                let value = args.get(index).map(text).transpose()?.ok_or_else(|| {
                    usage(format!(
                        "{option} requires a whole percent from 1 through 100"
                    ))
                })?;
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(usage(format!(
                        "{option} requires ASCII decimal digits from 1 through 100"
                    )));
                }
                let percent = value
                    .parse::<u8>()
                    .ok()
                    .filter(|value| (1..=100).contains(value))
                    .ok_or_else(|| usage(format!("{option} must be 1 through 100")))?;
                *destination = Some(percent);
            }
            "--raise-stopped-stages" if !policy.raise_stopped_stages => {
                policy.raise_stopped_stages = true;
            }
            _ => return Err(usage("Unknown or repeated fan option")),
        }
        index += 1;
    }
    let input = input.ok_or_else(|| usage("Fan commands require --input PATH"))?;
    policy.validate()?;
    Ok(if command == "validate" {
        Action::FanValidate(input, policy)
    } else {
        Action::FanHold(input, policy)
    })
}
fn parse_cpu_power(args: &[OsString]) -> Result<CliCpuPowerAction, ControlError> {
    let command = args
        .first()
        .map(text)
        .transpose()?
        .ok_or_else(|| usage("Use cpu-power probe, set WATTS, clear, recover or reapply"))?;
    let rest = &args[1..];
    match command {
        "probe" if rest.is_empty() => Ok(CliCpuPowerAction::Probe),
        "clear" | "recover" | "reapply" => {
            let dry_run = match rest {
                [] => false,
                [option] if option == "--dry-run" => true,
                _ => {
                    return Err(usage(
                        "This CPU operation accepts only an optional --dry-run",
                    ))
                }
            };
            Ok(match command {
                "clear" => CliCpuPowerAction::Clear { dry_run },
                "recover" => CliCpuPowerAction::Recover { dry_run },
                _ => CliCpuPowerAction::Reapply { dry_run },
            })
        }
        "set" => {
            let raw = rest
                .first()
                .map(text)
                .transpose()?
                .ok_or_else(|| usage("cpu-power set requires a positive whole-watt value"))?;
            if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(usage("CPU watts must be a positive whole decimal integer"));
            }
            let watts = raw
                .parse::<u64>()
                .ok()
                .filter(|watts| *watts != 0 && *watts <= i32::MAX as u64 / 1_000_000)
                .ok_or_else(|| {
                    usage("CPU watts must be 1..=2147 to fit the signed 32-bit microwatt protocol")
                })?;
            let mut options = CliCpuPolicyOptions::default();
            let mut dry_run = false;
            let mut i = 1;
            while i < rest.len() {
                let option = text(&rest[i])?;
                if option == "--dry-run" {
                    if dry_run {
                        return Err(usage("Repeated CPU --dry-run option"));
                    }
                    dry_run = true;
                    i += 1;
                    continue;
                }
                let (slot, qualified) = match option {
                    "--target-celsius" => (&mut options.target_celsius, "90"),
                    "--polling-seconds" => (&mut options.polling_seconds, "1"),
                    "--rapl-debounce-seconds" => (&mut options.rapl_debounce_seconds, "0"),
                    _ => return Err(usage("Unknown CPU policy option")),
                };
                if slot.is_some() {
                    return Err(usage("Repeated or conflicting CPU policy option"));
                }
                i += 1;
                let value = rest.get(i).map(text).transpose()?.ok_or_else(|| {
                    usage("CPU policy option requires native or its qualified value")
                })?;
                *slot = Some(if value == "native" {
                    CliCpuPolicySetting::Native
                } else if value == qualified {
                    CliCpuPolicySetting::Qualified
                } else {
                    return Err(usage(format!("{option} requires native or {qualified}")));
                });
                i += 1;
            }
            Ok(CliCpuPowerAction::Set {
                watts,
                options,
                dry_run,
            })
        }
        _ => Err(usage(
            "Use cpu-power probe, set WATTS [options] [--dry-run], clear, recover or reapply",
        )),
    }
}

mod cpu_power_cli {
    use super::*;
    use mechrevo_console_core::cpu_power::{
        Action as CpuAction, Client, CpuPowerUpdate, Outcome, Plan, Policy, Status,
    };

    trait Backend {
        fn probe_authorized(&mut self) -> CpuPowerUpdate;
        fn preview(&self, action: CpuAction) -> Result<Plan, ControlError>;
        fn execute_authorized(&mut self, plan: &Plan) -> CpuPowerUpdate;
    }
    impl Backend for Client {
        fn probe_authorized(&mut self) -> CpuPowerUpdate {
            Client::probe_authorized(self)
        }
        fn preview(&self, action: CpuAction) -> Result<Plan, ControlError> {
            Client::preview(self, action)
        }
        fn execute_authorized(&mut self, plan: &Plan) -> CpuPowerUpdate {
            Client::execute_authorized(self, plan)
        }
    }
    fn set_policy(
        status: &Status,
        watts: u64,
        options: &CliCpuPolicyOptions,
    ) -> Result<Policy, ControlError> {
        let cap_uw = watts
            .checked_mul(1_000_000)
            .filter(|cap| *cap != 0)
            .ok_or_else(|| {
                usage("CPU watts cannot be represented as a positive whole-watt policy")
            })?;
        let previous = status.current_policy.as_ref();
        Ok(Policy {
            cap_uw,
            target_celsius: match options.target_celsius {
                None => previous.and_then(|policy| policy.target_celsius),
                Some(CliCpuPolicySetting::Native) => None,
                Some(CliCpuPolicySetting::Qualified) => Some(90),
            },
            polling_seconds: match options.polling_seconds {
                None => previous.and_then(|policy| policy.polling_seconds),
                Some(CliCpuPolicySetting::Native) => None,
                Some(CliCpuPolicySetting::Qualified) => Some(1),
            },
            rapl_debounce_seconds: match options.rapl_debounce_seconds {
                None => previous.and_then(|policy| policy.rapl_debounce_seconds),
                Some(CliCpuPolicySetting::Native) => None,
                Some(CliCpuPolicySetting::Qualified) => Some(0),
            },
        })
    }
    fn update_error(update: &CpuPowerUpdate, expected: Outcome) -> Option<ControlError> {
        if let Some(error) = &update.error {
            return Some(error.clone());
        }
        if !update.helper_reaped {
            return Some(ControlError::new("cpu_power_helper_liveness_unknown", "The helper process has not been confirmed exited; no later policy operation was attempted"));
        }
        if update.outcome != expected {
            return Some(ControlError::new(
                "cpu_power_unverified_outcome",
                "The typed transaction outcome did not confirm the requested operation",
            ));
        }
        None
    }
    fn emit_update(
        writer: &mut impl Write,
        update: &CpuPowerUpdate,
        expected: Outcome,
    ) -> Result<(), ControlError> {
        if let Err(mut error) = write_json(writer, &json!(update)) {
            error.details.push(format!(
                "cpu_power_outcome={:?} helper_reaped={}",
                update.outcome, update.helper_reaped
            ));
            error.details.push(
                serde_json::to_string(update)
                    .unwrap_or_else(|_| "CPU outcome evidence could not be serialized".into()),
            );
            return Err(error);
        }
        if let Some(error) = update_error(update, expected) {
            return Err(error);
        }
        Ok(())
    }
    fn emit_preparation_error(
        writer: &mut impl Write,
        document: &Value,
        probe: &CpuPowerUpdate,
        primary: &ControlError,
    ) -> Result<(), ControlError> {
        write_json(writer, document).map_err(|mut output_error| {
            output_error.details.push(format!(
                "cpu_power_prepare_error={} helper_reaped={}",
                primary.code, probe.helper_reaped
            ));
            output_error.details.push(
                serde_json::to_string(primary)
                    .unwrap_or_else(|_| "CPU preparation error could not be serialized".into()),
            );
            output_error.details.push(document.to_string());
            output_error
        })
    }
    fn run_with(
        command: CliCpuPowerAction,
        backend: &mut impl Backend,
        writer: &mut impl Write,
    ) -> Result<(), ControlError> {
        // All CLI syntax is checked by parse() before this explicit probe. The
        // probe is authorized even for --dry-run; preview itself remains local.
        let probe = backend.probe_authorized();
        if command == CliCpuPowerAction::Probe {
            return emit_update(writer, &probe, Outcome::Observed);
        }
        if let Some(error) = update_error(&probe, Outcome::Observed) {
            emit_preparation_error(
                writer,
                &json!({"probe":probe,"plan":null,"error":error}),
                &probe,
                &error,
            )?;
            return Err(error);
        }
        let Some(status) = &probe.status else {
            let error = ControlError::new(
                "cpu_power_status_unavailable",
                "The explicit probe returned no complete policy observation",
            );
            emit_preparation_error(
                writer,
                &json!({"probe":probe,"plan":null,"error":error}),
                &probe,
                &error,
            )?;
            return Err(error);
        };
        let (action, dry_run) = match command {
            CliCpuPowerAction::Set {
                watts,
                options,
                dry_run,
            } => (
                CpuAction::Set {
                    policy: set_policy(status, watts, &options)?,
                },
                dry_run,
            ),
            CliCpuPowerAction::Clear { dry_run } => (CpuAction::Clear, dry_run),
            CliCpuPowerAction::Recover { dry_run } => (CpuAction::Recover, dry_run),
            CliCpuPowerAction::Reapply { dry_run } => (CpuAction::Reapply, dry_run),
            CliCpuPowerAction::Probe => unreachable!("probe returned above"),
        };
        let plan = match backend.preview(action) {
            Ok(plan) => plan,
            Err(error) => {
                emit_preparation_error(
                    writer,
                    &json!({"dry_run":dry_run,"probe":probe,"plan":null,"error":error}),
                    &probe,
                    &error,
                )?;
                return Err(error);
            }
        };
        if dry_run {
            return write_json(writer, &json!({"dry_run":true,"probe":probe,"plan":plan}));
        }
        let applied = backend.execute_authorized(&plan);
        emit_update(writer, &applied, Outcome::AppliedVerified)
    }
    pub(super) fn run(
        command: CliCpuPowerAction,
        writer: &mut impl Write,
    ) -> Result<(), ControlError> {
        run_with(command, &mut Client::new(), writer)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use mechrevo_console_core::cpu_power::{self, ActionKind, Management};
        use std::cell::RefCell;

        fn status(managed: bool) -> Status {
            serde_json::from_value(json!({
                "management":if managed {"managed"}else{"native"}, "revision":"a".repeat(64), "qualified":true,
                "allowed_actions":if managed {json!(["set","clear","reapply"])}else{json!(["set"])},
                "current_policy":if managed {json!({"cap_uw":55_000_000,"target_celsius":90,"polling_seconds":1,"rapl_debounce_seconds":0})}else{Value::Null},
                "operational_ceiling_uw":90_000_000,"minimum_limit_uw":35_000_000,"thermal_spec_reference_uw":55_000_000,
                "observed":[{"interface":"msr","pl1_uw":if managed {55_000_000}else{90_000_000}},
                    {"interface":"mmio","pl1_uw":if managed {55_000_000}else{90_000_000}}],
                "thermald_active":true,"holding_now":managed,"blocking_errors":[],"diagnostic_errors":[]
            })).unwrap()
        }
        fn observed(status: Status) -> CpuPowerUpdate {
            CpuPowerUpdate {
                status: Some(status),
                outcome: Outcome::Observed,
                error: None,
                receipt: None,
                helper_reaped: true,
            }
        }
        struct Stub {
            probe: CpuPowerUpdate,
            probes: usize,
            previews: RefCell<Vec<CpuAction>>,
            executions: Vec<Plan>,
            result: CpuPowerUpdate,
        }
        impl Stub {
            fn new(managed: bool) -> Self {
                let probe = observed(status(managed));
                let mut result = probe.clone();
                result.outcome = Outcome::AppliedVerified;
                result.receipt = Some(json!({"fixture_receipt":true}));
                Self {
                    probe,
                    probes: 0,
                    previews: RefCell::new(Vec::new()),
                    executions: Vec::new(),
                    result,
                }
            }
        }
        impl Backend for Stub {
            fn probe_authorized(&mut self) -> CpuPowerUpdate {
                self.probes += 1;
                self.probe.clone()
            }
            fn preview(&self, action: CpuAction) -> Result<Plan, ControlError> {
                self.previews.borrow_mut().push(action.clone());
                cpu_power::preview(
                    self.probe.status.as_ref().expect("probe status fixture"),
                    action,
                )
            }
            fn execute_authorized(&mut self, plan: &Plan) -> CpuPowerUpdate {
                self.executions.push(plan.clone());
                let mut result = self.result.clone();
                if result.outcome == Outcome::AppliedVerified && result.error.is_none() {
                    if let Some(status) = &mut result.status {
                        match &plan.action {
                            CpuAction::Set { policy } => {
                                status.management = Management::Managed;
                                status.current_policy = Some(policy.clone());
                                status.holding_now = true;
                                for limit in &mut status.observed {
                                    limit.pl1_uw = Some(policy.cap_uw);
                                }
                            }
                            CpuAction::Clear | CpuAction::Recover => {
                                status.management = Management::Native;
                                status.current_policy = None;
                                status.holding_now = false;
                                status.allowed_actions = vec![ActionKind::Set];
                                for limit in &mut status.observed {
                                    limit.pl1_uw = status.operational_ceiling_uw;
                                }
                            }
                            CpuAction::Reapply => {}
                        }
                        status.revision = Some("b".repeat(64));
                    }
                }
                result
            }
        }
        fn run_args(
            args: &[&str],
            backend: &mut Stub,
            writer: &mut impl Write,
        ) -> Result<(), ControlError> {
            let action = super::super::parse(args.iter().map(OsString::from).collect())?;
            let Action::CpuPower(command) = action else {
                panic!("CPU command fixture expected");
            };
            run_with(command, backend, writer)
        }
        #[test]
        fn invalid_cpu_syntax_is_rejected_before_probe_or_execution() {
            for args in [
                vec!["cpu-power"],
                vec!["cpu-power", "probe", "--json"],
                vec!["cpu-power", "probe", "--dry-run"],
                vec!["cpu-power", "set"],
                vec!["cpu-power", "set", "0"],
                vec!["cpu-power", "set", "-1"],
                vec!["cpu-power", "set", "+55"],
                vec!["cpu-power", "set", "55.0"],
                vec!["cpu-power", "set", "true"],
                vec!["cpu-power", "set", "５５"],
                vec!["cpu-power", "set", "18446744073709551615"],
                vec!["cpu-power", "set", "55", "--dry-run", "--dry-run"],
                vec!["cpu-power", "set", "55", "--target-celsius"],
                vec![
                    "cpu-power",
                    "set",
                    "55",
                    "--target-celsius",
                    "90",
                    "--target-celsius",
                    "native",
                ],
                vec![
                    "cpu-power",
                    "set",
                    "55",
                    "--polling-seconds",
                    "1",
                    "--polling-seconds",
                    "1",
                ],
                vec![
                    "cpu-power",
                    "set",
                    "55",
                    "--rapl-debounce-seconds",
                    "0",
                    "--rapl-debounce-seconds",
                    "native",
                ],
                vec!["cpu-power", "set", "55", "--target-celsius", "95"],
                vec!["cpu-power", "set", "55", "--polling-seconds", "2"],
                vec!["cpu-power", "set", "55", "--rapl-debounce-seconds", "1"],
                vec!["cpu-power", "set", "55", "--root", "/sys"],
                vec!["cpu-power", "set", "55", "--helper", "/tmp/program"],
                vec!["cpu-power", "set", "55", "--json"],
                vec!["cpu-power", "clear", "--target-celsius", "native"],
                vec!["cpu-power", "recover", "--dry-run", "--dry-run"],
                vec!["cpu-power", "reapply", "--path", "/tmp/state"],
                vec!["cpu-power", "apply"],
            ] {
                let mut backend = Stub::new(true);
                assert!(
                    run_args(&args, &mut backend, &mut Vec::new()).is_err(),
                    "Accepted {args:?}"
                );
                assert_eq!(backend.probes, 0, "Authorized invalid syntax: {args:?}");
                assert!(backend.previews.borrow().is_empty());
                assert!(backend.executions.is_empty());
            }
        }
        #[test]
        fn cpu_protocol_integer_boundary_is_checked_before_authorization() {
            for args in [
                vec!["cpu-power", "set", "2148"],
                vec!["cpu-power", "set", "2148", "--dry-run"],
            ] {
                let mut backend = Stub::new(false);
                assert!(run_args(&args, &mut backend, &mut Vec::new()).is_err());
                assert_eq!(backend.probes, 0);
                assert!(backend.previews.borrow().is_empty());
                assert!(backend.executions.is_empty());
            }
            assert!(matches!(
                super::super::parse(vec![
                    "cpu-power".into(),
                    "set".into(),
                    "2147".into(),
                    "--dry-run".into()
                ]),
                Ok(Action::CpuPower(CliCpuPowerAction::Set {
                    watts: 2147,
                    dry_run: true,
                    ..
                }))
            ));
            let mut backend = Stub::new(false);
            assert!(run_args(
                &["cpu-power", "set", "2147", "--dry-run"],
                &mut backend,
                &mut Vec::new()
            )
            .is_err());
            assert_eq!(backend.probes, 1);
            assert!(matches!(backend.previews.borrow().as_slice(),
                [CpuAction::Set { policy }] if policy.cap_uw == 2_147_000_000));
            assert!(backend.executions.is_empty());
        }
        #[test]
        fn dry_run_preserves_complete_existing_policy_including_explicit_zero() {
            let mut backend = Stub::new(true);
            let mut bytes = Vec::new();
            run_args(
                &["cpu-power", "set", "50", "--dry-run"],
                &mut backend,
                &mut bytes,
            )
            .unwrap();
            let output: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(backend.probes, 1);
            assert!(backend.executions.is_empty());
            assert_eq!(
                output["plan"]["action"]["policy"],
                json!({"cap_uw":50_000_000,"target_celsius":90,"polling_seconds":1,"rapl_debounce_seconds":0})
            );
            assert_eq!(
                output["probe"]["status"]["current_policy"]["rapl_debounce_seconds"],
                0
            );
            assert_eq!(output["dry_run"], true);
        }
        #[test]
        fn new_native_policy_keeps_explicit_nulls_and_uses_operational_not_thermal_reference_bound()
        {
            let mut backend = Stub::new(false);
            let mut bytes = Vec::new();
            run_args(
                &["cpu-power", "set", "60", "--dry-run"],
                &mut backend,
                &mut bytes,
            )
            .unwrap();
            let output: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                output["plan"]["action"]["policy"],
                json!({"cap_uw":60_000_000,"target_celsius":null,"polling_seconds":null,"rapl_debounce_seconds":null})
            );
            assert!(output["probe"]["status"]["current_policy"].is_null());
            assert!(backend.executions.is_empty());
        }
        #[test]
        fn explicit_policy_overrides_are_independent_and_dependencies_are_not_silently_reset() {
            let mut backend = Stub::new(true);
            let mut bytes = Vec::new();
            run_args(
                &[
                    "cpu-power",
                    "set",
                    "50",
                    "--target-celsius",
                    "native",
                    "--polling-seconds",
                    "native",
                    "--rapl-debounce-seconds",
                    "native",
                    "--dry-run",
                ],
                &mut backend,
                &mut bytes,
            )
            .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["plan"]["action"]["policy"],
                json!({"cap_uw":50_000_000,"target_celsius":null,"polling_seconds":null,"rapl_debounce_seconds":null})
            );
            let mut backend = Stub::new(true);
            assert!(run_args(
                &[
                    "cpu-power",
                    "set",
                    "50",
                    "--target-celsius",
                    "native",
                    "--dry-run"
                ],
                &mut backend,
                &mut Vec::new()
            )
            .is_err());
            assert_eq!(backend.probes, 1);
            assert!(backend.executions.is_empty());
            let action = serde_json::to_value(&backend.previews.borrow()[0]).unwrap();
            assert_eq!(action["policy"]["polling_seconds"], 1);
            assert_eq!(action["policy"]["rapl_debounce_seconds"], 0);
            let mut backend = Stub::new(false);
            run_args(
                &[
                    "cpu-power",
                    "set",
                    "55",
                    "--target-celsius",
                    "90",
                    "--polling-seconds",
                    "1",
                    "--rapl-debounce-seconds",
                    "0",
                    "--dry-run",
                ],
                &mut backend,
                &mut Vec::new(),
            )
            .unwrap();
        }
        #[test]
        fn core_range_and_allowed_action_checks_fail_before_execution() {
            for watts in ["34", "91"] {
                let mut backend = Stub::new(true);
                assert!(run_args(
                    &["cpu-power", "set", watts, "--dry-run"],
                    &mut backend,
                    &mut Vec::new()
                )
                .is_err());
                assert_eq!(backend.probes, 1);
                assert!(backend.executions.is_empty());
            }
            let mut backend = Stub::new(false);
            assert!(run_args(&["cpu-power", "clear"], &mut backend, &mut Vec::new()).is_err());
            assert!(backend.executions.is_empty());
        }
        #[test]
        fn actual_execution_is_explicit_once_and_dry_actions_only_preview() {
            let mut backend = Stub::new(true);
            let mut bytes = Vec::new();
            run_args(&["cpu-power", "set", "50"], &mut backend, &mut bytes).unwrap();
            assert_eq!(backend.probes, 1);
            assert_eq!(backend.executions.len(), 1);
            assert_eq!(
                serde_json::to_value(&backend.executions[0]).unwrap()["action"]["policy"]
                    ["target_celsius"],
                90
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["outcome"],
                "applied_verified"
            );
            for operation in ["clear", "reapply"] {
                let mut backend = Stub::new(true);
                run_args(
                    &["cpu-power", operation, "--dry-run"],
                    &mut backend,
                    &mut Vec::new(),
                )
                .unwrap();
                assert_eq!(backend.probes, 1);
                assert!(backend.executions.is_empty());
            }
        }
        #[test]
        fn explicit_probe_is_read_only_and_recover_uses_the_advertised_pending_action() {
            let mut backend = Stub::new(true);
            let mut bytes = Vec::new();
            run_args(&["cpu-power", "probe"], &mut backend, &mut bytes).unwrap();
            assert_eq!(backend.probes, 1);
            assert!(backend.previews.borrow().is_empty());
            assert!(backend.executions.is_empty());
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["outcome"],
                "observed"
            );
            let mut backend = Stub::new(true);
            let status = backend.probe.status.as_mut().unwrap();
            status.management = Management::RecoveryPending;
            status.holding_now = false;
            status.allowed_actions = vec![ActionKind::Recover];
            run_args(
                &["cpu-power", "recover", "--dry-run"],
                &mut backend,
                &mut Vec::new(),
            )
            .unwrap();
            assert!(backend.executions.is_empty());
            run_args(&["cpu-power", "recover"], &mut backend, &mut Vec::new()).unwrap();
            assert_eq!(backend.executions.len(), 1);
            assert!(matches!(backend.executions[0].action, CpuAction::Recover));
        }
        #[test]
        fn partial_probe_errors_or_unknown_liveness_block_preview_and_keep_status_visible() {
            for liveness in [false, true] {
                let mut backend = Stub::new(true);
                let mut bytes = Vec::new();
                backend.probe.helper_reaped = !liveness;
                if !liveness {
                    backend.probe.error = Some(ControlError::new(
                        "partial_probe",
                        "Qualification failed after partial observation",
                    ));
                }
                assert!(run_args(&["cpu-power", "set", "50"], &mut backend, &mut bytes).is_err());
                let output: Value = serde_json::from_slice(&bytes).unwrap();
                assert!(output["probe"]["status"].is_object());
                assert!(output["plan"].is_null());
                assert!(backend.previews.borrow().is_empty());
                assert!(backend.executions.is_empty());
            }
        }
        #[test]
        fn failed_execution_keeps_typed_outcome_primary_error_and_receipt_without_retry() {
            let mut backend = Stub::new(true);
            let mut bytes = Vec::new();
            backend.result.outcome = Outcome::FailedRestoredVerified;
            backend.result.error = Some(ControlError::new(
                "write_failed",
                "Primary write failed; rollback separately verified",
            ));
            backend.result.receipt =
                Some(json!({"primary_errno":5,"restore_verified":true,"receipt_write_error":null}));
            assert!(run_args(&["cpu-power", "set", "50"], &mut backend, &mut bytes).is_err());
            let output: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(output["outcome"], "failed_restored_verified");
            assert_eq!(output["error"]["code"], "write_failed");
            assert_eq!(output["receipt"]["restore_verified"], true);
            assert_eq!(backend.executions.len(), 1);
        }
        #[test]
        fn broken_output_during_probe_or_preview_failure_keeps_original_error_and_liveness() {
            struct Broken;
            impl Write for Broken {
                fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                    Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "fixture pipe closed",
                    ))
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            let mut backend = Stub::new(true);
            backend.probe.helper_reaped = false;
            backend.probe.error = Some(ControlError::new("probe_primary", "Original probe error"));
            let error =
                run_args(&["cpu-power", "set", "50"], &mut backend, &mut Broken).unwrap_err();
            assert_eq!(error.code, "output_failed");
            assert!(error
                .details
                .iter()
                .any(|detail| detail.contains("probe_primary")
                    && detail.contains("helper_reaped=false")));
            assert!(backend.executions.is_empty());
            assert!(backend.previews.borrow().is_empty());
            let mut backend = Stub::new(true);
            let error = run_args(
                &["cpu-power", "set", "91", "--dry-run"],
                &mut backend,
                &mut Broken,
            )
            .unwrap_err();
            assert_eq!(error.code, "output_failed");
            assert!(error
                .details
                .iter()
                .any(|detail| detail.contains("cpu_power_prepare_error=")
                    && detail.contains("helper_reaped=true")));
            assert!(error
                .details
                .iter()
                .any(|detail| detail.contains("\"plan\":null") && detail.contains("\"probe\"")));
            assert!(backend.executions.is_empty());
        }
        #[test]
        fn broken_output_after_execute_retains_outcome_and_never_repeats_policy_write() {
            struct Broken;
            impl Write for Broken {
                fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                    Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "fixture pipe closed",
                    ))
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            let mut backend = Stub::new(true);
            let error =
                run_args(&["cpu-power", "set", "50"], &mut backend, &mut Broken).unwrap_err();
            assert_eq!(backend.executions.len(), 1);
            assert!(error
                .details
                .iter()
                .any(|detail| detail.contains("AppliedVerified")));
            assert!(error
                .details
                .iter()
                .any(|detail| detail.contains("fixture_receipt")));
            let mut backend = Stub::new(true);
            assert!(run_args(
                &["cpu-power", "set", "50", "--dry-run"],
                &mut backend,
                &mut Broken
            )
            .is_err());
            assert!(backend.executions.is_empty());
        }
    }
}
mod brightness_cli {
    use super::*;
    use mechrevo_console_core::{
        brightness, BrightnessApplyResult, BrightnessDevice, BrightnessPlan,
    };

    const CONTEXT_MARKER: &str = "brightness_cli_context=";

    trait Backend {
        fn devices(&self) -> Vec<BrightnessDevice>;
        fn plan(&self, percent: u8, device: Option<&str>) -> Result<BrightnessPlan, ControlError>;
        fn apply(&self, plan: &BrightnessPlan) -> Result<BrightnessApplyResult, ControlError>;
    }
    impl Backend for Service {
        fn devices(&self) -> Vec<BrightnessDevice> {
            self.brightness_devices()
        }
        fn plan(&self, percent: u8, device: Option<&str>) -> Result<BrightnessPlan, ControlError> {
            self.plan_brightness(percent, device)
        }
        fn apply(&self, plan: &BrightnessPlan) -> Result<BrightnessApplyResult, ControlError> {
            self.apply_brightness(plan)
        }
    }
    fn with_context(mut error: ControlError, context: Value) -> ControlError {
        error.details.push(format!("{CONTEXT_MARKER}{context}"));
        error
    }
    pub(super) fn error_document(error: &ControlError) -> Value {
        let mut document = json!({"error":error});
        if let Some(transaction) = brightness::transaction_from_error(error) {
            document["brightness_transaction"] = json!(transaction);
        }
        if let Some(context) = error.details.iter().rev().find_map(|detail| {
            detail
                .strip_prefix(CONTEXT_MARKER)
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        }) {
            document["brightness_context"] = context;
        }
        document
    }
    fn run_with(
        percent: Option<u8>,
        device: Option<&str>,
        dry_run: bool,
        expected_current: Option<u32>,
        backend: &impl Backend,
        writer: &mut impl Write,
    ) -> Result<(), ControlError> {
        let Some(percent) = percent else {
            let mut devices = backend.devices();
            if let Some(id) = device {
                devices.retain(|candidate| candidate.id == id);
                if devices.is_empty() {
                    return Err(ControlError::new(
                        "brightness_device_unavailable",
                        "Requested device was not enumerated",
                    ));
                }
                if devices.len() != 1 {
                    return Err(ControlError::new(
                        "brightness_device_ambiguous",
                        "Multiple brightness devices have this exact ID; no write was attempted",
                    ));
                }
            }
            return write_json(writer, &json!(devices));
        };
        let plan = backend.plan(percent, device)?;
        if let Some(expected) = expected_current {
            if expected != plan.previous {
                return Err(with_context(ControlError::new("brightness_plan_stale",
                    "The raw OS brightness no longer matches --expected-current; no write was attempted"),
                    json!({"plan":plan,"expected_current":expected,"apply_attempted":false})));
            }
        }
        if dry_run {
            return write_json(writer, &json!({"dry_run":true,"plan":plan}));
        }
        let applied = backend.apply(&plan).map_err(|error| {
            with_context(error, json!({"plan":plan,"apply_returned_success":false}))
        })?;
        let mut output = json!(applied);
        // The backend verifies this exact route and never silently falls back.
        // source identifies the observed OS interface; it is not the write route.
        output["write_backend"] = json!(plan.backend);
        output["logind_session"] = json!(plan.logind_session);
        output["requested_control"] = json!(plan.target);
        write_json(writer, &output).map_err(|error| with_context(error,
            json!({"plan":plan,"apply_returned_success":true,"applied_result":output,
                "scope":"The apply result was returned before output failed; no retry or restoration was initiated by the CLI."})))
    }
    pub(super) fn run(
        percent: Option<u8>,
        device: Option<&str>,
        dry_run: bool,
        expected_current: Option<u32>,
        service: &Service,
        writer: &mut impl Write,
    ) -> Result<(), ControlError> {
        run_with(percent, device, dry_run, expected_current, service, writer)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::{Cell, RefCell};

        fn source() -> Value {
            json!({"backend":"linux_backlight","path":"/fixture/devices/panel","device_id":"panel"})
        }
        fn binding() -> Value {
            json!({"session_id":"3","object_path":"/org/freedesktop/login1/session/_33",
                "uid":1000,"seat_id":"seat0","seat_path":"/org/freedesktop/login1/seat/seat0"})
        }
        fn transaction(outcome: &str, recovery: &str) -> Value {
            json!({"outcome":outcome,"write_attempted":outcome!="not_applied" && outcome!="conflict",
                "writer_may_continue":outcome=="unknown","observed_control":if outcome=="unknown" {Value::Null}else{json!(700)},
                "primary_error":null,"rollback_error":null,"readback_error":null,
                "recovery":recovery,"recovery_reason":null})
        }
        struct Stub {
            devices: Vec<BrightnessDevice>,
            enumerations: Cell<usize>,
            plan: BrightnessPlan,
            plans: RefCell<Vec<(u8, Option<String>)>>,
            plan_error: Option<ControlError>,
            applications: Cell<usize>,
            result: Result<BrightnessApplyResult, ControlError>,
        }
        impl Stub {
            fn new() -> Self {
                let device = serde_json::from_value(json!({"id":"panel","label":"Fixture screen",
                    "current_percent":70.0,"control_value":700,"actual_value":690,"maximum":1000,
                    "source":source(),"status":"available","readable":true,"writable":false,
                    "write_backend":"linux_logind","logind_session":binding(),"supported_percentages":[],"reason":null})).unwrap();
                let plan =
                    serde_json::from_value(json!({"device_id":"panel","requested_percent":50,
                    "target":500,"previous":700,"maximum":1000,"source":source(),
                    "backend":"linux_logind","logind_session":binding()}))
                    .unwrap();
                let mut tx = transaction("confirmed", "not_needed");
                tx["observed_control"] = json!(500);
                let result = serde_json::from_value(json!({"device_id":"panel","requested_percent":50,
                    "confirmed":500,"previous":700,"actual_value":490,"source":source(),"transaction":tx})).unwrap();
                Self {
                    devices: vec![device],
                    enumerations: Cell::new(0),
                    plan,
                    plans: RefCell::new(Vec::new()),
                    plan_error: None,
                    applications: Cell::new(0),
                    result: Ok(result),
                }
            }
        }
        impl Backend for Stub {
            fn devices(&self) -> Vec<BrightnessDevice> {
                self.enumerations.set(self.enumerations.get() + 1);
                self.devices.clone()
            }
            fn plan(
                &self,
                percent: u8,
                device: Option<&str>,
            ) -> Result<BrightnessPlan, ControlError> {
                self.plans
                    .borrow_mut()
                    .push((percent, device.map(str::to_owned)));
                if let Some(error) = &self.plan_error {
                    return Err(error.clone());
                }
                Ok(self.plan.clone())
            }
            fn apply(&self, _: &BrightnessPlan) -> Result<BrightnessApplyResult, ControlError> {
                self.applications.set(self.applications.get() + 1);
                self.result.clone()
            }
        }
        fn run_args(
            args: &[&str],
            backend: &Stub,
            output: &mut impl Write,
        ) -> Result<(), ControlError> {
            let action = super::super::parse(args.iter().map(OsString::from).collect())?;
            let Action::Brightness(percent, device, dry, expected) = action else {
                panic!("brightness command fixture expected");
            };
            run_with(percent, device.as_deref(), dry, expected, backend, output)
        }
        #[test]
        fn expected_raw_value_syntax_and_missing_device_are_rejected_before_backend_calls() {
            for args in [
                vec!["brightness", "50", "--expected-current"],
                vec!["brightness", "50", "--expected-current", "4294967296"],
                vec!["brightness", "50", "--expected-current", "-1"],
                vec!["brightness", "50", "--expected-current", "+1"],
                vec!["brightness", "50", "--expected-current", "1.0"],
                vec!["brightness", "50", "--expected-current", "７００"],
                vec![
                    "brightness",
                    "50",
                    "--expected-current",
                    "0",
                    "--expected-current",
                    "0",
                ],
                vec!["brightness", "--expected-current", "700"],
                vec![
                    "brightness",
                    "--device",
                    "panel",
                    "--expected-current",
                    "700",
                ],
                vec!["brightness", "50", "--device", "--dry-run"],
                vec!["brightness", "50", "--expected-current", "--dry-run"],
            ] {
                let backend = Stub::new();
                assert!(
                    run_args(&args, &backend, &mut Vec::new()).is_err(),
                    "accepted {args:?}"
                );
                assert_eq!(backend.enumerations.get(), 0);
                assert!(backend.plans.borrow().is_empty());
                assert_eq!(backend.applications.get(), 0);
            }
            for raw in ["0", "4294967295"] {
                assert!(super::super::parse(vec![
                    "brightness".into(),
                    "50".into(),
                    "--expected-current".into(),
                    raw.into(),
                    "--dry-run".into()
                ])
                .is_ok());
            }
        }
        #[test]
        fn listing_preserves_source_route_requested_and_actual_as_distinct_fields() {
            let backend = Stub::new();
            let mut bytes = Vec::new();
            run_args(&["brightness", "--device", "panel"], &backend, &mut bytes).unwrap();
            let output: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(output.is_array());
            assert_eq!(output[0]["source"], source());
            assert_eq!(output[0]["write_backend"], "linux_logind");
            assert_eq!(output[0]["control_value"], 700);
            assert_eq!(output[0]["actual_value"], 690);
            assert_eq!(output[0]["writable"], false);
            assert_eq!(backend.enumerations.get(), 1);
            assert!(backend.plans.borrow().is_empty());
            assert_eq!(backend.applications.get(), 0);
            assert!(backend.devices[0].can_control());
        }
        #[test]
        fn explicit_device_is_forwarded_and_ambiguous_or_missing_devices_never_apply() {
            let backend = Stub::new();
            run_args(
                &["brightness", "50", "--device", "panel", "--dry-run"],
                &backend,
                &mut Vec::new(),
            )
            .unwrap();
            assert_eq!(*backend.plans.borrow(), vec![(50, Some("panel".into()))]);
            assert_eq!(backend.applications.get(), 0);
            for code in ["brightness_device_ambiguous", "brightness_device_missing"] {
                let mut backend = Stub::new();
                backend.plan_error =
                    Some(ControlError::new(code, "fixture device selection refused"));
                let error = run_args(&["brightness", "50"], &backend, &mut Vec::new()).unwrap_err();
                assert_eq!(error.code, code);
                assert_eq!(backend.applications.get(), 0);
            }
            let mut backend = Stub::new();
            backend.devices.push(backend.devices[0].clone());
            assert_eq!(
                run_args(
                    &["brightness", "--device", "panel"],
                    &backend,
                    &mut Vec::new()
                )
                .unwrap_err()
                .code,
                "brightness_device_ambiguous"
            );
            assert_eq!(backend.applications.get(), 0);
        }
        #[test]
        fn logind_dry_run_has_original_shape_and_never_calls_apply() {
            let backend = Stub::new();
            let mut bytes = Vec::new();
            run_args(
                &[
                    "brightness",
                    "50",
                    "--device",
                    "panel",
                    "--expected-current",
                    "700",
                    "--dry-run",
                ],
                &backend,
                &mut bytes,
            )
            .unwrap();
            let output: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(output, json!({"dry_run":true,"plan":backend.plan}));
            assert_eq!(output["plan"]["backend"], "linux_logind");
            assert_eq!(backend.applications.get(), 0);
        }
        #[test]
        fn expected_current_guard_matches_raw_plan_previous_and_does_not_replan_or_apply_on_mismatch(
        ) {
            let backend = Stub::new();
            run_args(
                &["brightness", "50", "--expected-current", "700"],
                &backend,
                &mut Vec::new(),
            )
            .unwrap();
            assert_eq!(backend.plans.borrow().len(), 1);
            assert_eq!(backend.applications.get(), 1);
            for dry in [false, true] {
                let backend = Stub::new();
                let mut args = vec!["brightness", "50", "--expected-current", "699"];
                if dry {
                    args.push("--dry-run");
                }
                let mut bytes = Vec::new();
                let error = run_args(&args, &backend, &mut bytes).unwrap_err();
                assert_eq!(error.code, "brightness_plan_stale");
                assert_eq!(backend.plans.borrow().len(), 1);
                assert_eq!(backend.applications.get(), 0);
                assert!(bytes.is_empty());
                let document = error_document(&error);
                assert_eq!(document["brightness_context"]["expected_current"], 699);
                assert_eq!(document["brightness_context"]["plan"]["previous"], 700);
                assert_eq!(document["brightness_context"]["apply_attempted"], false);
            }
        }
        #[test]
        fn successful_apply_preserves_flat_result_and_separates_write_route_from_observation() {
            let backend = Stub::new();
            let mut bytes = Vec::new();
            run_args(&["brightness", "50"], &backend, &mut bytes).unwrap();
            let output: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(output["source"], source());
            assert_eq!(output["write_backend"], "linux_logind");
            assert_eq!(output["logind_session"], binding());
            assert_eq!(output["requested_percent"], 50);
            assert_eq!(output["requested_control"], 500);
            assert_eq!(output["confirmed"], 500);
            assert_eq!(output["actual_value"], 490);
            assert_eq!(output["transaction"]["outcome"], "confirmed");
            assert!(!output["transaction"]["writer_may_continue"]
                .as_bool()
                .unwrap());
            assert_eq!(backend.applications.get(), 1);
        }
        #[test]
        fn failed_apply_retains_all_typed_transaction_states_and_primary_secondary_errors() {
            for (outcome, recovery) in [
                ("not_applied", "not_needed"),
                ("conflict", "skipped"),
                ("restored", "restored"),
                ("recovery_failed", "failed"),
                ("unknown", "unknown"),
            ] {
                let mut backend = Stub::new();
                let mut tx = transaction(outcome, recovery);
                tx["primary_error"] = json!({"code":"fixture_primary","message":"write or readback failed","details":["primary evidence"]});
                if outcome == "recovery_failed" {
                    tx["rollback_error"] = json!({"code":"fixture_rollback","message":"recovery failed","details":["rollback evidence"]});
                    tx["readback_error"] = json!({"code":"fixture_readback","message":"confirmation failed","details":[]});
                }
                let typed: brightness::BrightnessTransaction =
                    serde_json::from_value(tx.clone()).unwrap();
                let mut failure = ControlError::new(
                    if outcome == "unknown" {
                        "brightness_outcome_unknown"
                    } else {
                        "brightness_write_failed"
                    },
                    "fixture primary failure",
                );
                failure.details = vec![
                    "original driver evidence".into(),
                    format!(
                        "{}{}",
                        brightness::BRIGHTNESS_TRANSACTION_MARKER,
                        serde_json::to_string(&typed).unwrap()
                    ),
                ];
                backend.result = Err(failure.clone());
                let mut bytes = Vec::new();
                let error = run_args(
                    &["brightness", "50", "--device", "panel"],
                    &backend,
                    &mut bytes,
                )
                .unwrap_err();
                assert_eq!(error.code, failure.code);
                assert_eq!(
                    &error.details[..failure.details.len()],
                    failure.details.as_slice()
                );
                assert_eq!(backend.applications.get(), 1);
                assert!(bytes.is_empty());
                let document = error_document(&error);
                assert_eq!(document["brightness_transaction"], tx);
                assert_eq!(document["brightness_context"]["plan"]["source"], source());
                assert_eq!(
                    document["brightness_context"]["plan"]["backend"],
                    "linux_logind"
                );
                assert_eq!(
                    document["brightness_context"]["apply_returned_success"],
                    false
                );
            }
            let unrelated = ControlError::new("unrelated", "fixture");
            assert_eq!(error_document(&unrelated), json!({"error":unrelated}));
        }
        #[test]
        fn output_failure_after_apply_keeps_completed_transaction_without_retry_or_rollback() {
            struct Broken {
                remaining: usize,
            }
            impl Write for Broken {
                fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                    if self.remaining == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "fixture closed output",
                        ));
                    }
                    let count = self.remaining.min(bytes.len());
                    self.remaining -= count;
                    Ok(count)
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            let mut successful_bytes = Vec::new();
            run_args(&["brightness", "50"], &Stub::new(), &mut successful_bytes).unwrap();
            // Cover both an immediately closed pipe and failure on the final
            // newline after the complete JSON document has already been sent.
            for remaining in [0, successful_bytes.len() - 1] {
                let backend = Stub::new();
                let error = run_args(&["brightness", "50"], &backend, &mut Broken { remaining })
                    .unwrap_err();
                assert_eq!(error.code, "output_failed");
                assert_eq!(backend.applications.get(), 1);
                let document = error_document(&error);
                let context = &document["brightness_context"];
                assert_eq!(context["apply_returned_success"], true);
                assert_eq!(context["applied_result"]["write_backend"], "linux_logind");
                assert_eq!(context["applied_result"]["requested_control"], 500);
                assert_eq!(context["applied_result"]["actual_value"], 490);
                assert_eq!(
                    context["applied_result"]["transaction"]["outcome"],
                    "confirmed"
                );
            }
            let backend = Stub::new();
            assert!(run_args(
                &["brightness", "50", "--dry-run"],
                &backend,
                &mut Broken { remaining: 0 }
            )
            .is_err());
            assert_eq!(backend.applications.get(), 0);
        }
    }
}

fn select_oem_control<'a>(
    controls: &'a [OemControl],
    id: &str,
) -> Result<&'a OemControl, ControlError> {
    let mut matching = controls.iter().filter(|control| control.id == id);
    let selected = matching.next().ok_or_else(|| {
        ControlError::new(
            "oem_control_unavailable",
            "Requested OEM control was not enumerated; use oem list for exact IDs",
        )
    })?;
    if matching.next().is_some() {
        return Err(ControlError::new(
            "oem_control_ambiguous",
            "Multiple OEM controls have this ID; no write was attempted",
        ));
    }
    Ok(selected)
}
fn oem_value_error(message: impl Into<String>) -> ControlError {
    ControlError::new("invalid_oem_value", message)
}
fn parse_oem_value(
    kind: &OemControlKind,
    domain: &OemDomain,
    raw: &str,
) -> Result<OemValue, ControlError> {
    match (kind, domain) {
        (
            OemControlKind::FnLock | OemControlKind::AcAutoBoot | OemControlKind::UsbPowerShare,
            OemDomain::Boolean,
        ) => match raw {
            "on" => Ok(OemValue::Boolean(true)),
            "off" => Ok(OemValue::Boolean(false)),
            _ => Err(oem_value_error("Boolean OEM controls require on or off")),
        },
        (OemControlKind::ChargingProfile, OemDomain::Choices(choices)) => {
            if choices.iter().any(|choice| choice == raw) {
                Ok(OemValue::Choice(raw.into()))
            } else {
                Err(oem_value_error(
                    "Charging profile must be one of the exact choices returned by oem list",
                ))
            }
        }
        (OemControlKind::KeyboardBrightness, OemDomain::Level { max }) => {
            if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(oem_value_error(
                    "Keyboard brightness requires a nonnegative integer level",
                ));
            }
            let level = raw.parse::<u32>().map_err(|_| {
                oem_value_error("Keyboard brightness level exceeds the integer range")
            })?;
            if level > *max {
                return Err(oem_value_error(format!(
                    "Keyboard brightness must be between 0 and {max}"
                )));
            }
            Ok(OemValue::Level(level))
        }
        (OemControlKind::KeyboardColor, OemDomain::Rgb) => {
            if raw.len() != 7
                || !raw.starts_with('#')
                || !raw.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            {
                return Err(oem_value_error("Keyboard color requires #RRGGBB"));
            }
            let rgb = u32::from_str_radix(&raw[1..], 16)
                .map_err(|_| oem_value_error("Keyboard color requires #RRGGBB"))?;
            Ok(OemValue::Rgb([
                (rgb >> 16) as u8,
                (rgb >> 8) as u8,
                rgb as u8,
            ]))
        }
        _ => Err(ControlError::new(
            "invalid_oem_domain",
            "Enumerated OEM control kind and value domain do not agree; no write was attempted",
        )),
    }
}
fn profile_output(
    dry_run: bool,
    plan: &mechrevo_console_core::ProfilePlan,
    expected_current: Option<&str>,
    apply: impl FnOnce() -> Result<mechrevo_console_core::ApplyResult, ControlError>,
) -> Result<Value, ControlError> {
    if expected_current.is_some_and(|expected| expected != plan.previous) {
        return Err(ControlError::new(
            "profile_expected_current_changed",
            "The current raw OS profile differs from --expected-current; no setter was invoked",
        ));
    }
    if dry_run {
        Ok(json!({"dry_run":true,"plan":plan}))
    } else {
        Ok(json!(apply()?))
    }
}

fn write_profile_output(output: &Value, writer: &mut impl Write) -> Result<(), ControlError> {
    write_json(writer, output).map_err(|mut error| {
        // A broken stdout does not undo an acknowledged system policy change.
        // Preserve the completed operation instead of executing it again.
        error
            .details
            .push(format!("completed_profile_output={output}"));
        error
    })
}

fn oem_set_output(
    dry_run: bool,
    plan: Value,
    apply: impl FnOnce() -> Result<Value, ControlError>,
) -> Result<Value, ControlError> {
    if dry_run {
        Ok(json!({"dry_run":true,"plan":plan}))
    } else {
        apply()
    }
}
fn set_oem(service: &Service, id: &str, raw: &str, dry_run: bool) -> Result<Value, ControlError> {
    let controls = service.oem_controls();
    let control = select_oem_control(&controls, id)?;
    let target = parse_oem_value(&control.kind, &control.domain, raw)?;
    let plan = service.plan_oem(id, target)?;
    let plan_json = serde_json::to_value(&plan)
        .map_err(|error| ControlError::new("serialization_failed", error.to_string()))?;
    oem_set_output(dry_run, plan_json, || {
        let applied = mechrevo_console_core::privilege::apply_oem(service, &plan)?;
        serde_json::to_value(applied)
            .map_err(|error| ControlError::new("serialization_failed", error.to_string()))
    })
}
fn duration_json(duration: Duration) -> Value {
    json!({"seconds":duration.as_secs(),"nanoseconds":duration.subsec_nanos()})
}

// @mark mechrevo.fan_curve role=caller contract=ramfan1p5_typed_input
fn curve_validation_json(
    curve: &ValidatedFanCurve,
    minimum_duty_policy: MinimumDutyPolicy,
) -> Result<Value, ControlError> {
    let curve = curve.with_minimum_duty(minimum_duty_policy)?;
    Ok(json!({
        "outcome": "protocol_validated",
        "applied": false,
        "hardware_qualified": false,
        "validation_scope": "Curve file shape and RamFan1p5 protocol only; device support and thermal behavior require separate qualification",
        "table_write_bytes": OWNED_TABLE_BYTES,
        "duty_raw_unit": "half_percent",
        "curve": curve.curve(),
        "minimum_duty_policy": minimum_duty_policy
    }))
}

fn load_curve_file(
    path: &std::path::Path,
    minimum_duty_policy: MinimumDutyPolicy,
) -> Result<ValidatedFanCurve, ControlError> {
    // Internal callers must reject invalid policies before opening any file too.
    minimum_duty_policy.validate()?;
    let metadata = std::fs::metadata(path)
        .map_err(|error| ControlError::new("curve_read_failed", error.to_string()))?;
    if !metadata.is_file() {
        return Err(ControlError::new(
            "curve_read_failed",
            "Curve input must be a regular file",
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| ControlError::new("curve_read_failed", error.to_string()))?;
    ValidatedFanCurve::from_reader(file)
}

#[cfg(target_os = "linux")]
mod fan_hold {
    use super::*;
    use mechrevo_console_core::fan_curve::linux::{
        FanCurvePhase, FanCurveSession, FanCurveSessionError, FanCurveState,
    };
    use std::io::IsTerminal;

    pub(super) fn state_json(state: &FanCurveState) -> Value {
        let phase = match state.phase {
            FanCurvePhase::Idle => "idle",
            FanCurvePhase::Acquired => "acquired",
            FanCurvePhase::Active => "active",
            FanCurvePhase::RecoveryRequired => "recovery_required",
        };
        json!({
            "epoch":state.token.epoch, "generation":state.token.generation,
            "lease_id":state.token.lease_id, "last_request_id":state.last_request_id,
            "phase":phase, "owner":state.owner,
            "modified_or_uncertain":state.applied, "revoked":state.revoked,
            "last_error":state.last_error, "restore_error":state.restore_error,
            "restored_verified":state.restored_verified, "live_error":state.live_error,
            "live_table_available":state.live_table.is_some()
        })
    }

    fn session_error(error: FanCurveSessionError) -> ControlError {
        let mut result = ControlError::new("fan_curve_operation_failed", error.to_string());
        result.details.push(
            json!({
                "operation":error.operation, "errno":error.raw_os_error(),
                "state_is_current":error.state_is_current,
                "state":error.state.as_ref().map(|state|state_json(state)),
                "state_query_error":error.state_query_error,
                "state_query_errno":error.state_query_errno,
                "invalid_response":error.invalid_response.as_deref(),
                "state_query_invalid_response":error.state_query_invalid_response.as_deref(),
                "mutation_may_have_occurred":error.mutation_may_have_occurred,
                "close_fallback_outcome":"unconfirmed"
            })
            .to_string(),
        );
        result
    }

    pub(super) trait Holder {
        fn acquire(&mut self) -> Result<Value, ControlError>;
        fn apply(&mut self, curve: &ValidatedFanCurve) -> Result<Value, ControlError>;
        fn restore(&mut self) -> Result<Value, ControlError>;
    }
    impl Holder for FanCurveSession {
        fn acquire(&mut self) -> Result<Value, ControlError> {
            FanCurveSession::acquire(self)
                .map(|state| state_json(&state))
                .map_err(session_error)
        }
        fn apply(&mut self, curve: &ValidatedFanCurve) -> Result<Value, ControlError> {
            FanCurveSession::apply(self, curve)
                .map(|state| state_json(&state))
                .map_err(session_error)
        }
        fn restore(&mut self) -> Result<Value, ControlError> {
            FanCurveSession::restore(self)
                .map(|state| state_json(&state))
                .map_err(session_error)
        }
    }

    fn recover(holder: &mut impl Holder, mut primary: ControlError) -> ControlError {
        let recovery = match holder.restore() {
            Ok(state) => release_receipt(&state),
            Err(error) => json!({"restoration":"unconfirmed", "error":error}),
        };
        primary.details.push(recovery.to_string());
        primary
    }

    fn release_receipt(state: &Value) -> Value {
        let outcome = match state["restored_verified"].as_bool() {
            Some(true) => "driver_verified",
            Some(false) => "released_without_restoration_required",
            None => "unconfirmed",
        };
        json!({"restoration":outcome,"state":state})
    }

    // @mark mechrevo.fan_curve role=caller contract=fd_owned_curve_lifetime
    pub(super) fn hold(
        holder: &mut impl Holder,
        curve: &ValidatedFanCurve,
        minimum_duty_policy: MinimumDutyPolicy,
        wait: impl FnOnce() -> io::Result<()>,
        mut emit: impl FnMut(Value) -> Result<(), ControlError>,
    ) -> Result<(), ControlError> {
        let curve = curve.with_minimum_duty(minimum_duty_policy)?;
        let transaction = (|| {
            holder.acquire()?;
            let state = holder.apply(&curve)?;
            emit(json!({"outcome":"holding", "state":state,
                "curve":curve.curve(), "minimum_duty_policy":minimum_duty_policy,
                "lifetime":"This foreground process retains the exclusive lease until Enter or exit; it is not a saved startup setting"}))?;
            wait().map_err(|error| ControlError::new("fan_hold_input_failed", error.to_string()))
        })();
        if let Err(error) = transaction {
            return Err(recover(holder, error));
        }
        let state = holder.restore()?;
        let receipt = release_receipt(&state);
        emit(json!({"outcome":"restored_and_released", "state":state})).map_err(|mut error| {
            // Output failure happens after recovery completed. Retain that
            // receipt and do not issue a second restoration transaction.
            error.details.push(receipt.to_string());
            error
        })
    }

    pub(super) fn run(
        path: &std::path::Path,
        minimum_duty_policy: MinimumDutyPolicy,
        output: &mut impl Write,
    ) -> Result<(), ControlError> {
        minimum_duty_policy.validate()?;
        if !io::stdin().is_terminal() {
            return Err(ControlError::new(
                "fan_hold_requires_terminal",
                "fan hold requires an interactive terminal; no device was opened",
            ));
        }
        let curve = load_curve_file(path, minimum_duty_policy)?;
        let mut session = FanCurveSession::open().map_err(session_error)?;
        hold(
            &mut session,
            &curve,
            minimum_duty_policy,
            || {
                let mut prompt = io::stderr().lock();
                writeln!(prompt, "曲线正在由本进程持有；按 Enter 恢复旧表并退出。强制结束后的内核恢复结果无法由本进程确认。")?;
                prompt.flush()?;
                wait_for_enter(io::stdin().lock())
            },
            |value| {
                write_json(output, &value)?;
                output
                    .flush()
                    .map_err(|error| ControlError::new("output_failed", error.to_string()))
            },
        )
    }

    pub(super) fn wait_for_enter(mut input: impl Read) -> io::Result<()> {
        let mut byte = [0];
        loop {
            match input.read(&mut byte) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Terminal input ended before Enter",
                    ))
                }
                Ok(_) if byte[0] == b'\n' => return Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}
fn epoch_json(time: SystemTime) -> Value {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            json!({"unix_seconds":duration.as_secs(),"nanoseconds":duration.subsec_nanos()})
        }
        Err(_) => Value::Null,
    }
}
#[cfg(any(target_os = "linux", test))]
fn read_kernel_release(reader: impl Read) -> Value {
    const MAX_BYTES: usize = 256;
    let source = "/proc/sys/kernel/osrelease";
    let mut bytes = Vec::new();
    if let Err(error) = reader.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes) {
        return json!({"value":null,"source":source,"status":"read_failed","reason":error.to_string()});
    }
    if bytes.len() > MAX_BYTES {
        return json!({"value":null,"source":source,"status":"invalid_data","reason":"Kernel release exceeds the 256-byte limit"});
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return json!({"value":null,"source":source,"status":"invalid_data","reason":"Kernel release is not UTF-8"});
    };
    let release = text.trim();
    if release.is_empty() || !release.bytes().all(|byte| byte.is_ascii_graphic()) {
        return json!({"value":null,"source":source,"status":"invalid_data","reason":"Kernel release is empty or contains non-printable characters"});
    }
    json!({"value":release,"source":source,"status":"available"})
}
fn runtime_metadata() -> Value {
    #[cfg(target_os = "linux")]
    let kernel_release = match std::fs::File::open("/proc/sys/kernel/osrelease") {
        Ok(file) => read_kernel_release(file),
        Err(error) => {
            json!({"value":null,"source":"/proc/sys/kernel/osrelease","status":"read_failed","reason":error.to_string()})
        }
    };
    #[cfg(not(target_os = "linux"))]
    let kernel_release = json!({"value":null,"source":null,"status":"not_collected","reason":"Runtime kernel release collection is implemented only for Linux"});
    json!({"kernel_release":kernel_release})
}
fn collect_status_json(
    snapshot_provider: impl FnOnce() -> Snapshot,
    mut wall_clock: impl FnMut() -> SystemTime,
    mut monotonic_clock: impl FnMut() -> Instant,
    runtime: Value,
) -> Result<Value, ControlError> {
    let monotonic_start = monotonic_clock();
    let started_at = wall_clock();
    let snapshot = snapshot_provider();
    let finished_at = wall_clock();
    let monotonic_finish = monotonic_clock();
    let elapsed = monotonic_finish.checked_duration_since(monotonic_start);
    let wall_clock_status = if started_at.duration_since(UNIX_EPOCH).is_err()
        || finished_at.duration_since(UNIX_EPOCH).is_err()
    {
        "before_unix_epoch"
    } else if finished_at < started_at {
        "moved_backwards"
    } else {
        "ordered"
    };
    let identification = registry::identify(&snapshot.identity);
    let mut value = serde_json::to_value(&snapshot)
        .map_err(|e| ControlError::new("serialization_failed", e.to_string()))?;
    value["identity_evidence"] = serde_json::to_value(identification)
        .map_err(|e| ControlError::new("serialization_failed", e.to_string()))?;
    value["schema_version"] = 1.into();
    value["collection"] = json!({
        "started_at":epoch_json(started_at),
        "finished_at":epoch_json(finished_at),
        "wall_clock_status":wall_clock_status,
        "monotonic_elapsed":elapsed.map(duration_json),
        "monotonic_clock_status":if elapsed.is_some(){"ordered"}else{"moved_backwards"},
        "time_basis":"UTC Unix epoch; seconds and nanoseconds are exact integer components. A timestamp before the epoch is null. Wall-clock ordering does not prove clock synchronization.",
        "consistency":"non_atomic_sequential_reads",
        "scope":"One complete Service::snapshot() call, including brightness enumeration and OEM controls. Start and finish wall-clock reads surround that call; monotonic elapsed also includes those clock reads. Excludes runtime metadata collection, model matching and JSON serialization. Values may come from different instants or cached OS readings; this is not a simultaneous hardware sample.",
        "tool":{"name":"mechrevo-console","package":env!("CARGO_PKG_NAME"),"version":env!("CARGO_PKG_VERSION")},
        "build_target":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},
        "runtime":runtime,
    });
    Ok(value)
}
fn status_json(service: &Service) -> Result<Value, ControlError> {
    let runtime = runtime_metadata();
    collect_status_json(
        || service.snapshot(),
        SystemTime::now,
        Instant::now,
        runtime,
    )
}
fn write_json(writer: &mut impl Write, value: &Value) -> Result<(), ControlError> {
    serde_json::to_writer_pretty(&mut *writer, value)
        .map_err(|e| ControlError::new("output_failed", e.to_string()))?;
    writeln!(writer).map_err(|e| ControlError::new("output_failed", e.to_string()))
}

/// Each compact JSON line is a separate observation. A failed sample produces
/// an error frame, stops collection and never carries forward an old reading.
fn stream_thermal(
    writer: &mut impl Write,
    samples: u16,
    interval_ms: u32,
    mut collect: impl FnMut() -> Result<ThermalSample, ControlError>,
    mut sleep: impl FnMut(Duration),
    mut wall_clock: impl FnMut() -> SystemTime,
) -> Result<(), ControlError> {
    if !(1..=3600).contains(&samples) || !(250..=60_000).contains(&interval_ms) {
        return Err(usage(
            "Thermal sample count or interval is outside its supported range",
        ));
    }
    for sequence in 1..=samples {
        let started_at = wall_clock();
        let observed = collect();
        let finished_at = wall_clock();
        let wall_clock_status = match (
            started_at.duration_since(UNIX_EPOCH),
            finished_at.duration_since(UNIX_EPOCH),
        ) {
            (Ok(start), Ok(finish)) if finish >= start => "ordered",
            (Ok(_), Ok(_)) => "moved_backwards",
            _ => "before_unix_epoch",
        };
        let mut frame = json!({
            "schema_version": 1,
            "type": "thermal_sample",
            "sequence": sequence,
            "requested_samples": samples,
            "interval_ms": interval_ms,
            "interval_policy": "Wait after each completed sample; no catch-up bursts",
            "platform": std::env::consts::OS,
            "started_at": epoch_json(started_at),
            "finished_at": epoch_json(finished_at),
            "wall_clock_status": wall_clock_status,
            "scope": "Linux hwmon temperature and fan RPM only. Sequential read window, possibly cached source values; no OEM, brightness, power-policy or catalog collection. No fan lease acquisition or authentication.",
            "readings": [],
            "diagnostics": [],
            "monotonic_elapsed": null,
        });
        let error = match observed {
            Ok(sample) => {
                let elapsed = sample.finished_at.checked_duration_since(sample.started_at);
                if let Some(elapsed) = elapsed {
                    frame["status"] = "sampled".into();
                    frame["monotonic_elapsed"] = duration_json(elapsed);
                    frame["readings"] = json!(sample.readings);
                    frame["diagnostics"] = json!(sample.diagnostics);
                    None
                } else {
                    Some(ControlError::new(
                        "thermal_sample_time_invalid",
                        "Thermal read window ends before it starts",
                    ))
                }
            }
            Err(error) => Some(error),
        };
        if let Some(error) = &error {
            frame["status"] = "error".into();
            frame["error"] = json!(error);
        }
        let write = serde_json::to_writer(&mut *writer, &frame)
            .map_err(io::Error::other)
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush());
        if let Err(output_error) = write {
            let mut failure = ControlError::new("output_failed", output_error.to_string());
            failure.details.push(format!(
                "Collection stopped during output of sample {sequence}; that line may be partial"
            ));
            if let Some(collection_error) = &error {
                failure.details.push(format!(
                    "thermal_collection_error: {}",
                    json!(collection_error)
                ));
            }
            return Err(failure);
        }
        if let Some(mut error) = error {
            error.details.push(format!(
                "Collection stopped at failed sample {sequence}; no old readings were reused"
            ));
            return Err(error);
        }
        if sequence < samples {
            sleep(Duration::from_millis(u64::from(interval_ms)));
        }
    }
    Ok(())
}
// @mark mechrevo.console_cli role=implementation
fn export(path: &std::path::Path, value: &Value) -> Result<(), ControlError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| ControlError::new("export_failed", format!("{}: {e}", path.display())))?;
    // create_new is race-safe: symlinks and existing recovery material are refused.
    write_json(&mut file, value)?;
    file.sync_all()
        .map_err(|e| ControlError::new("export_failed", format!("{}: {e}", path.display())))
}
fn run(action: Action, service: &Service) -> Result<(), ControlError> {
    let mut stdout = io::stdout().lock();
    let output = match action {
        Action::Help => {
            write!(stdout, "{HELP}{CPU_POWER_HELP}{}", windows_fn_cli::HELP)
                .map_err(|e| ControlError::new("output_failed", e.to_string()))?;
            return Ok(());
        }
        Action::Version => json!({"version":env!("CARGO_PKG_VERSION")}),
        Action::Status(true) => status_json(service)?,
        Action::Thermal {
            samples,
            interval_ms,
        } => {
            return stream_thermal(
                &mut stdout,
                samples,
                interval_ms,
                || service.sample_thermal(),
                std::thread::sleep,
                SystemTime::now,
            );
        }
        Action::Status(false) => {
            let s = service.snapshot();
            writeln!(stdout,"Platform: {}\nManufacturer: {}\nProduct: {}\nBoard: {}\nBIOS: {}\nSupport: {}\nPower policy: {}\nCurrent: {}",s.platform,s.identity.manufacturer.as_deref().unwrap_or("unknown"),s.identity.product_name.as_deref().unwrap_or("unknown"),s.identity.board_name.as_deref().unwrap_or("unknown"),s.identity.bios_version.as_deref().unwrap_or("unknown"),registry::identify(&s.identity).reason,s.power_policy.scope,s.power_policy.current.as_deref().unwrap_or("unknown")).map_err(|e|ControlError::new("output_failed",e.to_string()))?;
            for r in s.telemetry {
                writeln!(
                    stdout,
                    "{}: {} {} [device={} source={}]",
                    r.label,
                    r.value.map(|v| v.to_string()).unwrap_or("unknown".into()),
                    r.unit,
                    r.device_id,
                    r.source.path
                )
                .map_err(|e| ControlError::new("output_failed", e.to_string()))?;
            }
            for control in s.oem_controls {
                let current = control
                    .current
                    .as_ref()
                    .map(|value| json!(value).to_string())
                    .unwrap_or_else(|| "unknown".into());
                writeln!(
                    stdout,
                    "OEM {} ({}): {} [{:?}]",
                    control.id, control.label, current, control.status
                )
                .map_err(|error| ControlError::new("output_failed", error.to_string()))?;
            }
            for d in s.diagnostics {
                writeln!(stdout, "{}: {}", d.code, d.message)
                    .map_err(|e| ControlError::new("output_failed", e.to_string()))?;
            }
            return Ok(());
        }
        Action::Capabilities => json!(service.capabilities()),
        Action::Models(query) => json!(service.catalog(&query)),
        Action::OemList => json!(service.oem_controls()),
        Action::OemSet(id, value, dry_run) => set_oem(service, &id, &value, dry_run)?,
        Action::WindowsFn(command) => return windows_fn_cli::run(command, &mut stdout),
        Action::FanValidate(path, policy) => {
            curve_validation_json(&load_curve_file(&path, policy)?, policy)?
        }
        Action::CpuPower(command) => return cpu_power_cli::run(command, &mut stdout),
        Action::FanHold(path, policy) => {
            #[cfg(target_os = "linux")]
            return fan_hold::run(&path, policy, &mut stdout);
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (path, policy);
                return Err(ControlError::new(
                    "fan_hold_unsupported",
                    "The exclusive fan-curve driver session is available on Linux only",
                ));
            }
        }
        Action::Profile(profile, dry, expected) => {
            let plan = service.plan_profile(profile)?;
            let output = profile_output(dry, &plan, expected.as_deref(), || {
                service.apply_profile(&plan)
            })?;
            return write_profile_output(&output, &mut stdout);
        }
        Action::Brightness(percent, device, dry, expected_current) => {
            return brightness_cli::run(
                percent,
                device.as_deref(),
                dry,
                expected_current,
                service,
                &mut stdout,
            )
        }
        Action::Export(path) => {
            let value = status_json(service)?;
            export(&path, &value)?;
            json!({"exported":path,"encoding":"UTF-8","schema_version":1})
        }
    };
    write_json(&mut stdout, &output)
}
fn main() {
    let result = parse(std::env::args_os().skip(1).collect())
        .and_then(|action| run(action, &Service::new()));
    if let Err(error) = result {
        let _ = write_json(
            &mut io::stderr().lock(),
            &windows_fn_cli::error_document(&error, brightness_cli::error_document(&error)),
        );
        std::process::exit(2)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use mechrevo_console_core::{BrightnessDevice, CapabilityStatus, EvidenceSource, Identity};
    use std::cell::RefCell;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn fan_validate_requires_one_explicit_input_and_never_accepts_apply_flags() {
        assert_eq!(
            parse(args(&["fan", "validate", "--input", "曲线 file.json"])).unwrap(),
            Action::FanValidate("曲线 file.json".into(), MinimumDutyPolicy::default())
        );
        assert_eq!(
            parse(args(&["fan", "hold", "--input", "curve.json"])).unwrap(),
            Action::FanHold("curve.json".into(), MinimumDutyPolicy::default())
        );
        for args_values in [
            vec!["fan"],
            vec!["fan", "validate"],
            vec!["fan", "validate", "file.json"],
            vec!["fan", "validate", "--input", ""],
            vec!["fan", "validate", "--input", "file.json", "--apply"],
            vec!["fan", "apply", "--input", "file.json"],
        ] {
            assert!(parse(args(&args_values)).is_err());
        }
    }

    #[test]
    fn fan_minimum_options_accept_both_commands_and_whole_percent_boundaries() {
        for command in ["validate", "hold"] {
            let action = parse(args(&[
                "fan",
                command,
                "--gpu-min-percent",
                "100",
                "--raise-stopped-stages",
                "--input",
                "curve.json",
                "--cpu-min-percent",
                "1",
            ]))
            .unwrap();
            let policy = MinimumDutyPolicy {
                cpu_min_percent: Some(1),
                gpu_min_percent: Some(100),
                raise_stopped_stages: true,
            };
            assert_eq!(
                action,
                if command == "validate" {
                    Action::FanValidate("curve.json".into(), policy)
                } else {
                    Action::FanHold("curve.json".into(), policy)
                }
            );
        }
        assert_eq!(
            parse(args(&[
                "fan",
                "validate",
                "--input",
                "curve.json",
                "--cpu-min-percent",
                "030",
            ]))
            .unwrap(),
            Action::FanValidate(
                "curve.json".into(),
                MinimumDutyPolicy {
                    cpu_min_percent: Some(30),
                    ..MinimumDutyPolicy::default()
                }
            )
        );
    }

    #[test]
    fn fan_minimum_options_reject_malformed_or_repeated_arguments_before_io() {
        // The path intentionally cannot be opened: these must fail in parse,
        // before loading a file, checking a terminal or opening the driver.
        for command in ["validate", "hold"] {
            for option in ["--cpu-min-percent", "--gpu-min-percent"] {
                for value in [
                    "",
                    "0",
                    "101",
                    "256",
                    "+30",
                    "-1",
                    "30.0",
                    "30%",
                    " 30",
                    "30 ",
                    "３０",
                    "999999999999999999999999999999999999",
                    "--raise-stopped-stages",
                ] {
                    assert!(
                        parse(args(&[
                            "fan",
                            command,
                            "--input",
                            "/missing/curve.json",
                            option,
                            value,
                        ]))
                        .is_err(),
                        "accepted {command} {option} {value:?}"
                    );
                }
                assert!(parse(args(&[
                    "fan",
                    command,
                    "--input",
                    "/missing/curve.json",
                    option
                ]))
                .is_err());
                assert!(parse(args(&[
                    "fan",
                    command,
                    "--input",
                    "/missing/curve.json",
                    option,
                    "30",
                    option,
                    "40",
                ]))
                .is_err());
            }
            for suffix in [
                vec!["--input", "/missing/curve.json", "--input", "other.json"],
                vec!["--input", "--cpu-min-percent", "30"],
                vec!["--input", "/missing/curve.json", "--raise-stopped-stages"],
                vec![
                    "--input",
                    "/missing/curve.json",
                    "--cpu-min-percent",
                    "30",
                    "--raise-stopped-stages",
                    "--raise-stopped-stages",
                ],
                vec![
                    "--input",
                    "/missing/curve.json",
                    "--cpu-min-percent",
                    "30",
                    "--apply",
                ],
                vec![
                    "--input",
                    "/missing/curve.json",
                    "--gpu-min-percent",
                    "30",
                    "extra.json",
                ],
                vec!["--cpu-min-percent", "30"],
                vec!["--raise-stopped-stages", "--gpu-min-percent", "30"],
            ] {
                let mut values = vec!["fan", command];
                values.extend(suffix);
                assert!(parse(args(&values)).is_err(), "accepted {values:?}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn fan_minimum_options_reject_non_unicode_values_but_preserve_path_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(b"curve-\xff.json".to_vec());
        assert_eq!(
            parse(vec![
                "fan".into(),
                "validate".into(),
                "--input".into(),
                path.clone()
            ])
            .unwrap(),
            Action::FanValidate(PathBuf::from(path), MinimumDutyPolicy::default())
        );
        for option in ["--cpu-min-percent", "--gpu-min-percent"] {
            let mut values = args(&["fan", "hold", "--input", "/missing/curve.json", option]);
            values.push(OsString::from_vec(vec![0xff]));
            assert_eq!(parse(values).unwrap_err().code, "invalid_argument");
        }
    }

    #[test]
    fn fan_curve_loader_rejects_invalid_policy_before_reading_a_path() {
        for policy in [
            MinimumDutyPolicy {
                cpu_min_percent: Some(0),
                ..MinimumDutyPolicy::default()
            },
            MinimumDutyPolicy {
                gpu_min_percent: Some(101),
                ..MinimumDutyPolicy::default()
            },
            MinimumDutyPolicy {
                raise_stopped_stages: true,
                ..MinimumDutyPolicy::default()
            },
        ] {
            let expected = policy.validate().unwrap_err().code;
            assert_eq!(
                load_curve_file(std::path::Path::new(""), policy)
                    .unwrap_err()
                    .code,
                expected
            );
        }
    }

    #[test]
    fn fan_validate_uses_complete_typed_input_and_does_not_claim_hardware_application() {
        let document = json!({
            "schema_version": 1, "layout":"uniwill_ram_fan1p5", "respective":true,
            "cpu":{"rise_celsius":vec![40;15],"fall_celsius":vec![50;15],"duty_raw":vec![51;16]},
            "gpu":{"rise_celsius":vec![255;15],"fall_celsius":vec![255;15],"duty_raw":vec![200;16]}
        });
        let curve =
            ValidatedFanCurve::from_reader(serde_json::to_vec(&document).unwrap().as_slice())
                .unwrap();
        let output = curve_validation_json(&curve, MinimumDutyPolicy::default()).unwrap();
        assert_eq!(output["outcome"], "protocol_validated");
        assert_eq!(output["applied"], false);
        assert_eq!(output["hardware_qualified"], false);
        assert_eq!(output["curve"], document);
        assert_eq!(
            output["minimum_duty_policy"],
            json!(MinimumDutyPolicy::default())
        );
        assert_eq!(output["table_write_bytes"], 94);
        assert!(ValidatedFanCurve::from_reader(b"{}".as_slice()).is_err());
    }

    #[test]
    fn fan_validation_outputs_the_final_curve_and_explicit_minimum_policy() {
        let mut document = json!({
            "schema_version": 1, "layout":"uniwill_ram_fan1p5", "respective":false,
            "cpu":{"rise_celsius":vec![40;15],"fall_celsius":vec![50;15],"duty_raw":vec![51;16]},
            "gpu":{"rise_celsius":vec![255;15],"fall_celsius":vec![255;15],"duty_raw":vec![61;16]}
        });
        document["cpu"]["duty_raw"][0] = json!(0);
        document["cpu"]["duty_raw"][1] = json!(61);
        document["gpu"]["duty_raw"][0] = json!(0);
        let curve = ValidatedFanCurve::from_json(&serde_json::to_vec(&document).unwrap()).unwrap();
        for raise_stopped_stages in [false, true] {
            let policy = MinimumDutyPolicy {
                cpu_min_percent: Some(30),
                gpu_min_percent: None,
                raise_stopped_stages,
            };
            let output = curve_validation_json(&curve, policy).unwrap();
            let mut expected = document.clone();
            expected["cpu"]["duty_raw"] = json!(vec![60; 16]);
            expected["cpu"]["duty_raw"][0] = json!(if raise_stopped_stages { 60 } else { 0 });
            expected["cpu"]["duty_raw"][1] = json!(61);
            assert_eq!(output["curve"], expected);
            assert_eq!(output["minimum_duty_policy"], json!(policy));
            assert_eq!(output["applied"], false);
            assert_eq!(output["hardware_qualified"], false);
        }
        assert_eq!(json!(curve.curve()), document);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_fan_applies_the_transformed_curve_and_rejects_policy_before_acquire() {
        #[derive(Default)]
        struct HolderSpy {
            events: Vec<&'static str>,
            applied: Option<ValidatedFanCurve>,
        }
        impl fan_hold::Holder for HolderSpy {
            fn acquire(&mut self) -> Result<Value, ControlError> {
                self.events.push("acquire");
                Ok(json!({"owner":true}))
            }
            fn apply(&mut self, curve: &ValidatedFanCurve) -> Result<Value, ControlError> {
                self.events.push("apply");
                self.applied = Some(curve.clone());
                Ok(json!({"owner":true,"applied":true}))
            }
            fn restore(&mut self) -> Result<Value, ControlError> {
                self.events.push("restore");
                Ok(json!({"owner":false,"restored_verified":true}))
            }
        }
        let mut document = json!({
            "schema_version":1,"layout":"uniwill_ram_fan1p5","respective":true,
            "cpu":{"rise_celsius":vec![40;15],"fall_celsius":vec![50;15],"duty_raw":vec![51;16]},
            "gpu":{"rise_celsius":vec![255;15],"fall_celsius":vec![255;15],"duty_raw":vec![61;16]}
        });
        document["cpu"]["duty_raw"][0] = json!(0);
        document["gpu"]["duty_raw"][0] = json!(0);
        let curve = ValidatedFanCurve::from_json(&serde_json::to_vec(&document).unwrap()).unwrap();
        for policy in [
            MinimumDutyPolicy::default(),
            MinimumDutyPolicy {
                cpu_min_percent: Some(30),
                gpu_min_percent: Some(50),
                raise_stopped_stages: false,
            },
            MinimumDutyPolicy {
                cpu_min_percent: Some(30),
                gpu_min_percent: Some(50),
                raise_stopped_stages: true,
            },
            MinimumDutyPolicy {
                cpu_min_percent: None,
                gpu_min_percent: Some(100),
                raise_stopped_stages: true,
            },
        ] {
            let mut holder = HolderSpy::default();
            let mut outputs = Vec::new();
            fan_hold::hold(
                &mut holder,
                &curve,
                policy,
                || Ok(()),
                |output| {
                    outputs.push(output);
                    Ok(())
                },
            )
            .unwrap();
            let mut expected = document.clone();
            for (name, floor, original) in [
                ("cpu", policy.cpu_min_percent, 51),
                ("gpu", policy.gpu_min_percent, 61),
            ] {
                let raw = floor.map(|value| u16::from(value) * 2).unwrap_or(0);
                expected[name]["duty_raw"] = json!(vec![raw.max(original); 16]);
                expected[name]["duty_raw"][0] =
                    json!(if floor.is_some() && policy.raise_stopped_stages {
                        raw
                    } else {
                        0
                    });
            }
            let applied = holder.applied.as_ref().unwrap();
            assert_eq!(json!(applied.curve()), expected);
            assert_eq!(holder.events, ["acquire", "apply", "restore"]);
            assert_eq!(outputs[0]["curve"], expected);
            assert_eq!(outputs[0]["minimum_duty_policy"], json!(policy));
            assert_eq!(outputs[1]["outcome"], "restored_and_released");
            assert_eq!(json!(curve.curve()), document);
        }
        for policy in [
            MinimumDutyPolicy {
                cpu_min_percent: Some(0),
                ..MinimumDutyPolicy::default()
            },
            MinimumDutyPolicy {
                raise_stopped_stages: true,
                ..MinimumDutyPolicy::default()
            },
        ] {
            let mut holder = HolderSpy::default();
            let error = fan_hold::hold(
                &mut holder,
                &curve,
                policy,
                || panic!("invalid policy reached wait"),
                |_| panic!("invalid policy emitted transaction output"),
            )
            .unwrap_err();
            assert_eq!(error.code, policy.validate().unwrap_err().code);
            assert!(holder.events.is_empty());
            assert!(holder.applied.is_none());
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn fan_hold_unsupported_platform_does_not_load_the_curve() {
        let policy = MinimumDutyPolicy {
            cpu_min_percent: Some(30),
            ..MinimumDutyPolicy::default()
        };
        let error = run(
            Action::FanHold("/missing/curve.json".into(), policy),
            &Service::new(),
        )
        .unwrap_err();
        assert_eq!(error.code, "fan_hold_unsupported");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_fan_lifetime_restores_on_every_exit_path_and_preserves_primary_errors() {
        use std::{cell::RefCell, rc::Rc};
        struct MockHolder {
            events: Rc<RefCell<Vec<String>>>,
            failure: &'static str,
            restore_fails: bool,
        }
        impl fan_hold::Holder for MockHolder {
            fn acquire(&mut self) -> Result<Value, ControlError> {
                self.events.borrow_mut().push("acquire".into());
                if self.failure == "acquire" {
                    Err(ControlError::new("acquire_failed", "test"))
                } else {
                    Ok(json!({"owner":true}))
                }
            }
            fn apply(&mut self, _: &ValidatedFanCurve) -> Result<Value, ControlError> {
                self.events.borrow_mut().push("apply".into());
                if self.failure == "apply" {
                    Err(ControlError::new("apply_failed", "test"))
                } else {
                    Ok(json!({"owner":true,"applied":true}))
                }
            }
            fn restore(&mut self) -> Result<Value, ControlError> {
                self.events.borrow_mut().push("restore".into());
                if self.restore_fails {
                    Err(ControlError::new("restore_failed", "test"))
                } else {
                    Ok(
                        json!({"owner":false,"restored_verified":!["acquire","apply"].contains(&self.failure)}),
                    )
                }
            }
        }
        let document = json!({
            "schema_version":1,"layout":"uniwill_ram_fan1p5","respective":true,
            "cpu":{"rise_celsius":vec![40;15],"fall_celsius":vec![50;15],"duty_raw":vec![51;16]},
            "gpu":{"rise_celsius":vec![255;15],"fall_celsius":vec![255;15],"duty_raw":vec![200;16]}
        });
        let curve = ValidatedFanCurve::from_json(&serde_json::to_vec(&document).unwrap()).unwrap();
        for failure in [
            "",
            "acquire",
            "apply",
            "emit_holding",
            "wait",
            "emit_released",
        ] {
            for restore_fails in [false, true] {
                let events = Rc::new(RefCell::new(Vec::new()));
                let mut holder = MockHolder {
                    events: events.clone(),
                    failure,
                    restore_fails,
                };
                let result = fan_hold::hold(
                    &mut holder,
                    &curve,
                    MinimumDutyPolicy::default(),
                    || {
                        events.borrow_mut().push("wait".into());
                        if failure == "wait" {
                            Err(io::Error::other("input failed"))
                        } else {
                            Ok(())
                        }
                    },
                    |output| {
                        let event = match output["outcome"].as_str().unwrap() {
                            "holding" => "emit_holding",
                            "restored_and_released" => "emit_released",
                            other => panic!("Unexpected output {other}"),
                        };
                        events.borrow_mut().push(event.into());
                        if failure == event {
                            Err(ControlError::new(event, "test"))
                        } else {
                            Ok(())
                        }
                    },
                );
                let events = events.borrow();
                assert_eq!(events.iter().filter(|value| *value == "restore").count(), 1);
                if failure.is_empty() && !restore_fails {
                    assert!(result.is_ok());
                    assert_eq!(
                        *events,
                        [
                            "acquire",
                            "apply",
                            "emit_holding",
                            "wait",
                            "restore",
                            "emit_released"
                        ]
                    );
                } else {
                    let error = result.unwrap_err();
                    let expected = match failure {
                        "acquire" => "acquire_failed",
                        "apply" => "apply_failed",
                        "emit_holding" => "emit_holding",
                        "wait" => "fan_hold_input_failed",
                        _ if restore_fails => "restore_failed",
                        "emit_released" => "emit_released",
                        _ => unreachable!(),
                    };
                    assert_eq!(error.code, expected);
                    if ["acquire", "apply", "emit_holding", "wait"].contains(&failure) {
                        let recovery: Value =
                            serde_json::from_str(error.details.last().unwrap()).unwrap();
                        assert_eq!(
                            recovery["restoration"],
                            if restore_fails {
                                "unconfirmed"
                            } else if ["acquire", "apply"].contains(&failure) {
                                "released_without_restoration_required"
                            } else {
                                "driver_verified"
                            }
                        );
                    }
                    if failure == "emit_released" && !restore_fails {
                        let receipt: Value =
                            serde_json::from_str(error.details.last().unwrap()).unwrap();
                        assert_eq!(receipt["restoration"], "driver_verified");
                        assert_eq!(receipt["state"]["owner"], false);
                    }
                    if restore_fails || failure != "emit_released" {
                        assert!(!events.iter().any(|event| event == "emit_released"));
                    }
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_fan_input_waits_for_enter_without_accumulating_other_keys() {
        let mut input = b"abc\nremaining".as_slice();
        fan_hold::wait_for_enter(&mut input).unwrap();
        assert_eq!(input, b"remaining");
        assert_eq!(
            fan_hold::wait_for_enter(b"abc".as_slice())
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        struct Interrupted {
            first: bool,
        }
        impl Read for Interrupted {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.first {
                    self.first = false;
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    buffer[0] = b'\n';
                    Ok(1)
                }
            }
        }
        fan_hold::wait_for_enter(Interrupted { first: true }).unwrap();
    }
    #[test]
    fn oem_commands_require_explicit_control_value_and_known_options() {
        assert_eq!(parse(args(&["oem", "list"])).unwrap(), Action::OemList);
        assert_eq!(
            parse(args(&["oem", "set", "fn_lock", "on"])).unwrap(),
            Action::OemSet("fn_lock".into(), "on".into(), false)
        );
        assert_eq!(
            parse(args(&[
                "oem",
                "set",
                "keyboard_color",
                "#ab12CD",
                "--dry-run"
            ]))
            .unwrap(),
            Action::OemSet("keyboard_color".into(), "#ab12CD".into(), true)
        );
        for values in [
            vec!["oem"],
            vec!["oem", "list", "--dry-run"],
            vec!["oem", "set"],
            vec!["oem", "set", "fn_lock"],
            vec!["oem", "set", "", "on"],
            vec!["oem", "set", "fn_lock", ""],
            vec!["oem", "set", "fn_lock", "--dry-run"],
            vec!["oem", "set", "--dry-run", "on"],
            vec!["oem", "set", "fn_lock", "on", "--force"],
            vec!["oem", "set", "fn_lock", "on", "--dry-run", "--dry-run"],
            vec!["oem", "apply", "fn_lock", "on"],
        ] {
            assert!(parse(args(&values)).is_err(), "Accepted {values:?}");
        }
    }
    #[test]
    fn oem_boolean_and_choice_values_follow_the_enumerated_domain() {
        for kind in [
            OemControlKind::FnLock,
            OemControlKind::AcAutoBoot,
            OemControlKind::UsbPowerShare,
        ] {
            assert_eq!(
                json!(parse_oem_value(&kind, &OemDomain::Boolean, "on").unwrap()),
                json!({"type":"boolean","value":true})
            );
            assert_eq!(
                json!(parse_oem_value(&kind, &OemDomain::Boolean, "off").unwrap()),
                json!({"type":"boolean","value":false})
            );
            for raw in ["1", "0", "true", "ON", "on ", "$(touch /tmp/invalid)"] {
                assert!(parse_oem_value(&kind, &OemDomain::Boolean, raw).is_err());
            }
        }
        let choices = OemDomain::Choices(vec!["high_capacity".into(), "balanced".into()]);
        assert_eq!(
            json!(parse_oem_value(&OemControlKind::ChargingProfile, &choices, "balanced").unwrap()),
            json!({"type":"choice","value":"balanced"})
        );
        for raw in [
            "80",
            "stationary",
            "BALANCED",
            "balanced\n",
            "../balanced",
            "balanced; exit",
        ] {
            assert!(parse_oem_value(&OemControlKind::ChargingProfile, &choices, raw).is_err());
        }
        assert!(parse_oem_value(
            &OemControlKind::ChargingProfile,
            &OemDomain::Choices(Vec::new()),
            "balanced"
        )
        .is_err());
        assert!(parse_oem_value(&OemControlKind::FnLock, &choices, "balanced").is_err());
        assert!(
            parse_oem_value(&OemControlKind::ChargingProfile, &OemDomain::Boolean, "on").is_err()
        );
    }
    #[test]
    fn oem_brightness_uses_raw_levels_and_actual_maximum() {
        let kind = OemControlKind::KeyboardBrightness;
        let domain = OemDomain::Level { max: 4 };
        for (raw, expected) in [("0", 0), ("4", 4), ("001", 1)] {
            assert_eq!(
                json!(parse_oem_value(&kind, &domain, raw).unwrap()),
                json!({"type":"level","value":expected})
            );
        }
        for raw in [
            "5",
            "100",
            "-1",
            "+1",
            "1.0",
            "nan",
            " 1",
            "1 ",
            "１",
            "",
            "4294967296",
        ] {
            assert!(
                parse_oem_value(&kind, &domain, raw).is_err(),
                "Accepted {raw:?}"
            );
        }
        assert_eq!(
            json!(
                parse_oem_value(&kind, &OemDomain::Level { max: u32::MAX }, "4294967295").unwrap()
            ),
            json!({"type":"level","value":u32::MAX})
        );
        assert!(parse_oem_value(&kind, &OemDomain::Boolean, "on").is_err());
    }
    #[test]
    fn oem_colors_require_exact_rgb_hex_and_preserve_channel_order() {
        for (raw, expected) in [
            ("#000000", [0, 0, 0]),
            ("#FFFFFF", [255, 255, 255]),
            ("#ab12CD", [171, 18, 205]),
        ] {
            assert_eq!(
                json!(
                    parse_oem_value(&OemControlKind::KeyboardColor, &OemDomain::Rgb, raw).unwrap()
                ),
                json!({"type":"rgb","value":expected})
            );
        }
        for raw in [
            "000000",
            "#fff",
            "#ffffff00",
            "#gg0000",
            "#12 456",
            "#12é45",
            "#12345\n",
            "#123456;exit",
            "#12345",
            "#123456 ",
        ] {
            assert!(
                parse_oem_value(&OemControlKind::KeyboardColor, &OemDomain::Rgb, raw).is_err(),
                "Accepted {raw:?}"
            );
        }
        assert!(parse_oem_value(
            &OemControlKind::KeyboardColor,
            &OemDomain::Level { max: 255 },
            "#123456"
        )
        .is_err());
    }
    #[test]
    fn oem_dry_run_returns_plan_without_invoking_write_or_privilege_backend() {
        let plan = json!({"control_id":"fn_lock","target":{"type":"boolean","value":true},"requires_privilege":true});
        let value = oem_set_output(true, plan.clone(), || {
            panic!("Dry-run invoked the write/privilege backend")
        })
        .unwrap();
        assert_eq!(value, json!({"dry_run":true,"plan":plan}));
        let writes = RefCell::new(0);
        let applied = json!({"control_id":"fn_lock","confirmed":{"type":"boolean","value":true}});
        let value = oem_set_output(false, plan.clone(), || {
            *writes.borrow_mut() += 1;
            Ok(applied.clone())
        })
        .unwrap();
        assert_eq!(*writes.borrow(), 1);
        assert_eq!(value, applied);
        let error = oem_set_output(false, plan, || {
            Err(ControlError::new("fixture_write_failed", "fixture failure"))
        })
        .unwrap_err();
        assert_eq!(error.code, "fixture_write_failed");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn qualified_fake_oem_control_dry_run_status_export_and_selection() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::var_os("MECHREVO_TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "mechrevo-cli-oem-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let module = root.join("module/tuxedo_keyboard");
        let platform = root.join("devices/platform/tuxedo_keyboard");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(module.join("srcversion"), "A1B2C3D4E5F6\n").unwrap();
        let node = platform.join("fn_lock");
        std::fs::write(&node, "0\n").unwrap();
        let service = Service::with_linux_sysfs(&root);
        let controls = service.oem_controls();
        let selected = select_oem_control(&controls, "fn-lock").unwrap();
        assert_eq!(selected.current, Some(OemValue::Boolean(false)));
        assert_eq!(selected.kind, OemControlKind::FnLock);
        assert!(!selected.binding.is_empty());
        for id in ["fn", "FN-LOCK", "../fn_lock", "$(touch /tmp/invalid)"] {
            assert_eq!(
                select_oem_control(&controls, id).unwrap_err().code,
                "oem_control_unavailable"
            );
        }
        let duplicate = vec![selected.clone(), selected.clone()];
        assert_eq!(
            select_oem_control(&duplicate, "fn-lock").unwrap_err().code,
            "oem_control_ambiguous"
        );
        let before = std::fs::read(&node).unwrap();
        let before_modified = std::fs::metadata(&node).unwrap().modified().unwrap();
        let dry = set_oem(&service, "fn-lock", "on", true).unwrap();
        assert_eq!(dry["dry_run"], true);
        assert_eq!(dry["plan"]["control_id"], "fn-lock");
        assert_eq!(
            dry["plan"]["previous"],
            json!({"type":"boolean","value":false})
        );
        assert_eq!(
            dry["plan"]["target"],
            json!({"type":"boolean","value":true})
        );
        assert!(dry["plan"]["requires_privilege"].is_boolean());
        assert_eq!(before, std::fs::read(&node).unwrap());
        assert_eq!(
            before_modified,
            std::fs::metadata(&node).unwrap().modified().unwrap()
        );
        assert!(set_oem(&service, "fn-lock", "true", true).is_err());
        assert_eq!(before, std::fs::read(&node).unwrap());
        let instant = Instant::now();
        let document = collect_status_json(
            || {
                let mut snapshot = fixture_snapshot();
                snapshot.oem_controls = controls.clone();
                snapshot
            },
            || UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            || instant,
            json!({}),
        )
        .unwrap();
        assert_eq!(document["oem_controls"], json!(controls));
        let exported_control = document["oem_controls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|control| control["id"] == "fn-lock")
            .unwrap();
        assert_eq!(
            exported_control["current"],
            json!({"type":"boolean","value":false})
        );
        assert!(exported_control["source"]["path"].is_string());
        assert!(exported_control["readback_scope"].is_string());
        let export_path = root.join("oem-status.json");
        export(&export_path, &document).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&export_path).unwrap()).unwrap(),
            document
        );
        assert_eq!(before, std::fs::read(&node).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
    fn fixture_snapshot() -> Snapshot {
        Snapshot {
            platform: "fixture".into(),
            identity: Identity::default(),
            telemetry: Vec::new(),
            capabilities: Vec::new(),
            power_policy: Default::default(),
            diagnostics: Vec::new(),
            oem_controls: Vec::new(),
            brightness: vec![BrightnessDevice {
                id: "fixture-screen".into(),
                label: "屏幕".into(),
                current_percent: Some(42.),
                control_value: Some(42),
                actual_value: None,
                maximum: Some(100),
                source: EvidenceSource {
                    backend: "fixture".into(),
                    path: "/fixture/brightness".into(),
                    device_id: Some("fixture-screen".into()),
                },
                status: CapabilityStatus::Available,
                readable: true,
                writable: false,
                write_backend: None,
                logind_session: None,
                supported_percentages: Vec::new(),
                reason: None,
            }],
        }
    }
    fn fixture_document() -> Value {
        let instant = Instant::now();
        collect_status_json(
            fixture_snapshot,
            || UNIX_EPOCH + Duration::new(1_800_000_000, 123_456_789),
            || instant,
            json!({"kernel_release":{"value":null,"status":"fixture"}}),
        )
        .unwrap()
    }
    #[test]
    fn collection_brackets_one_complete_snapshot_and_preserves_original_fields() {
        let events = RefCell::new(Vec::new());
        let instant = Instant::now();
        let mut ticks = [instant, instant + Duration::new(2, 345)].into_iter();
        let mut walls = [
            UNIX_EPOCH + Duration::new(1_800_000_000, 999_999_999),
            UNIX_EPOCH + Duration::new(1_800_000_003, 123),
        ]
        .into_iter();
        let value = collect_status_json(
            || {
                events.borrow_mut().push("snapshot");
                fixture_snapshot()
            },
            || {
                events.borrow_mut().push("wall");
                walls.next().unwrap()
            },
            || {
                events.borrow_mut().push("monotonic");
                ticks.next().unwrap()
            },
            json!({"kernel_release":{"value":"fixture-release","status":"fixture"}}),
        )
        .unwrap();
        assert_eq!(
            *events.borrow(),
            ["monotonic", "wall", "snapshot", "wall", "monotonic"]
        );
        let original = serde_json::to_value(fixture_snapshot()).unwrap();
        for (key, expected) in original.as_object().unwrap() {
            assert_eq!(&value[key], expected, "Snapshot field changed: {key}");
        }
        assert_eq!(value["schema_version"], 1);
        assert!(value["identity_evidence"].is_object());
        let metadata = &value["collection"];
        assert_eq!(metadata["started_at"]["unix_seconds"], 1_800_000_000);
        assert_eq!(metadata["started_at"]["nanoseconds"], 999_999_999);
        assert_eq!(metadata["finished_at"]["unix_seconds"], 1_800_000_003);
        assert_eq!(metadata["finished_at"]["nanoseconds"], 123);
        assert_eq!(
            metadata["monotonic_elapsed"],
            duration_json(Duration::new(2, 345))
        );
        assert_eq!(metadata["wall_clock_status"], "ordered");
        assert_eq!(metadata["monotonic_clock_status"], "ordered");
        assert_eq!(metadata["consistency"], "non_atomic_sequential_reads");
        assert_eq!(metadata["tool"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(metadata["tool"]["package"], env!("CARGO_PKG_NAME"));
        assert_eq!(metadata["build_target"]["os"], std::env::consts::OS);
        assert_eq!(metadata["build_target"]["arch"], std::env::consts::ARCH);
        assert_eq!(
            metadata["runtime"]["kernel_release"]["value"],
            "fixture-release"
        );
    }
    #[test]
    fn backward_wall_clock_keeps_observed_times_and_monotonic_duration() {
        let instant = Instant::now();
        let mut ticks = [instant, instant + Duration::from_millis(8)].into_iter();
        let mut walls = [
            UNIX_EPOCH + Duration::from_secs(20),
            UNIX_EPOCH + Duration::from_secs(10),
        ]
        .into_iter();
        let value = collect_status_json(
            fixture_snapshot,
            || walls.next().unwrap(),
            || ticks.next().unwrap(),
            json!({}),
        )
        .unwrap();
        assert_eq!(value["collection"]["wall_clock_status"], "moved_backwards");
        assert_eq!(value["collection"]["started_at"]["unix_seconds"], 20);
        assert_eq!(value["collection"]["finished_at"]["unix_seconds"], 10);
        assert_eq!(
            value["collection"]["monotonic_elapsed"],
            duration_json(Duration::from_millis(8))
        );
    }
    #[test]
    fn pre_epoch_and_backward_monotonic_time_are_explicitly_unknown() {
        let instant = Instant::now();
        let mut ticks = [instant + Duration::from_secs(1), instant].into_iter();
        let before_epoch = UNIX_EPOCH.checked_sub(Duration::from_nanos(1)).unwrap();
        let mut walls = [before_epoch, UNIX_EPOCH].into_iter();
        let value = collect_status_json(
            fixture_snapshot,
            || walls.next().unwrap(),
            || ticks.next().unwrap(),
            json!({}),
        )
        .unwrap();
        assert_eq!(
            value["collection"]["wall_clock_status"],
            "before_unix_epoch"
        );
        assert!(value["collection"]["started_at"].is_null());
        assert_eq!(
            value["collection"]["finished_at"],
            json!({"unix_seconds":0,"nanoseconds":0})
        );
        assert_eq!(
            value["collection"]["monotonic_clock_status"],
            "moved_backwards"
        );
        assert!(value["collection"]["monotonic_elapsed"].is_null());
        assert_eq!(
            duration_json(Duration::MAX),
            json!({"seconds":u64::MAX,"nanoseconds":999_999_999})
        );
    }
    #[test]
    fn kernel_release_is_bounded_and_rejects_unavailable_or_invalid_data() {
        let value = read_kernel_release(&b"6.17.1-fixture\n"[..]);
        assert_eq!(value["value"], "6.17.1-fixture");
        assert_eq!(value["source"], "/proc/sys/kernel/osrelease");
        assert_eq!(value["status"], "available");
        for bytes in [b"".as_slice(), b"\n", b"6.17\x00", b"6\n17", b"\xff"] {
            let value = read_kernel_release(bytes);
            assert!(value["value"].is_null());
            assert_eq!(value["status"], "invalid_data");
        }
        let bytes_read = std::rc::Rc::new(std::cell::Cell::new(0));
        struct EndlessReader(std::rc::Rc<std::cell::Cell<usize>>);
        impl Read for EndlessReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                buffer.fill(b'x');
                self.0.set(self.0.get() + buffer.len());
                Ok(buffer.len())
            }
        }
        let value = read_kernel_release(EndlessReader(bytes_read.clone()));
        assert!(value["value"].is_null());
        assert_eq!(value["status"], "invalid_data");
        assert_eq!(bytes_read.get(), 257);
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fixture failure",
                ))
            }
        }
        let value = read_kernel_release(FailedReader);
        assert!(value["value"].is_null());
        assert_eq!(value["status"], "read_failed");
    }
    #[test]
    fn explicit_profile_and_dry_run() {
        assert_eq!(
            parse(args(&["profile", "performance", "--dry-run"])).unwrap(),
            Action::Profile(PowerProfile::Performance, true, None)
        );
        assert!(parse(args(&["profile", "turbo"])).is_err());
        assert!(parse(args(&["profile", "balanced", "--force"])).is_err());
    }
    #[test]
    fn profile_expected_current_is_explicit_and_rejects_malformed_options() {
        assert_eq!(
            parse(args(&[
                "profile",
                "power-saver",
                "--expected-current",
                "balanced",
                "--dry-run"
            ]))
            .unwrap(),
            Action::Profile(PowerProfile::PowerSaver, true, Some("balanced".into()))
        );
        for input in [
            vec!["profile", "balanced", "--expected-current"],
            vec!["profile", "balanced", "--expected-current", ""],
            vec!["profile", "balanced", "--expected-current", " balanced"],
            vec!["profile", "balanced", "--expected-current", "x\n"],
            vec![
                "profile",
                "balanced",
                "--expected-current",
                "balanced",
                "--expected-current",
                "balanced",
            ],
            vec!["profile", "balanced", "--dry-run", "--dry-run"],
        ] {
            assert!(parse(args(&input)).is_err(), "{input:?}");
        }
    }
    fn profile_fixture_plan() -> mechrevo_console_core::ProfilePlan {
        mechrevo_console_core::ProfilePlan {
            requested: PowerProfile::PowerSaver,
            previous: "balanced".into(),
            target: "power-saver".into(),
            scope: "fixture system policy".into(),
            source: mechrevo_console_core::EvidenceSource {
                backend: "linux_power_profiles_daemon".into(),
                path: "fixture".into(),
                device_id: None,
            },
            details: None,
        }
    }
    #[test]
    fn profile_expected_current_mismatch_and_dry_run_never_call_setter() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let plan = profile_fixture_plan();
        for dry in [false, true] {
            let error = profile_output(dry, &plan, Some("performance"), || {
                calls.set(calls.get() + 1);
                unreachable!()
            })
            .unwrap_err();
            assert_eq!(error.code, "profile_expected_current_changed");
        }
        let output = profile_output(true, &plan, Some("balanced"), || {
            calls.set(calls.get() + 1);
            unreachable!()
        })
        .unwrap();
        assert_eq!(output, json!({"dry_run":true,"plan":plan}));
        assert_eq!(calls.get(), 0);
    }
    #[test]
    fn profile_confirmed_result_survives_output_failure_without_retry() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let plan = profile_fixture_plan();
        let output = profile_output(false, &plan, Some("balanced"), || {
            calls.set(calls.get() + 1);
            Ok(mechrevo_console_core::ApplyResult {
                requested: plan.requested,
                previous: plan.previous.clone(),
                confirmed: plan.target.clone(),
                scope: plan.scope.clone(),
                receipt: None,
            })
        })
        .unwrap();
        struct FailedOutput;
        impl Write for FailedOutput {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "fixture closed pipe",
                ))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let error = write_profile_output(&output, &mut FailedOutput).unwrap_err();
        let completed = error
            .details
            .iter()
            .find_map(|s| s.strip_prefix("completed_profile_output="))
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(completed).unwrap(), output);
        assert_eq!(output["confirmed"], "power-saver");
        assert_eq!(calls.get(), 1);
    }
    #[test]
    fn brightness_bounds_devices_and_unknown_flags() {
        for value in ["-1", "101", "256", "1.5", "nan", "99999999999999999"] {
            assert!(parse(args(&["brightness", value])).is_err());
        }
        for value in ["0", "100"] {
            assert!(parse(args(&["brightness", value, "--device", "screen"])).is_ok());
        }
        assert!(parse(args(&["brightness", "--dry-run"])).is_err());
        assert!(parse(args(&["brightness", "10", "--device"])).is_err());
        assert!(parse(args(&["brightness", "10", "--dry-run", "--dry-run"])).is_err());
    }
    #[test]
    fn export_requires_explicit_destination() {
        assert!(parse(args(&["export"])).is_err());
        assert!(parse(args(&["export", "file.json"])).is_err());
        assert_eq!(
            parse(args(&["export", "--output", "中文 空格.json"])).unwrap(),
            Action::Export("中文 空格.json".into())
        );
    }
    #[test]
    fn export_utf8_refuses_overwrite_and_reports_missing_parent() {
        let dir = std::env::temp_dir().join(format!(
            "mechrevo-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("中文 空格.json");
        let document = fixture_document();
        export(&path, &document).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("屏幕"));
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
            document
        );
        let original = std::fs::read(&path).unwrap();
        assert!(export(&path, &json!({"overwrite":true})).is_err());
        assert_eq!(original, std::fs::read(&path).unwrap());
        assert!(export(&dir.join("missing/export.json"), &json!({})).is_err());
        #[cfg(unix)]
        {
            let link = dir.join("existing-link.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(export(&link, &document).is_err());
            assert_eq!(original, std::fs::read(&path).unwrap());
            let absent = dir.join("absent.json");
            let dangling = dir.join("dangling-link.json");
            std::os::unix::fs::symlink(&absent, &dangling).unwrap();
            assert!(export(&dangling, &document).is_err());
            assert!(!absent.exists());
            assert!(std::fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn status_rejects_silent_ignored_options() {
        assert!(parse(args(&["status", "--force"])).is_err());
        assert!(parse(args(&["models", "a", "b"])).is_err());
    }
}

#[cfg(test)]
mod thermal_tests {
    use super::*;
    use mechrevo_console_core::{CapabilityStatus, EvidenceSource, SensorReading};
    use std::cell::{Cell, RefCell};

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    fn sample(value: Option<f64>) -> ThermalSample {
        let start = Instant::now();
        ThermalSample {
            readings: vec![SensorReading {
                id: "physical/temp1_input".into(),
                label: "CPU temperature".into(),
                device_id: "physical".into(),
                kind: "temperature".into(),
                value,
                unit: "°C".into(),
                source: EvidenceSource {
                    backend: "linux_sysfs".into(),
                    path: "class/hwmon/hwmon0/temp1_input".into(),
                    device_id: Some("physical".into()),
                },
                status: if value.is_some() {
                    CapabilityStatus::Available
                } else {
                    CapabilityStatus::Unavailable
                },
                reason: value
                    .is_none()
                    .then(|| "Runtime-suspended; input not read".into()),
            }],
            diagnostics: vec![],
            started_at: start,
            finished_at: start + Duration::from_millis(2),
        }
    }
    fn frames(bytes: &[u8]) -> Vec<Value> {
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    #[test]
    fn thermal_options_are_bounded_and_never_accept_write_or_privilege_flags() {
        assert_eq!(
            parse(arguments(&["thermal"])).unwrap(),
            Action::Thermal {
                samples: 1,
                interval_ms: 1000
            }
        );
        assert_eq!(
            parse(arguments(&[
                "thermal",
                "--samples",
                "3600",
                "--interval-ms",
                "250"
            ]))
            .unwrap(),
            Action::Thermal {
                samples: 3600,
                interval_ms: 250
            }
        );
        for args in [
            vec!["thermal", "--samples", "0"],
            vec!["thermal", "--samples", "3601"],
            vec!["thermal", "--samples", "65536"],
            vec!["thermal", "--samples", "+2"],
            vec!["thermal", "--samples", "1.5"],
            vec!["thermal", "--samples", " 2"],
            vec!["thermal", "--interval-ms", "249"],
            vec!["thermal", "--interval-ms", "60001"],
            vec!["thermal", "--interval-ms", "-1"],
            vec!["thermal", "--samples"],
            vec!["thermal", "--samples", "2", "--samples", "3"],
            vec!["thermal", "--interval-ms", "1000", "--interval-ms", "2000"],
            vec!["thermal", "--apply", "1"],
            vec!["thermal", "--sudo", "1"],
            vec!["thermal", "--output", "file.jsonl"],
            vec!["thermal", "--json"],
        ] {
            assert!(
                parse(arguments(&args)).is_err(),
                "unexpected acceptance: {args:?}"
            );
        }
    }
    #[test]
    fn jsonl_has_independent_values_times_and_no_sleep_after_final_sample() {
        let mut output = vec![];
        let calls = Cell::new(0);
        let sleeps = RefCell::new(vec![]);
        let clocks = Cell::new(100);
        stream_thermal(
            &mut output,
            2,
            250,
            || {
                calls.set(calls.get() + 1);
                Ok(sample((calls.get() == 1).then_some(62.0)))
            },
            |duration| sleeps.borrow_mut().push(duration),
            || {
                let value = clocks.get();
                clocks.set(value + 1);
                UNIX_EPOCH + Duration::from_secs(value)
            },
        )
        .unwrap();
        let rows = frames(&output);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["sequence"], 1);
        assert_eq!(rows[1]["sequence"], 2);
        assert_eq!(rows[0]["started_at"]["unix_seconds"], 100);
        assert_eq!(rows[1]["finished_at"]["unix_seconds"], 103);
        assert_eq!(rows[0]["monotonic_elapsed"]["nanoseconds"], 2_000_000);
        assert_eq!(rows[0]["readings"][0]["value"], 62.0);
        assert!(rows[1]["readings"][0]["value"].is_null());
        assert_eq!(rows[1]["readings"][0]["status"], "unavailable");
        assert_eq!(rows[1]["readings"][0]["id"], rows[0]["readings"][0]["id"]);
        assert_eq!(rows[1]["status"], "sampled");
        assert_eq!(*sleeps.borrow(), [Duration::from_millis(250)]);
        assert_eq!(calls.get(), 2);
    }
    #[test]
    fn collection_error_emits_empty_error_frame_then_stops_without_reusing_sample() {
        let mut output = vec![];
        let calls = Cell::new(0);
        let waits = Cell::new(0);
        let error = stream_thermal(
            &mut output,
            3,
            1000,
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    Ok(sample(Some(60.0)))
                } else {
                    Err(ControlError::new(
                        "thermal_sampling_unavailable",
                        "hwmon disappeared",
                    ))
                }
            },
            |_| waits.set(waits.get() + 1),
            SystemTime::now,
        )
        .unwrap_err();
        let rows = frames(&output);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["status"], "error");
        assert_eq!(rows[1]["readings"], json!([]));
        assert_eq!(rows[1]["error"]["code"], "thermal_sampling_unavailable");
        assert!(rows[1]["monotonic_elapsed"].is_null());
        assert_eq!(error.code, "thermal_sampling_unavailable");
        assert_eq!((calls.get(), waits.get()), (2, 1));
    }
    #[test]
    fn output_flush_failure_never_sleeps_or_collects_again() {
        struct BrokenFlush;
        impl Write for BrokenFlush {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "consumer gone"))
            }
        }
        let calls = Cell::new(0);
        let error = stream_thermal(
            &mut BrokenFlush,
            3,
            1000,
            || {
                calls.set(calls.get() + 1);
                Ok(sample(Some(60.0)))
            },
            |_| panic!("sleep after failed output"),
            SystemTime::now,
        )
        .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(error.code, "output_failed");
        assert!(error.details[0].contains("sample 1"));
    }
    #[test]
    fn partial_output_failure_stops_before_next_sample() {
        struct Partial {
            remaining: usize,
        }
        impl Write for Partial {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.remaining == 0 {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
                }
                let len = bytes.len().min(self.remaining);
                self.remaining -= len;
                Ok(len)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let calls = Cell::new(0);
        let error = stream_thermal(
            &mut Partial { remaining: 17 },
            3,
            250,
            || {
                calls.set(calls.get() + 1);
                Ok(sample(Some(61.0)))
            },
            |_| panic!("sleep after partial output"),
            SystemTime::now,
        )
        .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(error.code, "output_failed");
        assert!(error.details[0].contains("partial"));
    }
    #[test]
    fn failed_error_frame_keeps_collection_error_and_its_original_details() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }
        }
        let calls = Cell::new(0);
        let error = stream_thermal(
            &mut Broken,
            2,
            1000,
            || {
                calls.set(calls.get() + 1);
                let mut source = ControlError::new("sensor_removed", "hwmon disappeared");
                source.details.push("provider evidence".into());
                Err(source)
            },
            |_| panic!("failed output must stop"),
            SystemTime::now,
        )
        .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(error.code, "output_failed");
        let cause = error
            .details
            .iter()
            .find_map(|value| value.strip_prefix("thermal_collection_error: "))
            .unwrap();
        let source: ControlError = serde_json::from_str(cause).unwrap();
        assert_eq!(source.code, "sensor_removed");
        assert_eq!(source.message, "hwmon disappeared");
        assert_eq!(source.details, ["provider evidence"]);
    }
    #[test]
    fn wall_clock_rollback_and_invalid_monotonic_window_are_not_fresh_successes() {
        let mut output = vec![];
        let clocks = Cell::new(10);
        let error = stream_thermal(
            &mut output,
            2,
            1000,
            || {
                let mut sample = sample(Some(62.0));
                sample.started_at = sample.finished_at + Duration::from_secs(1);
                Ok(sample)
            },
            |_| panic!("invalid sample must stop"),
            || {
                let value = clocks.get();
                clocks.set(value - 1);
                UNIX_EPOCH + Duration::from_secs(value)
            },
        )
        .unwrap_err();
        let rows = frames(&output);
        assert_eq!(rows[0]["wall_clock_status"], "moved_backwards");
        assert_eq!(rows[0]["status"], "error");
        assert!(rows[0]["monotonic_elapsed"].is_null());
        assert_eq!(rows[0]["readings"], json!([]));
        assert_eq!(error.code, "thermal_sample_time_invalid");
    }
    #[test]
    fn before_epoch_clock_is_explicit_and_invalid_stream_options_never_collect() {
        let mut output = vec![];
        stream_thermal(
            &mut output,
            1,
            1000,
            || Ok(sample(None)),
            |_| unreachable!(),
            || UNIX_EPOCH - Duration::from_secs(1),
        )
        .unwrap();
        let rows = frames(&output);
        assert_eq!(rows[0]["wall_clock_status"], "before_unix_epoch");
        assert!(rows[0]["started_at"].is_null());
        for (samples, interval) in [(0, 1000), (3601, 1000), (1, 249), (1, 60001)] {
            assert!(stream_thermal(
                &mut output,
                samples,
                interval,
                || panic!("invalid request collected"),
                |_| panic!("invalid request slept"),
                SystemTime::now
            )
            .is_err());
        }
    }
}
