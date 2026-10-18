//! One-shot Windows OEM wire format. Shared display DTOs are not wire parsers:
//! every nested object is checked before conversion, including explicit nulls.
use super::{contract, HelperResult, Outcome, Receipt, Recovery, Status};
use crate::{CapabilityStatus, ControlError, OemApplyResult, OemDomain, OemPlan, OemValue};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 32 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Root has depth zero. Values more than sixteen nesting edges away are refused.
pub const MAX_JSON_DEPTH: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Probe,
    Apply { plan: Box<OemPlan> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: String,
    pub operation: Operation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub schema_version: u32,
    pub request_id: String,
    pub outcome: Outcome,
    pub status: Option<Status>,
    pub result: Option<OemApplyResult>,
    pub receipt: Receipt,
    pub error: Option<ControlError>,
}

impl Response {
    pub fn from_result(request_id: impl Into<String>, result: HelperResult) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            request_id: request_id.into(),
            outcome: result.outcome,
            status: result.status,
            result: result.result,
            receipt: result.receipt,
            error: result.error,
        }
    }
}

fn invalid(message: impl Into<String>) -> ControlError {
    ControlError::new("windows_fnlock_protocol_invalid", message)
}

// Value's normal Deserialize silently overwrites duplicate map keys. This
// depth-aware seed sees decoded keys (including escaped duplicates) at all levels.
struct UniqueSeed(usize);
impl<'de> DeserializeSeed<'de> for UniqueSeed {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if self.0 > MAX_JSON_DEPTH {
            return Err(serde::de::Error::custom("JSON depth limit exceeded"));
        }
        struct UniqueVisitor(usize);
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = Value;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bounded JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
                Ok(Value::Bool(value))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
                Ok(Value::from(value))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
                Ok(Value::from(value))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .ok_or_else(|| E::custom("Non-finite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
                Ok(Value::String(value.into()))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
                Ok(Value::String(value))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = access.next_element_seed(UniqueSeed(self.0 + 1))? {
                    values.push(value);
                }
                Ok(Value::Array(values))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = access.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("Duplicate JSON object key"));
                    }
                    values.insert(key, access.next_value_seed(UniqueSeed(self.0 + 1))?);
                }
                Ok(Value::Object(values))
            }
        }
        deserializer.deserialize_any(UniqueVisitor(self.0))
    }
}

fn parse(bytes: &[u8], limit: usize) -> Result<Value, ControlError> {
    if bytes.is_empty() || bytes.len() > limit {
        return Err(invalid(
            "JSON frame is empty or exceeds its fixed byte limit",
        ));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueSeed(0)
        .deserialize(&mut deserializer)
        .map_err(|e| invalid(format!("Invalid strict JSON: {e}")))?;
    deserializer
        .end()
        .map_err(|e| invalid(format!("Trailing JSON data: {e}")))?;
    Ok(value)
}

fn object<'a>(
    value: &'a Value,
    fields: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, ControlError> {
    let map = value
        .as_object()
        .ok_or_else(|| invalid("Expected a JSON object"))?;
    if map.len() != fields.len() || fields.iter().any(|field| !map.contains_key(*field)) {
        return Err(invalid(
            "Object has unknown or missing fields; nullable fields must be explicit",
        ));
    }
    Ok(map)
}

fn optional(
    value: &Value,
    check: impl FnOnce(&Value) -> Result<(), ControlError>,
) -> Result<(), ControlError> {
    if value.is_null() {
        Ok(())
    } else {
        check(value)
    }
}

fn error_shape(value: &Value) -> Result<(), ControlError> {
    object(value, &["code", "message", "details"])?;
    Ok(())
}

fn error_array_shape(value: &Value) -> Result<(), ControlError> {
    for error in value
        .as_array()
        .ok_or_else(|| invalid("Expected an error array"))?
    {
        error_shape(error)?;
    }
    Ok(())
}

fn source_shape(value: &Value) -> Result<(), ControlError> {
    object(value, &["backend", "path", "device_id"])?;
    Ok(())
}

fn enum_string(value: &Value) -> Result<(), ControlError> {
    if value.is_string() {
        Ok(())
    } else {
        Err(invalid(
            "Wire enums must use their exact string representation",
        ))
    }
}

fn boolean_shape(value: &Value) -> Result<(), ControlError> {
    let fields = object(value, &["type", "value"])?;
    if fields["type"] != "boolean" || !fields["value"].is_boolean() {
        return Err(invalid("Fn-lock value must be a typed Boolean"));
    }
    Ok(())
}

fn domain_shape(value: &Value) -> Result<(), ControlError> {
    let fields = object(value, &["type"])?;
    if fields["type"] != "boolean" {
        return Err(invalid("Fn-lock domain must be Boolean"));
    }
    Ok(())
}

fn plan_shape(value: &Value) -> Result<(), ControlError> {
    let p = object(
        value,
        &[
            "control_id",
            "kind",
            "label",
            "previous",
            "target",
            "domain",
            "source",
            "binding",
            "readback_scope",
            "requires_privilege",
            "companion_brightness",
        ],
    )?;
    boolean_shape(&p["previous"])?;
    boolean_shape(&p["target"])?;
    domain_shape(&p["domain"])?;
    source_shape(&p["source"])?;
    enum_string(&p["kind"])?;
    if !p["companion_brightness"].is_null() {
        return Err(invalid("Fn-lock has no companion brightness operation"));
    }
    Ok(())
}

fn control_shape(value: &Value) -> Result<(), ControlError> {
    let c = object(
        value,
        &[
            "id",
            "kind",
            "label",
            "current",
            "domain",
            "status",
            "writable",
            "source",
            "binding",
            "readback_scope",
            "reason",
        ],
    )?;
    optional(&c["current"], boolean_shape)?;
    domain_shape(&c["domain"])?;
    source_shape(&c["source"])?;
    enum_string(&c["kind"])?;
    enum_string(&c["status"])?;
    Ok(())
}

fn result_shape(value: &Value) -> Result<(), ControlError> {
    let r = object(
        value,
        &[
            "control_id",
            "kind",
            "previous",
            "confirmed",
            "source",
            "readback_scope",
        ],
    )?;
    boolean_shape(&r["previous"])?;
    boolean_shape(&r["confirmed"])?;
    source_shape(&r["source"])?;
    enum_string(&r["kind"])?;
    Ok(())
}

fn status_shape(value: &Value) -> Result<(), ControlError> {
    let s = object(
        value,
        &[
            "contract_id",
            "support_stage",
            "qualified",
            "control",
            "evidence",
            "blocking_errors",
            "diagnostics",
        ],
    )?;
    optional(&s["control"], control_shape)?;
    optional(&s["support_stage"], enum_string)?;
    error_array_shape(&s["blocking_errors"])?;
    let e = object(
        &s["evidence"],
        &[
            "disk_file_sha256",
            "loaded_module_metadata",
            "load_time_provenance",
            "current_memory_measurement",
            "runtime_acceptance_id",
        ],
    )?;
    optional(&e["loaded_module_metadata"], |value| {
        object(
            value,
            &[
                "service",
                "device_instance_id",
                "driver_inf",
                "driver_version",
                "loaded_path",
                "association_level",
            ],
        )?;
        Ok(())
    })?;
    Ok(())
}

fn receipt_shape(value: &Value) -> Result<(), ControlError> {
    let r = object(
        value,
        &[
            "write_attempted",
            "write_call_succeeded",
            "full_byte_verified",
            "protected_bits_preserved",
            "recovery",
            "restore_attempted",
            "restore_call_succeeded",
            "original_state_verified",
            "writer_may_continue",
            "primary_error",
            "readback_errors",
            "restore_error",
            "identity_error",
            "recovery_reason",
        ],
    )?;
    for field in ["primary_error", "restore_error", "identity_error"] {
        optional(&r[field], error_shape)?;
    }
    error_array_shape(&r["readback_errors"])?;
    enum_string(&r["recovery"])?;
    Ok(())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

pub fn validate_plan(plan: &OemPlan) -> Result<(), ControlError> {
    if plan.control_id != contract::CONTROL_ID
        || plan.kind != crate::OemControlKind::FnLock
        || plan.label != contract::CONTROL_LABEL
        || !matches!(plan.previous, OemValue::Boolean(_))
        || !matches!(plan.target, OemValue::Boolean(_))
        || plan.domain != OemDomain::Boolean
        || plan.source.backend != contract::SOURCE_BACKEND
        || plan.source.path != contract::SOURCE_PATH
        || !plan
            .source
            .device_id
            .as_deref()
            .is_some_and(|v| !v.is_empty() && v.len() <= 1024)
        || !contract::is_sha256(&plan.binding)
        || plan.readback_scope != contract::READBACK_SCOPE
        || !plan.requires_privilege
        || plan.companion_brightness.is_some()
    {
        return Err(invalid(
            "Plan is not the fixed Windows Boolean Fn-lock operation",
        ));
    }
    Ok(())
}

pub fn validate_status(status: &Status) -> Result<(), ControlError> {
    if status.evidence.current_memory_measurement.is_some() {
        return Err(invalid(
            "This implementation does not measure current kernel memory",
        ));
    }
    if let Some(control) = &status.control {
        let plan = OemPlan {
            control_id: control.id.clone(),
            kind: control.kind,
            label: control.label.clone(),
            previous: control.current.clone().unwrap_or(OemValue::Boolean(false)),
            target: OemValue::Boolean(false),
            domain: control.domain.clone(),
            source: control.source.clone(),
            binding: control.binding.clone(),
            readback_scope: control.readback_scope.clone(),
            requires_privilege: true,
            companion_brightness: None,
        };
        validate_plan(&plan)?;
        if !status.qualified && control.writable {
            return Err(invalid("Unqualified status advertises a writable control"));
        }
    }
    if status.qualified {
        let complete = status.contract_id.as_deref().is_some_and(valid_id)
            && status.support_stage.is_some()
            && status.blocking_errors.is_empty()
            && status.control.as_ref().is_some_and(|c| {
                c.writable
                    && c.status == CapabilityStatus::Available
                    && matches!(c.current, Some(OemValue::Boolean(_)))
            })
            && status.evidence.disk_file_sha256.as_deref()
                == Some("6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235")
            && status
                .evidence
                .loaded_module_metadata
                .as_ref()
                .is_some_and(|e| {
                    e.service == "UWACPIDriver"
                        && e.driver_version == "10.48.11.331"
                        && !e.device_instance_id.is_empty()
                        && !e.driver_inf.is_empty()
                        && !e.loaded_path.is_empty()
                        && !e.association_level.is_empty()
                        && status
                            .control
                            .as_ref()
                            .and_then(|c| c.source.device_id.as_deref())
                            == Some(e.device_instance_id.as_str())
                });
        let accepted = match status.support_stage {
            Some(super::SupportStage::RuntimeAccepted) => status
                .evidence
                .runtime_acceptance_id
                .as_deref()
                .is_some_and(|v| !v.is_empty()),
            Some(super::SupportStage::Candidate) => {
                cfg!(feature = "windows-fnlock-candidate")
                    && status.evidence.runtime_acceptance_id.is_none()
            }
            None => false,
        };
        if !complete || !accepted {
            return Err(invalid(
                "Qualified status lacks complete release-owned identity evidence",
            ));
        }
    }
    Ok(())
}

fn validate_receipt(receipt: &Receipt) -> Result<(), ControlError> {
    if (!receipt.write_attempted && receipt.write_call_succeeded.is_some())
        || (!receipt.restore_attempted && receipt.restore_call_succeeded.is_some())
        || (receipt.restore_attempted && !receipt.write_attempted)
        || (receipt.full_byte_verified == Some(true)
            && receipt.protected_bits_preserved != Some(true))
    {
        return Err(invalid("Receipt contains contradictory operation evidence"));
    }
    match receipt.recovery {
        Recovery::OriginalObserved
            if receipt.original_state_verified == Some(true)
                && !receipt.restore_attempted
                && receipt.restore_error.is_none() => {}
        Recovery::RestoredVerified
            if receipt.original_state_verified == Some(true)
                && receipt.restore_attempted
                && receipt.restore_call_succeeded == Some(true)
                && receipt.restore_error.is_none() => {}
        Recovery::OriginalObserved | Recovery::RestoredVerified => {
            return Err(invalid(
                "Recovery receipt lacks independent original-state confirmation",
            ))
        }
        _ => {}
    }
    Ok(())
}

fn validate_response(response: &Response) -> Result<(), ControlError> {
    if response.schema_version != SCHEMA_VERSION || !valid_id(&response.request_id) {
        return Err(invalid(
            "Unsupported response version or invalid request identifier",
        ));
    }
    if let Some(status) = &response.status {
        validate_status(status)?;
    }
    validate_receipt(&response.receipt)?;
    let receipt = &response.receipt;
    match response.outcome {
        Outcome::Observed => {
            if response
                .status
                .as_ref()
                .is_none_or(|status| !status.qualified)
                || response.result.is_some()
                || response.error.is_some()
                || receipt.write_attempted
                || receipt.restore_attempted
                || receipt.writer_may_continue
                || receipt.write_call_succeeded.is_some()
                || receipt.full_byte_verified.is_some()
                || receipt.protected_bits_preserved.is_some()
                || receipt.original_state_verified.is_some()
                || receipt.recovery != Recovery::NotNeeded
                || receipt.primary_error.is_some()
                || !receipt.readback_errors.is_empty()
                || receipt.restore_error.is_some()
                || receipt.identity_error.is_some()
            {
                return Err(invalid(
                    "Observed response is not a completed read-only observation",
                ));
            }
            if response.status.as_ref().is_some_and(|s| {
                s.qualified
                    && (!s.blocking_errors.is_empty()
                        || !s.control.as_ref().is_some_and(|c| {
                            c.writable
                                && c.status == CapabilityStatus::Available
                                && matches!(c.current, Some(OemValue::Boolean(_)))
                        }))
            }) {
                return Err(invalid(
                    "Qualified observation lacks a fresh readable Fn-lock control",
                ));
            }
        }
        Outcome::AppliedVerified => {
            if response.status.as_ref().is_none_or(|s| {
                !s.qualified
                    || !s.blocking_errors.is_empty()
                    || !s
                        .control
                        .as_ref()
                        .is_some_and(|c| c.writable && c.status == CapabilityStatus::Available)
            }) || response.result.is_none()
                || response.error.is_some()
                || receipt.full_byte_verified != Some(true)
                || receipt.protected_bits_preserved != Some(true)
                || receipt.writer_may_continue
                || receipt.restore_attempted
                || receipt.recovery != Recovery::NotNeeded
                || (receipt.write_attempted && receipt.write_call_succeeded != Some(true))
                || receipt.primary_error.is_some()
                || !receipt.readback_errors.is_empty()
                || receipt.restore_error.is_some()
                || receipt.identity_error.is_some()
            {
                return Err(invalid(
                    "Applied response lacks complete independent success evidence",
                ));
            }
        }
        Outcome::RejectedBeforeWrite => {
            if response.error.is_none()
                || response.result.is_some()
                || receipt.write_attempted
                || receipt.restore_attempted
                || receipt.writer_may_continue
                || receipt.recovery != Recovery::NotNeeded
            {
                return Err(invalid(
                    "Rejected response contradicts a write-before-rejection claim",
                ));
            }
        }
        Outcome::FailedRestoredVerified => {
            if response.error.is_none()
                || response.result.is_some()
                || receipt.primary_error.is_none()
                || !receipt.write_attempted
                || receipt.writer_may_continue
                || response
                    .status
                    .as_ref()
                    .is_none_or(|status| !status.qualified)
                || receipt.identity_error.is_some()
                || receipt.full_byte_verified != Some(false)
                || receipt.protected_bits_preserved != Some(true)
                || !matches!(
                    receipt.recovery,
                    Recovery::OriginalObserved | Recovery::RestoredVerified
                )
            {
                return Err(invalid(
                    "Failed-restored response lacks complete original-state confirmation",
                ));
            }
        }
        Outcome::FailedStateUnknown => {
            if response.error.is_none()
                || response.result.is_some()
                || response
                    .status
                    .as_ref()
                    .is_some_and(|status| status.qualified || status.control.is_some())
            {
                return Err(invalid(
                    "Unknown outcome must preserve failure and cannot confirm a result",
                ));
            }
        }
    }
    Ok(())
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, ControlError> {
    let value = parse(bytes, MAX_REQUEST_BYTES)?;
    let r = object(&value, &["schema_version", "request_id", "operation"])?;
    let op = r["operation"]
        .as_object()
        .ok_or_else(|| invalid("Operation is not an object"))?;
    match op.get("kind").and_then(Value::as_str) {
        Some("probe") => {
            object(&r["operation"], &["kind"])?;
        }
        Some("apply") => {
            object(&r["operation"], &["kind", "plan"])?;
            plan_shape(&op["plan"])?;
        }
        _ => return Err(invalid("Unsupported Windows OEM operation")),
    }
    let request: Request =
        serde_json::from_value(value).map_err(|e| invalid(format!("Invalid request type: {e}")))?;
    if request.schema_version != SCHEMA_VERSION || !valid_id(&request.request_id) {
        return Err(invalid(
            "Unsupported request version or invalid request identifier",
        ));
    }
    if let Operation::Apply { plan } = &request.operation {
        validate_plan(plan)?;
    }
    Ok(request)
}

pub fn encode_request(request: &Request) -> Result<Vec<u8>, ControlError> {
    let bytes = serde_json::to_vec(request).map_err(|e| invalid(e.to_string()))?;
    decode_request(&bytes)?;
    Ok(bytes)
}

pub fn decode_response(bytes: &[u8]) -> Result<Response, ControlError> {
    let value = parse(bytes, MAX_RESPONSE_BYTES)?;
    let r = object(
        &value,
        &[
            "schema_version",
            "request_id",
            "outcome",
            "status",
            "result",
            "receipt",
            "error",
        ],
    )?;
    enum_string(&r["outcome"])?;
    optional(&r["status"], status_shape)?;
    optional(&r["result"], result_shape)?;
    optional(&r["error"], error_shape)?;
    receipt_shape(&r["receipt"])?;
    let response: Response = serde_json::from_value(value)
        .map_err(|e| invalid(format!("Invalid response type: {e}")))?;
    validate_response(&response)?;
    Ok(response)
}

pub fn encode_response(response: &Response) -> Result<Vec<u8>, ControlError> {
    let bytes = serde_json::to_vec(response).map_err(|e| invalid(e.to_string()))?;
    decode_response(&bytes)?;
    Ok(bytes)
}

/// Validate stage evidence against this one-shot operation. This does not prove
/// process exit and must not itself release the client's lifetime gate.
pub fn validate_relation(request: &Request, response: &Response) -> Result<(), ControlError> {
    validate_response(response)?;
    if request.request_id != response.request_id || request.schema_version != SCHEMA_VERSION {
        return Err(invalid("Response does not match this authorized request"));
    }
    if matches!(request.operation, Operation::Probe)
        && (response.receipt.write_attempted
            || response.receipt.restore_attempted
            || response.result.is_some())
    {
        return Err(invalid(
            "A probe cannot report submitted hardware writes or restoration",
        ));
    }
    match (&request.operation, response.outcome) {
        (Operation::Probe, Outcome::Observed) => {}
        (Operation::Probe, Outcome::AppliedVerified | Outcome::FailedRestoredVerified) => {
            return Err(invalid("Probe returned a mutation outcome"))
        }
        (Operation::Apply { .. }, Outcome::Observed) => {
            return Err(invalid("Apply returned a probe outcome"))
        }
        (Operation::Apply { plan }, Outcome::AppliedVerified) => {
            validate_plan(plan)?;
            let result = response
                .result
                .as_ref()
                .ok_or_else(|| invalid("Missing apply result"))?;
            if result.control_id != plan.control_id
                || result.kind != plan.kind
                || result.previous != plan.previous
                || result.confirmed != plan.target
                || result.source != plan.source
                || result.readback_scope != plan.readback_scope
            {
                return Err(invalid(
                    "Confirmed result does not match the exact previewed Fn-lock plan",
                ));
            }
            let control = response
                .status
                .as_ref()
                .and_then(|s| s.control.as_ref())
                .ok_or_else(|| invalid("Missing post-write status"))?;
            if control.current.as_ref() != Some(&plan.target) || control.source != plan.source {
                return Err(invalid(
                    "Post-write observation does not confirm this Fn-lock target and source",
                ));
            }
            if !response.receipt.write_attempted && plan.previous != plan.target {
                return Err(invalid(
                    "A changed Fn-lock target cannot be a zero-write no-op",
                ));
            }
            if (plan.previous != plan.target) == (control.binding == plan.binding) {
                return Err(invalid(
                    "Post-write binding does not describe the verified complete-byte transition",
                ));
            }
        }
        (Operation::Apply { plan }, Outcome::FailedRestoredVerified) => {
            let control = response
                .status
                .as_ref()
                .and_then(|s| s.control.as_ref())
                .ok_or_else(|| {
                    invalid("Verified restoration lacks a fresh original-state observation")
                })?;
            if control.current.as_ref() != Some(&plan.previous)
                || control.source != plan.source
                || control.binding != plan.binding
            {
                return Err(invalid(
                    "Restored result is not this plan's original complete-byte binding",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Also verify the actual process exit. Never supply an inferred exit code just
/// because a response arrived; the native process handle supplies this evidence.
pub fn validate_exchange(
    request: &Request,
    response: &Response,
    exit_code: u32,
) -> Result<(), ControlError> {
    validate_relation(request, response)?;
    let success = matches!(
        response.outcome,
        Outcome::Observed | Outcome::AppliedVerified
    );
    if exit_code != if success { 0 } else { 2 } {
        return Err(invalid(
            "Actual helper exit contradicts the terminal response",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EvidenceSource, OemControl, OemControlKind};

    pub(super) fn plan() -> OemPlan {
        OemPlan {
            control_id: contract::CONTROL_ID.into(),
            kind: OemControlKind::FnLock,
            label: contract::CONTROL_LABEL.into(),
            previous: OemValue::Boolean(false),
            target: OemValue::Boolean(true),
            domain: OemDomain::Boolean,
            source: EvidenceSource {
                backend: contract::SOURCE_BACKEND.into(),
                path: contract::SOURCE_PATH.into(),
                device_id: Some("ACPI\\INOU0000\\0".into()),
            },
            binding: "a".repeat(64),
            readback_scope: contract::READBACK_SCOPE.into(),
            requires_privilege: true,
            companion_brightness: None,
        }
    }
    pub(super) fn status() -> Status {
        let p = plan();
        Status {
            contract_id: Some("accepted-test".into()),
            support_stage: Some(super::super::SupportStage::RuntimeAccepted),
            qualified: true,
            control: Some(OemControl {
                id: p.control_id,
                kind: p.kind,
                label: p.label,
                current: Some(p.previous),
                domain: p.domain,
                status: CapabilityStatus::Available,
                writable: true,
                source: p.source,
                binding: p.binding,
                readback_scope: p.readback_scope,
                reason: None,
            }),
            evidence: super::super::IdentityEvidence {
                disk_file_sha256: Some(
                    "6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235".into(),
                ),
                loaded_module_metadata: Some(super::super::LoadedModuleEvidence {
                    service: "UWACPIDriver".into(),
                    device_instance_id: "ACPI\\INOU0000\\0".into(),
                    driver_inf: "oem42.inf".into(),
                    driver_version: "10.48.11.331".into(),
                    loaded_path: "C:\\Windows\\System32\\drivers\\UWACPIDriver.sys".into(),
                    association_level: "present-pnp-running-scm-psapi-path-file".into(),
                }),
                load_time_provenance: None,
                current_memory_measurement: None,
                runtime_acceptance_id: Some("test-acceptance".into()),
            },
            blocking_errors: vec![],
            diagnostics: vec![],
        }
    }
    fn request() -> Request {
        Request {
            schema_version: 1,
            request_id: "fn-test-1".into(),
            operation: Operation::Apply {
                plan: Box::new(plan()),
            },
        }
    }
    fn observed() -> Response {
        Response::from_result(
            "fn-test-1",
            HelperResult {
                status: Some(status()),
                outcome: Outcome::Observed,
                result: None,
                receipt: Receipt::default(),
                error: None,
            },
        )
    }
    fn bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn strict_requests_roundtrip_and_reject_shared_nested_extensions_and_missing_nulls() {
        let request = request();
        let encoded = encode_request(&request).unwrap();
        assert!(decode_request(&encoded).is_ok());
        for path in ["root", "operation", "plan", "source", "previous", "domain"] {
            let mut v = serde_json::to_value(&request).unwrap();
            let object = match path {
                "root" => &mut v,
                "operation" => &mut v["operation"],
                "plan" => &mut v["operation"]["plan"],
                "source" => &mut v["operation"]["plan"]["source"],
                "previous" => &mut v["operation"]["plan"]["previous"],
                _ => &mut v["operation"]["plan"]["domain"],
            };
            object
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), Value::Bool(true));
            assert!(decode_request(&bytes(&v)).is_err(), "{path}");
        }
        for field in ["companion_brightness", "device_id"] {
            let mut v = serde_json::to_value(&request).unwrap();
            let object = if field == "device_id" {
                &mut v["operation"]["plan"]["source"]
            } else {
                &mut v["operation"]["plan"]
            };
            object.as_object_mut().unwrap().remove(field);
            assert!(decode_request(&bytes(&v)).is_err());
        }
    }

    #[test]
    fn duplicate_keys_invalid_utf8_trailing_data_and_byte_limits_are_rejected() {
        for input in [br#"{"schema_version":1,"schema_version":1,"request_id":"x","operation":{"kind":"probe"}}"#.as_slice(),br#"{"schema_version":1,"request_id":"x","operation":{"kind":"probe","\u006bind":"probe"}}"#.as_slice(),b"\xff",b"{}{}"] {
            assert!(decode_request(input).is_err());
        }
        assert!(decode_request(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
        assert!(decode_response(&vec![b' '; MAX_RESPONSE_BYTES + 1]).is_err());
        let probe = Request {
            schema_version: 1,
            request_id: "byte-boundary".into(),
            operation: Operation::Probe,
        };
        let mut exact = encode_request(&probe).unwrap();
        exact.resize(MAX_REQUEST_BYTES, b' ');
        assert!(decode_request(&exact).is_ok());
        exact.push(b' ');
        assert!(decode_request(&exact).is_err());
        let mut exact = encode_response(&observed()).unwrap();
        exact.resize(MAX_RESPONSE_BYTES, b' ');
        assert!(decode_response(&exact).is_ok());
        exact.push(b' ');
        assert!(decode_response(&exact).is_err());
        let encoded = String::from_utf8(encode_response(&observed()).unwrap()).unwrap();
        let duplicate = encoded.replace(
            "\"recovery\":\"not_needed\"",
            "\"recovery\":\"not_needed\",\"recovery\":\"not_needed\"",
        );
        assert_ne!(encoded, duplicate);
        assert!(decode_response(duplicate.as_bytes()).is_err());
    }

    #[test]
    fn depth_boundary_and_integer_representation_are_enforced() {
        for depth in [MAX_JSON_DEPTH, MAX_JSON_DEPTH + 1] {
            let mut v = Value::Null;
            for _ in 0..depth {
                v = Value::Array(vec![v]);
            }
            assert_eq!(
                parse(&bytes(&v), MAX_REQUEST_BYTES).is_ok(),
                depth == MAX_JSON_DEPTH
            );
        }
        for value in [
            "0",
            "4294967295",
            "4294967296",
            "-1",
            "1.0",
            "1e0",
            "true",
            "\"1\"",
            "null",
        ] {
            let s = format!(
                r#"{{"schema_version":{value},"request_id":"test","operation":{{"kind":"probe"}}}}"#
            );
            assert!(decode_request(s.as_bytes()).is_err(), "{value}");
        }
    }

    #[test]
    fn strict_responses_reject_every_missing_nullable_and_nested_unknown_field() {
        let baseline = serde_json::to_value(observed()).unwrap();
        assert!(decode_response(&bytes(&baseline)).is_ok());
        for parent in ["top", "status", "evidence", "control", "source", "receipt"] {
            let fields: Vec<&str> = match parent {
                "top" => vec!["status", "result", "error"],
                "status" => vec!["contract_id", "support_stage", "control"],
                "evidence" => vec![
                    "disk_file_sha256",
                    "loaded_module_metadata",
                    "load_time_provenance",
                    "current_memory_measurement",
                    "runtime_acceptance_id",
                ],
                "control" => vec!["current", "reason"],
                "source" => vec!["device_id"],
                _ => vec![
                    "write_call_succeeded",
                    "full_byte_verified",
                    "protected_bits_preserved",
                    "restore_call_succeeded",
                    "original_state_verified",
                    "primary_error",
                    "restore_error",
                    "identity_error",
                    "recovery_reason",
                ],
            };
            for field in fields {
                let mut v = baseline.clone();
                let node = match parent {
                    "top" => &mut v,
                    "status" => &mut v["status"],
                    "evidence" => &mut v["status"]["evidence"],
                    "control" => &mut v["status"]["control"],
                    "source" => &mut v["status"]["control"]["source"],
                    _ => &mut v["receipt"],
                };
                node.as_object_mut().unwrap().remove(field);
                assert!(decode_response(&bytes(&v)).is_err(), "{parent}.{field}");
            }
        }
        let mut v = baseline;
        v["receipt"]["readback_errors"] =
            serde_json::json!([{"code":"x","message":"x","details":[],"unknown":true}]);
        assert!(decode_response(&bytes(&v)).is_err());
    }

    #[test]
    fn applied_ack_requires_exact_target_full_byte_evidence_and_real_exit() {
        let req = request();
        let p = plan();
        let mut s = status();
        let c = s.control.as_mut().unwrap();
        c.current = Some(p.target.clone());
        c.binding = "b".repeat(64);
        let ack = Response::from_result(
            req.request_id.clone(),
            HelperResult {
                status: Some(s),
                outcome: Outcome::AppliedVerified,
                result: Some(OemApplyResult {
                    control_id: p.control_id.clone(),
                    kind: p.kind,
                    previous: p.previous.clone(),
                    confirmed: p.target.clone(),
                    source: p.source.clone(),
                    readback_scope: p.readback_scope.clone(),
                }),
                receipt: Receipt {
                    write_attempted: true,
                    write_call_succeeded: Some(true),
                    full_byte_verified: Some(true),
                    protected_bits_preserved: Some(true),
                    ..Receipt::default()
                },
                error: None,
            },
        );
        assert!(validate_exchange(&req, &ack, 0).is_ok());
        for exit in [1, 2, 3, 259] {
            assert!(validate_exchange(&req, &ack, exit).is_err());
        }
        let mut bad = ack.clone();
        bad.receipt.full_byte_verified = Some(false);
        assert!(validate_exchange(&req, &bad, 0).is_err());
        let mut bad = ack.clone();
        bad.result.as_mut().unwrap().confirmed = p.previous.clone();
        assert!(validate_exchange(&req, &bad, 0).is_err());
        let mut bad = ack.clone();
        bad.request_id = "foreign".into();
        assert!(validate_exchange(&req, &bad, 0).is_err());
        let mut bad = ack;
        bad.receipt.write_attempted = false;
        bad.receipt.write_call_succeeded = None;
        assert!(validate_exchange(&req, &bad, 0).is_err());
    }

    #[test]
    fn contradictory_observation_and_unknown_status_cannot_unlock_writes() {
        let mut response = observed();
        response.status = Some(Status::default());
        assert!(decode_response(&serde_json::to_vec(&response).unwrap()).is_err());
        response = observed();
        response.outcome = Outcome::FailedStateUnknown;
        response.error = Some(ControlError::new(
            "unknown",
            "Unconfirmed complete-byte state",
        ));
        assert!(decode_response(&serde_json::to_vec(&response).unwrap()).is_err());
        response.status = Some(Status::default());
        assert!(decode_response(&serde_json::to_vec(&response).unwrap()).is_ok());
    }
    #[test]
    fn restored_receipt_rejects_identity_failure_or_unqualified_original_state() {
        let error = ControlError::new("original_write_failed", "Offline injected write failure");
        let response = Response::from_result(
            "fn-test-1",
            HelperResult {
                status: Some(status()),
                outcome: Outcome::FailedRestoredVerified,
                result: None,
                receipt: Receipt {
                    write_attempted: true,
                    write_call_succeeded: Some(false),
                    full_byte_verified: Some(false),
                    protected_bits_preserved: Some(true),
                    recovery: Recovery::OriginalObserved,
                    original_state_verified: Some(true),
                    primary_error: Some(error.clone()),
                    ..Receipt::default()
                },
                error: Some(error.clone()),
            },
        );
        assert!(validate_exchange(&request(), &response, 2).is_ok());
        let mut bad = response.clone();
        bad.receipt.identity_error = Some(error.clone());
        assert!(decode_response(&serde_json::to_vec(&bad).unwrap()).is_err());
        let mut bad = response.clone();
        let s = bad.status.as_mut().unwrap();
        s.qualified = false;
        s.control.as_mut().unwrap().writable = false;
        assert!(decode_response(&serde_json::to_vec(&bad).unwrap()).is_err());
        let mut bad = response;
        bad.status.as_mut().unwrap().blocking_errors.push(error);
        assert!(decode_response(&serde_json::to_vec(&bad).unwrap()).is_err());
    }

    #[test]
    fn unit_enums_cannot_use_serde_external_object_representations_on_the_wire() {
        let mut request = serde_json::to_value(request()).unwrap();
        request["operation"]["plan"]["kind"] = serde_json::json!({"fn_lock":null});
        assert!(decode_request(&bytes(&request)).is_err());
        for field in [
            "outcome",
            "support_stage",
            "control_kind",
            "control_status",
            "recovery",
            "result_kind",
        ] {
            let mut response = serde_json::to_value(observed()).unwrap();
            match field {
                "outcome" => response["outcome"] = serde_json::json!({"observed":null}),
                "support_stage" => {
                    response["status"]["support_stage"] =
                        serde_json::json!({"runtime_accepted":null})
                }
                "control_kind" => {
                    response["status"]["control"]["kind"] = serde_json::json!({"fn_lock":null})
                }
                "control_status" => {
                    response["status"]["control"]["status"] = serde_json::json!({"available":null})
                }
                "recovery" => {
                    response["receipt"]["recovery"] = serde_json::json!({"not_needed":null})
                }
                _ => {
                    response["outcome"] = serde_json::json!("applied_verified");
                    response["status"]["control"]["current"] =
                        serde_json::json!({"type":"boolean","value":true});
                    response["status"]["control"]["binding"] = serde_json::json!("b".repeat(64));
                    response["receipt"]["write_attempted"] = serde_json::json!(true);
                    response["receipt"]["write_call_succeeded"] = serde_json::json!(true);
                    response["receipt"]["full_byte_verified"] = serde_json::json!(true);
                    response["receipt"]["protected_bits_preserved"] = serde_json::json!(true);
                    let p = plan();
                    response["result"] = serde_json::to_value(OemApplyResult {
                        control_id: p.control_id,
                        kind: p.kind,
                        previous: p.previous,
                        confirmed: p.target,
                        source: p.source,
                        readback_scope: p.readback_scope,
                    })
                    .unwrap();
                    assert!(decode_response(&bytes(&response)).is_ok());
                    response["result"]["kind"] = serde_json::json!({"fn_lock":null});
                }
            }
            assert!(decode_response(&bytes(&response)).is_err(), "{field}");
        }
    }
}
