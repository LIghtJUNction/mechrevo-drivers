use mechrevo_console_core::{registry, ControlError, PowerProfile, Service};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
    str::FromStr,
};
const HELP:&str="mechrevo-console 0.1.0 — Windows/Linux OS controls and evidence\n\nUsage:\n  mechrevo-console status [--json]\n  mechrevo-console capabilities\n  mechrevo-console models [query]\n  mechrevo-console profile <balanced|performance|power-saver> [--dry-run]\n  mechrevo-console brightness [percent] [--device ID] [--dry-run]\n  mechrevo-console export --output PATH\n  mechrevo-console --help\n\nPower profiles use Linux ACPI platform_profile or installed Windows OS power\nschemes. They are separate from OEM presets and never tune GPU/MUX/EC registers.\nBrightness requires an enumerated OS device; multiple devices require --device.\nExport writes UTF-8 JSON, requires an explicit path and refuses overwrites.\nMissing telemetry is unknown, never a simulated zero. No automatic sudo.\n";
#[derive(Debug, PartialEq)]
enum Action {
    Help,
    Version,
    Status(bool),
    Capabilities,
    Models(String),
    Profile(PowerProfile, bool),
    Brightness(Option<u8>, Option<String>, bool),
    Export(PathBuf),
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
        "capabilities" if rest.is_empty() => Ok(Action::Capabilities),
        "models" if rest.len() <= 1 => Ok(Action::Models(
            rest.first().map(text).transpose()?.unwrap_or("").into(),
        )),
        "profile" => {
            if rest.is_empty() || rest.len() > 2 {
                return Err(usage("profile requires one profile and optional --dry-run"));
            }
            let profile = PowerProfile::from_str(text(&rest[0])?)?;
            let dry = rest.len() == 2;
            if dry && rest[1] != "--dry-run" {
                return Err(usage("Unknown profile option"));
            }
            Ok(Action::Profile(profile, dry))
        }
        "brightness" => {
            let mut percent = None;
            let mut device = None;
            let mut dry = false;
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
                        if id.is_empty() {
                            return Err(usage("Device ID must not be empty"));
                        }
                        device = Some(id.into());
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
            Ok(Action::Brightness(percent, device, dry))
        }
        "export" if rest.len() == 2 && rest[0] == "--output" && !rest[1].is_empty() => {
            Ok(Action::Export(PathBuf::from(&rest[1])))
        }
        "export" => Err(usage(
            "export requires --output PATH; existing files are never replaced",
        )),
        _ => Err(usage("Unknown command or invalid options; use --help")),
    }
}
fn status_json(service: &Service) -> Result<Value, ControlError> {
    let snapshot = service.snapshot();
    let identification = registry::identify(&snapshot.identity);
    let mut value = serde_json::to_value(&snapshot)
        .map_err(|e| ControlError::new("serialization_failed", e.to_string()))?;
    value["identity_evidence"] = serde_json::to_value(identification)
        .map_err(|e| ControlError::new("serialization_failed", e.to_string()))?;
    value["schema_version"] = 1.into();
    Ok(value)
}
fn write_json(writer: &mut impl Write, value: &Value) -> Result<(), ControlError> {
    serde_json::to_writer_pretty(&mut *writer, value)
        .map_err(|e| ControlError::new("output_failed", e.to_string()))?;
    writeln!(writer).map_err(|e| ControlError::new("output_failed", e.to_string()))
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
            write!(stdout, "{HELP}")
                .map_err(|e| ControlError::new("output_failed", e.to_string()))?;
            return Ok(());
        }
        Action::Version => json!({"version":env!("CARGO_PKG_VERSION")}),
        Action::Status(true) => status_json(service)?,
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
            for d in s.diagnostics {
                writeln!(stdout, "{}: {}", d.code, d.message)
                    .map_err(|e| ControlError::new("output_failed", e.to_string()))?;
            }
            return Ok(());
        }
        Action::Capabilities => json!(service.capabilities()),
        Action::Models(query) => json!(service.catalog(&query)),
        Action::Profile(profile, dry) => {
            let plan = service.plan_profile(profile)?;
            if dry {
                json!({"dry_run":true,"plan":plan})
            } else {
                json!(service.apply_profile(&plan)?)
            }
        }
        Action::Brightness(None, device, _) => {
            let devices = service.brightness_devices();
            if let Some(id) = device {
                let selected = devices
                    .into_iter()
                    .filter(|d| d.id == id)
                    .collect::<Vec<_>>();
                if selected.is_empty() {
                    return Err(ControlError::new(
                        "brightness_device_unavailable",
                        "Requested device was not enumerated",
                    ));
                }
                json!(selected)
            } else {
                json!(devices)
            }
        }
        Action::Brightness(Some(percent), device, dry) => {
            let plan = service.plan_brightness(percent, device.as_deref())?;
            if dry {
                json!({"dry_run":true,"plan":plan})
            } else {
                json!(service.apply_brightness(&plan)?)
            }
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
        let _ = write_json(&mut io::stderr().lock(), &json!({"error":error}));
        std::process::exit(2)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn explicit_profile_and_dry_run() {
        assert_eq!(
            parse(args(&["profile", "performance", "--dry-run"])).unwrap(),
            Action::Profile(PowerProfile::Performance, true)
        );
        assert!(parse(args(&["profile", "turbo"])).is_err());
        assert!(parse(args(&["profile", "balanced", "--force"])).is_err());
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
        export(&path, &json!({"label":"屏幕"})).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("屏幕"));
        let original = std::fs::read(&path).unwrap();
        assert!(export(&path, &json!({"overwrite":true})).is_err());
        assert_eq!(original, std::fs::read(&path).unwrap());
        assert!(export(&dir.join("missing/export.json"), &json!({})).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn status_rejects_silent_ignored_options() {
        assert!(parse(args(&["status", "--force"])).is_err());
        assert!(parse(args(&["models", "a", "b"])).is_err());
    }
}
