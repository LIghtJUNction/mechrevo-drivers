//! Explicitly authorized CPU PL1 policy, separate from OS profiles and OEM EC.
//!
//! Construction and preview are local. The only subprocess entry points wait
//! for the fixed installed helper; an unknown outcome never becomes rejection.
use crate::{command::CommandOutput, ControlError};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub const HELPER_PATH: &str = "/usr/lib/mechrevo-power-control/console-helper";
#[cfg(target_os = "linux")]
const PKEXEC_PATH: &str = "/usr/bin/pkexec";
#[cfg(target_os = "linux")]
const MAX_REQUEST_BYTES: usize = 32 * 1024;
const OUTPUT_LIMIT: usize = 64 * 1024;
const MINIMUM_LIMIT_UW: u64 = 35_000_000;
const THERMAL_SPEC_REFERENCE_UW: u64 = 55_000_000;
// The installed controller's existing state/observation wire domain is i32.
const MAXIMUM_LIMIT_UW: u64 = i32::MAX as u64;

// deserialize_with deliberately has no serde(default): missing and null are
// different wire values. Every nullable protocol field is still required.
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub cap_uw: u64,
    #[serde(deserialize_with = "nullable")]
    pub target_celsius: Option<u16>,
    #[serde(deserialize_with = "nullable")]
    pub polling_seconds: Option<u64>,
    #[serde(deserialize_with = "nullable")]
    pub rapl_debounce_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Set,
    Clear,
    Recover,
    Reapply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Set { policy: Policy },
    Clear,
    Recover,
    Reapply,
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::Set { .. } => ActionKind::Set,
            Self::Clear => ActionKind::Clear,
            Self::Recover => ActionKind::Recover,
            Self::Reapply => ActionKind::Reapply,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub revision: String,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Management {
    Native,
    Managed,
    RecoveryPending,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interface {
    Msr,
    Mmio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedLimit {
    pub interface: Interface,
    #[serde(deserialize_with = "nullable")]
    pub pl1_uw: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub management: Management,
    #[serde(deserialize_with = "nullable")]
    pub revision: Option<String>,
    pub qualified: bool,
    pub allowed_actions: Vec<ActionKind>,
    #[serde(deserialize_with = "nullable")]
    pub current_policy: Option<Policy>,
    #[serde(deserialize_with = "nullable")]
    pub operational_ceiling_uw: Option<u64>,
    pub minimum_limit_uw: u64,
    pub thermal_spec_reference_uw: u64,
    pub observed: Vec<ObservedLimit>,
    #[serde(deserialize_with = "nullable")]
    pub thermald_active: Option<bool>,
    pub holding_now: bool,
    pub blocking_errors: Vec<String>,
    pub diagnostic_errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Observed,
    AppliedVerified,
    RejectedBeforeWrite,
    FailedRestoredVerified,
    FailedStateUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuPowerUpdate {
    #[serde(deserialize_with = "nullable")]
    pub status: Option<Status>,
    pub outcome: Outcome,
    #[serde(deserialize_with = "nullable")]
    pub error: Option<ControlError>,
    #[serde(deserialize_with = "nullable")]
    pub receipt: Option<Value>,
    pub helper_reaped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Probe,
    Execute { plan: Plan },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    request_id: String,
    operation: Operation,
}

// Use an independent strict wire error: the shared ControlError keeps its
// existing public serde contract for older APIs.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireError {
    code: String,
    message: String,
    details: Vec<String>,
}
impl From<WireError> for ControlError {
    fn from(value: WireError) -> Self {
        Self {
            code: value.code,
            message: value.message,
            details: value.details,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    schema_version: u32,
    request_id: String,
    outcome: Outcome,
    #[serde(deserialize_with = "nullable")]
    status: Option<Status>,
    #[serde(deserialize_with = "nullable")]
    receipt: Option<Value>,
    #[serde(deserialize_with = "nullable")]
    error: Option<WireError>,
}

fn invalid(message: impl Into<String>) -> ControlError {
    ControlError::new("cpu_power_invalid_plan", message)
}

fn revision_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}

fn validate_policy(policy: &Policy) -> Result<(), ControlError> {
    if policy.cap_uw == 0 || policy.cap_uw > MAXIMUM_LIMIT_UW || policy.cap_uw % 1_000_000 != 0 {
        return Err(invalid(
            "CPU cap must be positive whole watts within the controller's i32 micro-watt domain",
        ));
    }
    if policy.target_celsius.is_some_and(|v| v != 90)
        || policy.polling_seconds.is_some_and(|v| v != 1)
        || policy.rapl_debounce_seconds.is_some_and(|v| v != 0)
        || (policy.polling_seconds.is_some() && policy.target_celsius != Some(90))
        || (policy.rapl_debounce_seconds.is_some()
            && (policy.target_celsius != Some(90) || policy.polling_seconds != Some(1)))
    {
        return Err(invalid("CPU thermal options must be explicit native values or the qualified 90/1/0 combination"));
    }
    Ok(())
}

fn complete_observation(status: &Status) -> bool {
    status.observed.len() == 2
        && status
            .observed
            .iter()
            .any(|v| v.interface == Interface::Msr && v.pl1_uw.is_some_and(|v| v > 0))
        && status
            .observed
            .iter()
            .any(|v| v.interface == Interface::Mmio && v.pl1_uw.is_some_and(|v| v > 0))
}

fn validate_status(status: &Status) -> Result<(), ControlError> {
    if status.minimum_limit_uw != MINIMUM_LIMIT_UW
        || status.thermal_spec_reference_uw != THERMAL_SPEC_REFERENCE_UW
        || status
            .revision
            .as_deref()
            .is_some_and(|v| !revision_valid(v))
        || status
            .operational_ceiling_uw
            .is_some_and(|v| v == 0 || v > MAXIMUM_LIMIT_UW)
        || status
            .observed
            .iter()
            .any(|v| v.pl1_uw.is_some_and(|v| v == 0 || v > MAXIMUM_LIMIT_UW))
    {
        return Err(invalid(
            "CPU status contains an invalid reference, revision or limit",
        ));
    }
    for (index, action) in status.allowed_actions.iter().enumerate() {
        if status.allowed_actions[..index].contains(action) {
            return Err(invalid("CPU status repeats an allowed action"));
        }
    }
    for (index, reading) in status.observed.iter().enumerate() {
        if status.observed[..index]
            .iter()
            .any(|v| v.interface == reading.interface)
        {
            return Err(invalid("CPU status repeats a PL1 interface"));
        }
    }
    if let Some(policy) = &status.current_policy {
        validate_policy(policy)?;
        if policy.cap_uw < status.minimum_limit_uw
            || status
                .operational_ceiling_uw
                .is_some_and(|ceiling| policy.cap_uw > ceiling)
        {
            return Err(invalid(
                "CPU current policy is outside its reported operating interval",
            ));
        }
    }
    if (status.management == Management::Native && status.current_policy.is_some())
        || (status.management == Management::Managed && status.current_policy.is_none())
        || (status.management == Management::Native
            && status.allowed_actions.iter().any(|v| *v != ActionKind::Set))
        || (status.management == Management::Managed
            && status.allowed_actions.contains(&ActionKind::Recover))
        || (status.management == Management::Unknown && !status.allowed_actions.is_empty())
        || (status.management == Management::RecoveryPending
            && status
                .allowed_actions
                .iter()
                .any(|v| *v != ActionKind::Recover))
    {
        return Err(invalid("CPU management state and policy/actions disagree"));
    }
    if status.qualified
        && (status.management == Management::Unknown
            || status.revision.is_none()
            || status.operational_ceiling_uw.is_none()
            || !status.blocking_errors.is_empty()
            || !complete_observation(status))
    {
        return Err(invalid(
            "CPU qualified status lacks complete qualification evidence",
        ));
    }
    if !status.qualified && !status.allowed_actions.is_empty() {
        return Err(invalid("Unqualified CPU status advertises write actions"));
    }
    if status.holding_now {
        let valid = status.qualified
            && status.management == Management::Managed
            && status.thermald_active == Some(true)
            && status.current_policy.as_ref().is_some_and(|policy| {
                status
                    .observed
                    .iter()
                    .all(|v| v.pl1_uw.is_some_and(|limit| limit <= policy.cap_uw))
            });
        if !valid {
            return Err(invalid("CPU holding flag lacks complete managed readback"));
        }
    }
    Ok(())
}

/// Create a local preview from an already received status. No filesystem,
/// authorization or helper call occurs here. The helper rechecks this revision
/// under its operation lock before making any change.
pub fn preview(status: &Status, action: Action) -> Result<Plan, ControlError> {
    validate_status(status)?;
    if !status.qualified
        || !status.blocking_errors.is_empty()
        || !status.allowed_actions.contains(&action.kind())
    {
        return Err(ControlError::new(
            "cpu_power_action_unavailable",
            "The cached CPU status does not authorize this action; explicitly probe again",
        ));
    }
    let applicable = match action {
        Action::Set { .. } => matches!(status.management, Management::Native | Management::Managed),
        Action::Clear | Action::Reapply => status.management == Management::Managed,
        Action::Recover => status.management == Management::RecoveryPending,
    };
    if !applicable {
        return Err(invalid(
            "CPU action does not match the cached management state",
        ));
    }
    if let Action::Set { policy } = &action {
        validate_policy(policy)?;
        if policy.cap_uw < status.minimum_limit_uw
            || policy.cap_uw
                > status
                    .operational_ceiling_uw
                    .ok_or_else(|| invalid("No qualified operational ceiling"))?
        {
            return Err(invalid(
                "CPU cap is outside the qualified operational interval",
            ));
        }
    }
    Ok(Plan {
        revision: status
            .revision
            .clone()
            .ok_or_else(|| invalid("Missing CPU status revision"))?,
        action,
    })
}

// @mark mechrevo.power_control role=interface
/// Stateful client for explicit CPU policy authorization. New/Default are pure.
#[derive(Debug, Default)]
pub struct Client {
    cached: Option<Status>,
    needs_probe: bool,
    lifetime_unknown: bool,
}

impl Client {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cached_status(&self) -> Option<&Status> {
        self.cached.as_ref()
    }
    pub fn helper_reaped(&self) -> bool {
        !self.lifetime_unknown
    }

    pub fn preview(&self, action: Action) -> Result<Plan, ControlError> {
        if self.needs_probe || self.lifetime_unknown {
            return Err(ControlError::new(
                "cpu_power_probe_required",
                "A fresh successful CPU probe is required before creating a plan",
            ));
        }
        preview(
            self.cached.as_ref().ok_or_else(|| {
                ControlError::new(
                    "cpu_power_probe_required",
                    "Explicitly probe CPU policy before creating a plan",
                )
            })?,
            action,
        )
    }

    pub fn probe_authorized(&mut self) -> CpuPowerUpdate {
        self.perform(Operation::Probe, run_installed)
    }

    pub fn execute_authorized(&mut self, plan: &Plan) -> CpuPowerUpdate {
        if !cfg!(target_os = "linux") {
            return rejected(ControlError::new("cpu_power_unsupported", "The installed CPU PL1 controller is available only on Linux; no platform fallback is used"), true);
        }
        self.perform(Operation::Execute { plan: plan.clone() }, run_installed)
    }

    fn perform<F>(&mut self, operation: Operation, runner: F) -> CpuPowerUpdate
    where
        F: FnOnce(&Request) -> Result<CommandOutput, ControlError>,
    {
        if self.lifetime_unknown {
            return rejected(ControlError::new("cpu_power_helper_liveness_unknown", "The previous helper's exit has not been confirmed; another helper cannot be started"), false);
        }
        if let Operation::Execute { plan } = &operation {
            let local = self.preview(plan.action.clone());
            match local {
                Ok(current) if current == *plan => {}
                Ok(_) => {
                    return rejected(
                        ControlError::new(
                            "cpu_power_stale_plan",
                            "The CPU plan does not match the cached revision; create a new preview",
                        ),
                        true,
                    )
                }
                Err(error) => return rejected(error, true),
            }
        }
        let is_probe = matches!(&operation, Operation::Probe);
        let request = Request {
            schema_version: 1,
            request_id: request_id(),
            operation,
        };
        let update = match runner(&request) {
            Ok(output) => decode_reply(&request, self.cached.as_ref(), output),
            Err(error) => runner_failure(error),
        };
        if let Some(status) = &update.status {
            self.cached = Some(status.clone());
        }
        if !update.helper_reaped {
            self.lifetime_unknown = true;
        }
        match update.outcome {
            Outcome::Observed
                if is_probe
                    && update.error.is_none()
                    && update.status.as_ref().is_some_and(|v| v.qualified) =>
            {
                self.needs_probe = false
            }
            Outcome::AppliedVerified => self.needs_probe = false,
            Outcome::FailedRestoredVerified | Outcome::FailedStateUnknown => {
                self.needs_probe = true
            }
            // An unsuccessful explicit probe cannot leave an older successful
            // observation usable for writes.
            _ if is_probe => self.needs_probe = true,
            _ => {}
        }
        update
    }
}

fn rejected(error: ControlError, reaped: bool) -> CpuPowerUpdate {
    CpuPowerUpdate {
        status: None,
        outcome: if reaped {
            Outcome::RejectedBeforeWrite
        } else {
            Outcome::FailedStateUnknown
        },
        error: Some(error),
        receipt: None,
        helper_reaped: reaped,
    }
}

fn request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "cpu-{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(target_os = "linux")]
fn validate_program(path: &std::path::Path) -> Result<(), ControlError> {
    use std::os::unix::fs::MetadataExt;
    let mut component_path = std::path::PathBuf::from("/");
    for component in path.components().skip(1) {
        component_path.push(component);
        let metadata = std::fs::symlink_metadata(&component_path).map_err(|e| {
            ControlError::new(
                "cpu_power_helper_unavailable",
                format!("Cannot inspect {}: {e}", component_path.display()),
            )
        })?;
        if metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
            || metadata.file_type().is_symlink()
            || (component_path != path && !metadata.is_dir())
            || (component_path == path && (!metadata.is_file() || metadata.mode() & 0o111 == 0))
        {
            return Err(ControlError::new("cpu_power_helper_untrusted", "CPU helper and its directories must be root-owned, executable where required, and not writable by other users"));
        }
    }
    Ok(())
}

// @mark mechrevo.power_control role=implementation
fn run_installed(request: &Request) -> Result<CommandOutput, ControlError> {
    #[cfg(target_os = "linux")]
    {
        validate_program(std::path::Path::new(HELPER_PATH))?;
        validate_program(std::path::Path::new(PKEXEC_PATH))?;
        let payload = serde_json::to_string(request).map_err(|e| invalid(e.to_string()))?;
        if payload.len() > MAX_REQUEST_BYTES {
            return Err(invalid("CPU request exceeds the protocol size limit"));
        }
        crate::command::run_control_helper(
            PKEXEC_PATH,
            &[
                "--disable-internal-agent",
                HELPER_PATH,
                "--request",
                &payload,
            ],
            OUTPUT_LIMIT,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = request;
        Err(ControlError::new("cpu_power_unsupported", "The installed CPU PL1 controller is available only on Linux; no platform fallback is used"))
    }
}

fn runner_failure(mut error: ControlError) -> CpuPowerUpdate {
    let (outcome, reaped, mapped) = match error.code.as_str() {
        "oem_outcome_unknown" => (
            Outcome::FailedStateUnknown,
            true,
            "cpu_power_outcome_unknown",
        ),
        "oem_helper_liveness_unknown" => (
            Outcome::FailedStateUnknown,
            false,
            "cpu_power_helper_liveness_unknown",
        ),
        "command_unavailable" => (
            Outcome::RejectedBeforeWrite,
            true,
            "cpu_power_helper_unavailable",
        ),
        "cpu_power_helper_unavailable"
        | "cpu_power_helper_untrusted"
        | "cpu_power_unsupported"
        | "cpu_power_invalid_plan" => (Outcome::RejectedBeforeWrite, true, ""),
        _ => (
            Outcome::FailedStateUnknown,
            false,
            "cpu_power_runner_unknown",
        ),
    };
    if !mapped.is_empty() {
        error
            .details
            .push(format!("runner_error_code: {}", error.code));
        error.code = mapped.into();
    }
    CpuPowerUpdate {
        status: None,
        outcome,
        error: Some(error),
        receipt: None,
        helper_reaped: reaped,
    }
}

fn append_output(error: &mut ControlError, output: &CommandOutput) {
    error
        .details
        .push(format!("helper_exit_status: {}", output.status));
    if !output.stdout.is_empty() {
        error.details.push(format!("stdout: {}", output.stdout));
    }
    if !output.stderr.is_empty() {
        error.details.push(format!("stderr: {}", output.stderr));
    }
}

fn protocol_error(output: &CommandOutput, message: impl Into<String>) -> CpuPowerUpdate {
    let mut error = ControlError::new("cpu_power_outcome_unknown", message);
    append_output(&mut error, output);
    CpuPowerUpdate {
        status: None,
        outcome: Outcome::FailedStateUnknown,
        error: Some(error),
        receipt: None,
        helper_reaped: true,
    }
}

// Parse once with a duplicate-aware Value visitor, including receipt objects.
// A regular serde_json::Value would silently retain the last duplicate key.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(v)))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|v| UniqueValue(Value::Number(v)))
                    .ok_or_else(|| E::custom("Non-finite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v.into())))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v)))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = access.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = access.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!("Duplicate key: {key}")));
                    }
                    values.insert(key, access.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

fn receipt_matches(receipt: &Value, action: &Action, success: bool) -> bool {
    let Some(object) = receipt.as_object() else {
        return false;
    };
    let operation = match action {
        Action::Set { .. } => "set",
        Action::Clear => "clear",
        Action::Recover => "recover",
        Action::Reapply => "apply",
    };
    if object.get("version").and_then(Value::as_u64) != Some(1)
        || object.get("operation").and_then(Value::as_str) != Some(operation)
        || object.get("success").and_then(Value::as_bool) != Some(success)
    {
        return false;
    }
    if success
        && !receipt_observed(
            object.get("observed"),
            match action {
                Action::Set { policy } => Some(policy.cap_uw),
                Action::Reapply => object.get("cap_uw").and_then(Value::as_u64),
                _ => None,
            },
        )
    {
        return false;
    }
    if let Action::Set { policy } = action {
        if success && object.get("cap_uw").and_then(Value::as_u64) != Some(policy.cap_uw) {
            return false;
        }
    }
    if success && !matches!(action, Action::Reapply) {
        object.get("phase").and_then(Value::as_str) == Some("completed")
            && object
                .get("transaction")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
    } else if !success {
        object.get("restoration_verified").and_then(Value::as_bool) == Some(true)
            && object.get("rollback_error") == Some(&Value::Null)
            && matches!(action, Action::Set { .. } | Action::Clear)
    } else {
        object
            .get("transaction")
            .is_some_and(|value| value.is_null() || value.as_str().is_some_and(|v| !v.is_empty()))
            && object
                .get("cap_uw")
                .and_then(Value::as_u64)
                .is_some_and(|v| v > 0)
            && object
                .get("writes")
                .and_then(Value::as_array)
                .is_some_and(|writes| {
                    writes.iter().enumerate().all(|(index, value)| {
                        value.as_str().is_some_and(|v| ZONES.contains(&v))
                            && !writes[..index].contains(value)
                    })
                })
    }
}

const ZONES: [&str; 2] = [
    "/sys/class/powercap/intel-rapl:0",
    "/sys/class/powercap/intel-rapl-mmio:0",
];

fn receipt_observed(value: Option<&Value>, cap: Option<u64>) -> bool {
    value.and_then(Value::as_object).is_some_and(|object| {
        object.len() == 2
            && ZONES.iter().all(|zone| {
                object
                    .get(*zone)
                    .and_then(Value::as_object)
                    .and_then(|v| v.get("pl1_uw"))
                    .and_then(Value::as_u64)
                    .is_some_and(|v| {
                        v > 0 && v <= MAXIMUM_LIMIT_UW && cap.is_none_or(|limit| v <= limit)
                    })
            })
    })
}

fn rejection_receipt(receipt: &Value, action: &Action) -> bool {
    let Some(object) = receipt.as_object() else {
        return false;
    };
    let operation = match action {
        Action::Set { .. } => "set",
        Action::Clear => "clear",
        Action::Recover => "recover",
        Action::Reapply => "apply",
    };
    object.get("version").and_then(Value::as_u64) == Some(1)
        && object.get("operation").and_then(Value::as_str) == Some(operation)
        && object.get("success").and_then(Value::as_bool) == Some(false)
        && object.get("phase").and_then(Value::as_str) != Some("completed")
        && object.get("restoration_verified").and_then(Value::as_bool) != Some(true)
        && object
            .get("writes")
            .is_none_or(|value| value.as_array().is_some_and(|writes| writes.is_empty()))
}

fn success_matches(status: &Status, action: &Action, previous: Option<&Status>) -> bool {
    if !status.qualified || !status.blocking_errors.is_empty() {
        return false;
    }
    match action {
        Action::Set { policy } => {
            status.management == Management::Managed
                && status.current_policy.as_ref() == Some(policy)
                && status.holding_now
        }
        Action::Clear | Action::Recover => {
            status.management == Management::Native
                && status.current_policy.is_none()
                && status.thermald_active == Some(true)
        }
        Action::Reapply => {
            status.management == Management::Managed
                && status.holding_now
                && previous.is_some_and(|old| status.current_policy == old.current_policy)
        }
    }
}

fn decode_reply(
    request: &Request,
    previous: Option<&Status>,
    output: CommandOutput,
) -> CpuPowerUpdate {
    if matches!(output.status, 126 | 127) && output.stdout.trim().is_empty() {
        let (code, message) = if output.status == 126 {
            (
                "cpu_power_authorization_cancelled",
                "Administrator authorization was cancelled before the CPU helper ran",
            )
        } else {
            (
                "cpu_power_authorization_unavailable",
                "CPU policy authorization or its agent is unavailable",
            )
        };
        let mut error = ControlError::new(code, message);
        append_output(&mut error, &output);
        return rejected(error, true);
    }
    if output.stdout.len() + output.stderr.len() > OUTPUT_LIMIT {
        // The production runner enforces this; reject inconsistent injected or
        // future runners without treating an oversized response as success.
        return protocol_error(
            &output,
            "CPU helper output exceeds the shared capture limit",
        );
    }
    let value = match serde_json::from_str::<UniqueValue>(&output.stdout) {
        Ok(value) => value.0,
        Err(error) => {
            return protocol_error(
                &output,
                format!("CPU helper did not return a complete unique-key JSON result: {error}"),
            )
        }
    };
    let mut reply: Reply = match serde_json::from_value(value) {
        Ok(reply) => reply,
        Err(error) => {
            return protocol_error(
                &output,
                format!("CPU helper result violates the required schema: {error}"),
            )
        }
    };
    if reply.schema_version != 1 || reply.request_id != request.request_id {
        return protocol_error(
            &output,
            "CPU helper result does not match the request/version",
        );
    }
    if let Some(status) = &reply.status {
        if let Err(error) = validate_status(status) {
            return protocol_error(
                &output,
                format!("CPU helper status is inconsistent: {error}"),
            );
        }
    }
    let expected_exit = if (reply.outcome == Outcome::Observed && reply.error.is_none())
        || reply.outcome == Outcome::AppliedVerified
    {
        0
    } else {
        1
    };
    if output.status != expected_exit {
        return protocol_error(
            &output,
            "CPU helper outcome disagrees with its actual exit status",
        );
    }
    let valid = match (&request.operation, reply.outcome) {
        (Operation::Probe, Outcome::Observed) => reply.status.is_some() && reply.receipt.is_none(),
        (Operation::Execute { plan }, Outcome::AppliedVerified) => {
            reply.error.is_none()
                && reply
                    .status
                    .as_ref()
                    .is_some_and(|status| success_matches(status, &plan.action, previous))
                && (!matches!(&plan.action, Action::Reapply)
                    || reply
                        .receipt
                        .as_ref()
                        .and_then(|v| v.get("cap_uw"))
                        .and_then(Value::as_u64)
                        == reply
                            .status
                            .as_ref()
                            .and_then(|v| v.current_policy.as_ref())
                            .map(|v| v.cap_uw))
                && reply
                    .receipt
                    .as_ref()
                    .is_some_and(|receipt| receipt_matches(receipt, &plan.action, true))
        }
        (Operation::Execute { plan }, Outcome::FailedRestoredVerified) => {
            matches!(&plan.action, Action::Set { .. } | Action::Clear)
                && reply.error.is_some()
                && reply.status.as_ref().is_some_and(|status| {
                    status.qualified
                        && status.blocking_errors.is_empty()
                        && previous.is_some_and(|old| {
                            status.management == old.management
                                && status.current_policy == old.current_policy
                        })
                        && (status.management == Management::Native
                            && status.thermald_active == Some(true)
                            || status.holding_now)
                })
                && match &reply.receipt {
                    Some(receipt) => receipt_matches(receipt, &plan.action, false),
                    None => reply.error.as_ref().is_some_and(|error| {
                        error
                            .details
                            .iter()
                            .any(|v| v == "cpu_power_restore_confirmed")
                    }),
                }
        }
        (Operation::Probe, Outcome::RejectedBeforeWrite) => {
            reply.error.is_some() && reply.receipt.is_none()
        }
        (Operation::Execute { plan }, Outcome::RejectedBeforeWrite) => {
            reply.error.is_some()
                && reply
                    .receipt
                    .as_ref()
                    .is_none_or(|receipt| rejection_receipt(receipt, &plan.action))
        }
        (_, Outcome::FailedStateUnknown) => reply.error.is_some(),
        _ => false,
    };
    if !valid {
        return protocol_error(
            &output,
            "CPU helper action, receipt, post-state and outcome do not agree",
        );
    }
    let error = reply.error.take().map(|error| {
        let mut error: ControlError = error.into();
        append_output(&mut error, &output);
        error
    });
    CpuPowerUpdate {
        status: reply.status,
        outcome: reply.outcome,
        error,
        receipt: reply.receipt,
        helper_reaped: true,
    }
}

#[cfg(test)]
mod tests;
