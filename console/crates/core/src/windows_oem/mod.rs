//! Restricted Windows Fn-lock control. Cached UI data is never authorization.
//!
//! Candidate contracts exercise the same native transport as accepted ones,
//! but offline tests and EC bit readback do not establish Windows acceptance.
use crate::{ControlError, OemApplyResult, OemControl, OemPlan};
use serde::{Deserialize, Serialize};

pub mod client;
// Native-only internals remain available to production-path offline tests.
// Keep Windows and test diagnostics enabled; other platforms use shared DTOs.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub mod contract;
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) mod device;
#[cfg(any(target_os = "windows", test))]
pub mod helper;
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) mod identity;
#[cfg(any(target_os = "windows", test))]
pub(crate) mod ipc_policy;
#[cfg(all(test, not(target_os = "windows")))]
#[path = "native/mutex.rs"]
mod mutex_policy_tests;
#[cfg(target_os = "windows")]
pub(crate) mod native;
pub mod protocol;
#[cfg(any(target_os = "windows", test))]
pub(crate) mod release_manifest;
#[cfg(test)]
mod tests;
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) mod transaction;

pub use client::Client;
pub use contract::SupportStage;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Observed,
    AppliedVerified,
    RejectedBeforeWrite,
    FailedRestoredVerified,
    FailedStateUnknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientHelperLifetime {
    NotStarted,
    Running,
    Reaped,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct IdentityEvidence {
    pub disk_file_sha256: Option<String>,
    pub loaded_module_metadata: Option<LoadedModuleEvidence>,
    pub load_time_provenance: Option<String>,
    pub current_memory_measurement: Option<String>,
    pub runtime_acceptance_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadedModuleEvidence {
    pub service: String,
    pub device_instance_id: String,
    pub driver_inf: String,
    pub driver_version: String,
    pub loaded_path: String,
    /// Describes the association actually established, not memory measurement.
    pub association_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub contract_id: Option<String>,
    pub support_stage: Option<SupportStage>,
    pub qualified: bool,
    pub control: Option<OemControl>,
    pub evidence: IdentityEvidence,
    pub blocking_errors: Vec<ControlError>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    #[default]
    NotNeeded,
    OriginalObserved,
    RestoredVerified,
    Skipped,
    Failed,
    Unknown,
}

/// Complete stage evidence. A failed restore call remains a failure even when
/// a later read happens to show the old byte.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub write_attempted: bool,
    pub write_call_succeeded: Option<bool>,
    pub full_byte_verified: Option<bool>,
    pub protected_bits_preserved: Option<bool>,
    pub recovery: Recovery,
    pub restore_attempted: bool,
    pub restore_call_succeeded: Option<bool>,
    pub original_state_verified: Option<bool>,
    pub writer_may_continue: bool,
    pub primary_error: Option<ControlError>,
    pub readback_errors: Vec<ControlError>,
    pub restore_error: Option<ControlError>,
    pub identity_error: Option<ControlError>,
    pub recovery_reason: Option<String>,
}

impl Receipt {
    pub fn blocks_further_writes(&self) -> bool {
        self.writer_may_continue
            || matches!(
                self.recovery,
                Recovery::Skipped | Recovery::Failed | Recovery::Unknown
            )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub status: Option<Status>,
    pub outcome: Outcome,
    pub result: Option<OemApplyResult>,
    pub receipt: Receipt,
    pub error: Option<ControlError>,
    pub helper_reaped: bool,
    pub helper_lifetime: ClientHelperLifetime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperResult {
    pub status: Option<Status>,
    pub outcome: Outcome,
    pub result: Option<OemApplyResult>,
    pub receipt: Receipt,
    pub error: Option<ControlError>,
}

/// Preview only consumes a completed authorized observation. It never opens a
/// device or starts an elevated helper.
pub fn preview(status: &Status, target: bool) -> Result<OemPlan, ControlError> {
    transaction::preview(status, target)
}
