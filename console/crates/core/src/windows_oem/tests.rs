//! Offline production-transaction tests. Synthetic identities and transport
//! receipts below are never runtime-acceptance records or hardware evidence.
use super::contract::{SessionIdentity, SupportStage, FN_MASK};
use super::device::{FnByteRead, FnDevice, TransferEvidence, WriteSubmission};
use super::transaction::{self, FnLockIo};
use super::*;
use crate::{ControlError, OemValue};
use std::collections::VecDeque;

fn failure(code: &str) -> ControlError {
    ControlError::new(code, "offline injected failure")
}
fn transfer(value: u8) -> TransferEvidence {
    TransferEvidence {
        input_bytes: 4_194_304,
        output_capacity_bytes: 4_194_304,
        returned_bytes: 4_194_304,
        input_guard_intact: true,
        output_guard_intact: true,
        input_unchanged: true,
        output_tail_matches_input: true,
        output_prefix_le: value.into(),
        changed_output_bytes: 2,
        first_changed_output_offset: Some(0),
        last_changed_output_offset: Some(1),
    }
}

struct WriteBehavior {
    submitted: bool,
    effect: bool,
    error: Option<ControlError>,
}
impl WriteBehavior {
    fn ok() -> Self {
        Self {
            submitted: true,
            effect: true,
            error: None,
        }
    }
    fn failed(effect: bool, code: &str) -> Self {
        Self {
            submitted: true,
            effect,
            error: Some(failure(code)),
        }
    }
}
struct OfflineIo {
    byte: u8,
    reads: VecDeque<Result<u8, ControlError>>,
    behavior: VecDeque<WriteBehavior>,
    identities: VecDeque<Result<SessionIdentity, ControlError>>,
    writes: Vec<u8>,
    read_count: usize,
    revalidate_count: usize,
    status: Status,
}
impl OfflineIo {
    fn new(byte: u8) -> Self {
        Self {
            byte,
            reads: VecDeque::new(),
            behavior: VecDeque::new(),
            identities: VecDeque::new(),
            writes: vec![],
            read_count: 0,
            revalidate_count: 0,
            status: Status {
                contract_id: Some("offline-candidate".into()),
                support_stage: Some(SupportStage::Candidate),
                qualified: true,
                evidence: IdentityEvidence {
                    disk_file_sha256: Some(
                        "6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235".into(),
                    ),
                    loaded_module_metadata: Some(LoadedModuleEvidence {
                        service: "UWACPIDriver".into(),
                        device_instance_id: "ACPI\\INOU0000\\OFFLINE-FIXTURE".into(),
                        driver_inf: "oem-offline.inf".into(),
                        driver_version: "10.48.11.331".into(),
                        loaded_path: "C:\\Windows\\System32\\drivers\\UWACPIDriver.sys".into(),
                        association_level: "offline synthetic association, not Windows acceptance"
                            .into(),
                    }),
                    ..IdentityEvidence::default()
                },
                diagnostics: vec!["Offline synthetic identity; no Windows acceptance".into()],
                ..Status::default()
            },
        }
    }
    fn identity() -> SessionIdentity {
        SessionIdentity::new("offline-candidate", b"fixed offline lifecycle")
    }
    fn plan(&mut self, target: bool) -> OemPlan {
        let checked = transaction::probe(self);
        assert!(checked.error.is_none());
        super::preview(checked.status.as_ref().unwrap(), target).unwrap()
    }
}
impl FnDevice for OfflineIo {
    fn read_fn_byte(&mut self) -> Result<FnByteRead, ControlError> {
        self.read_count += 1;
        if let Some(value) = self.reads.pop_front() {
            self.byte = value?;
        }
        Ok(FnByteRead {
            value: self.byte,
            transfer: transfer(self.byte),
        })
    }
    fn write_fn_byte(&mut self, value: u8) -> WriteSubmission {
        let behavior = self.behavior.pop_front().unwrap_or_else(WriteBehavior::ok);
        if !behavior.submitted {
            return WriteSubmission::NotSubmitted(behavior.error.unwrap());
        }
        self.writes.push(value);
        if behavior.effect {
            self.byte = value;
        }
        WriteSubmission::Submitted(match behavior.error {
            Some(error) => Err(error),
            None => Ok(transfer(value)),
        })
    }
}
impl FnLockIo for OfflineIo {
    fn revalidate(&mut self) -> Result<SessionIdentity, ControlError> {
        self.revalidate_count += 1;
        self.identities
            .pop_front()
            .unwrap_or_else(|| Ok(Self::identity()))
    }
    fn status(&self) -> Status {
        self.status.clone()
    }
}

// @mark mechrevo.windows_fnlock role=verification
#[test]
fn all_full_bytes_preserve_every_unowned_bit_for_both_targets() {
    for previous in 0_u8..=255 {
        for target in [false, true] {
            let mut io = OfflineIo::new(previous);
            let plan = io.plan(target);
            let outcome = transaction::apply(&mut io, &plan);
            let expected = (previous & !FN_MASK) | if target { FN_MASK } else { 0 };
            assert_eq!(
                outcome.outcome,
                Outcome::AppliedVerified,
                "{previous:#x}/{target}"
            );
            assert_eq!(io.byte, expected);
            assert_eq!(io.byte & !FN_MASK, previous & !FN_MASK);
            assert_eq!(io.writes.len(), usize::from(previous != expected));
            assert_eq!(outcome.receipt.write_attempted, previous != expected);
            assert_eq!(outcome.receipt.full_byte_verified, Some(true));
            assert_eq!(outcome.receipt.protected_bits_preserved, Some(true));
            assert_eq!(outcome.receipt.recovery, Recovery::NotNeeded);
            assert!(outcome.error.is_none());
            assert_eq!(outcome.result.unwrap().confirmed, OemValue::Boolean(target));
        }
    }
}

#[test]
fn probe_and_cached_preview_do_not_write_or_requalify_during_preview() {
    let mut io = OfflineIo::new(0x4e);
    let probe = transaction::probe(&mut io);
    assert_eq!(probe.outcome, Outcome::Observed);
    assert!(!probe.receipt.write_attempted);
    assert_eq!(probe.receipt.full_byte_verified, None);
    let counters = (io.read_count, io.revalidate_count);
    let plan = super::preview(probe.status.as_ref().unwrap(), true).unwrap();
    assert_eq!((io.read_count, io.revalidate_count), counters);
    assert!(io.writes.is_empty());
    assert_eq!(plan.previous, OemValue::Boolean(false));
}

#[test]
fn no_op_still_requires_a_second_fresh_full_byte_read() {
    let mut io = OfflineIo::new(0x10);
    let plan = io.plan(true);
    io.reads
        .extend([Ok(0x10), Err(failure("no_op_read_failed"))]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
    assert_eq!(result.error.unwrap().code, "no_op_read_failed");
    assert!(io.writes.is_empty());
}

#[test]
fn preview_other_bit_change_rejects_even_when_fn_boolean_is_identical() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.byte = 2;
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
    assert_eq!(result.error.unwrap().code, "windows_fnlock_stale_plan");
    assert!(io.writes.is_empty());
}

#[test]
fn changed_full_byte_immediately_before_write_is_zero_write() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.reads.extend([Ok(0), Ok(2)]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
    assert_eq!(
        result.error.unwrap().code,
        "windows_fnlock_changed_before_write"
    );
    assert!(io.writes.is_empty());
}

#[test]
fn fixed_metadata_tampering_and_non_boolean_target_are_rejected_before_io() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    for index in 0..8 {
        let mut invalid = plan.clone();
        match index {
            0 => invalid.source.path = "another device".into(),
            1 => invalid.source.backend = "linux".into(),
            2 => invalid.kind = crate::OemControlKind::ChargingProfile,
            3 => invalid.requires_privilege = false,
            4 => invalid.companion_brightness = Some(1),
            5 => invalid.target = OemValue::Level(1),
            6 => invalid.binding = "not a SHA256 binding".into(),
            _ => invalid.readback_scope = "only Fn bit".into(),
        }
        let before = (io.read_count, io.revalidate_count);
        let result = transaction::apply(&mut io, &invalid);
        assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
        assert_eq!((io.read_count, io.revalidate_count), before);
        assert!(io.writes.is_empty());
    }
}

#[test]
fn false_return_after_actual_write_is_not_reported_rejected_and_can_restore() {
    let mut io = OfflineIo::new(0x42);
    let plan = io.plan(true);
    io.behavior
        .push_back(WriteBehavior::failed(true, "original_win32_failure"));
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedRestoredVerified);
    assert_eq!(io.writes, [0x52, 0x42]);
    assert_eq!(result.receipt.recovery, Recovery::RestoredVerified);
    assert_eq!(result.receipt.write_call_succeeded, Some(false));
    assert_eq!(result.receipt.restore_call_succeeded, Some(true));
    assert_eq!(
        result.receipt.primary_error.unwrap().code,
        "original_win32_failure"
    );
    assert_eq!(result.error.unwrap().code, "original_win32_failure");
}

#[test]
fn success_return_without_hardware_change_does_not_confirm_target() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior.push_back(WriteBehavior {
        submitted: true,
        effect: false,
        error: None,
    });
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedRestoredVerified);
    assert_eq!(result.receipt.recovery, Recovery::OriginalObserved);
    assert_eq!(result.receipt.original_state_verified, Some(true));
    assert!(!result.receipt.restore_attempted);
    assert_eq!(io.writes, [0x10]);
    assert!(result.result.is_none());
}

// @mark mechrevo.windows_fnlock role=verification
#[test]
fn third_party_protected_bit_change_is_not_overwritten() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.reads.extend([Ok(0), Ok(0), Ok(0x12)]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.recovery, Recovery::Skipped);
    assert_eq!(result.receipt.protected_bits_preserved, Some(false));
    assert_eq!(io.writes, [0x10]);
    assert_eq!(io.byte, 0x12);
    assert!(result.status.unwrap().control.is_none());
}

#[test]
fn unknown_readback_preserves_primary_and_never_blindly_restores() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior
        .push_back(WriteBehavior::failed(true, "primary_error"));
    io.reads
        .extend([Ok(0), Ok(0), Err(failure("independent_read_error"))]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.primary_error.unwrap().code, "primary_error");
    assert_eq!(
        result.receipt.readback_errors[0].code,
        "independent_read_error"
    );
    assert_eq!(io.writes, [0x10]);
}

#[test]
fn failed_restore_even_with_old_byte_observed_never_claims_restored() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior.extend([
        WriteBehavior::failed(true, "write_failed"),
        WriteBehavior::failed(true, "restore_failed"),
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(io.byte, 0);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.recovery, Recovery::Failed);
    assert_eq!(result.receipt.original_state_verified, Some(true));
    assert_eq!(result.receipt.restore_call_succeeded, Some(false));
    assert_eq!(result.receipt.restore_error.unwrap().code, "restore_failed");
    assert_eq!(result.receipt.primary_error.unwrap().code, "write_failed");
    assert!(result.result.is_none());
}

#[test]
fn restore_readback_error_is_retained_with_both_write_errors() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior.extend([
        WriteBehavior::failed(true, "write_failed"),
        WriteBehavior::failed(false, "restore_failed"),
    ]);
    io.reads.extend([
        Ok(0),
        Ok(0),
        Ok(0x10),
        Ok(0x10),
        Err(failure("restore_read_failed")),
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.primary_error.unwrap().code, "write_failed");
    assert_eq!(result.receipt.restore_error.unwrap().code, "restore_failed");
    assert_eq!(
        result.receipt.readback_errors[0].code,
        "restore_read_failed"
    );
    assert_eq!(result.receipt.original_state_verified, None);
}

#[test]
fn concurrent_change_between_recovery_checks_is_not_restored() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior
        .push_back(WriteBehavior::failed(true, "write_failed"));
    io.reads.extend([Ok(0), Ok(0), Ok(0x10), Ok(0x32)]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.recovery, Recovery::Skipped);
    assert_eq!(io.writes, [0x10]);
}

#[test]
fn changed_lifecycle_after_write_suppresses_success_and_restore() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.identities.extend([
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(SessionIdentity::new(
            "offline-candidate",
            b"different lifecycle",
        )),
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(
        result.receipt.identity_error.unwrap().code,
        "windows_fnlock_identity_changed"
    );
    assert_eq!(io.writes, [0x10]);
}

#[test]
fn qualified_status_and_fresh_native_identity_are_both_required() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.status.qualified = false;
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
    assert!(io.writes.is_empty());
    io.status.qualified = true;
    io.identities
        .push_back(Err(failure("native_qualification_failure")));
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.error.unwrap().code, "native_qualification_failure");
    assert!(io.writes.is_empty());
}

#[test]
fn allocation_or_handle_failure_before_submission_is_zero_write() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior.push_back(WriteBehavior {
        submitted: false,
        effect: false,
        error: Some(failure("allocation_failed")),
    });
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
    assert!(!result.receipt.write_attempted);
    assert_eq!(result.receipt.write_call_succeeded, None);
    assert!(io.writes.is_empty());
}

#[test]
fn lifecycle_changes_before_submission_are_zero_write() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.identities.extend([
        Ok(OfflineIo::identity()),
        Ok(SessionIdentity::new(
            "offline-candidate",
            b"replacement driver",
        )),
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
    assert_eq!(
        result.error.unwrap().code,
        "windows_fnlock_identity_changed"
    );
    assert!(io.writes.is_empty());
}

#[test]
fn failed_call_with_original_byte_observed_does_not_issue_restore() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior
        .push_back(WriteBehavior::failed(false, "original_call_failed"));
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedRestoredVerified);
    assert_eq!(result.receipt.recovery, Recovery::OriginalObserved);
    assert!(!result.receipt.restore_attempted);
    assert_eq!(result.receipt.write_call_succeeded, Some(false));
    assert_eq!(io.writes, [0x10]);
}

#[test]
fn successful_restore_call_with_wrong_full_byte_is_still_failed() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior.extend([
        WriteBehavior::failed(true, "original_call_failed"),
        WriteBehavior {
            submitted: true,
            effect: false,
            error: None,
        },
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.recovery, Recovery::Failed);
    assert_eq!(result.receipt.restore_call_succeeded, Some(true));
    assert_eq!(result.receipt.original_state_verified, Some(false));
    assert_eq!(
        result.receipt.restore_error.unwrap().code,
        "windows_fnlock_restore_mismatch"
    );
    assert_eq!(
        result.receipt.primary_error.unwrap().code,
        "original_call_failed"
    );
    assert_eq!(io.writes, [0x10, 0]);
}

#[test]
fn identity_failure_before_restore_is_not_allowed_to_overwrite_target() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior
        .push_back(WriteBehavior::failed(true, "original_call_failed"));
    io.identities.extend([
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Err(failure("lost_identity_before_restore")),
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(
        result.receipt.identity_error.unwrap().code,
        "lost_identity_before_restore"
    );
    assert_eq!(
        result.receipt.primary_error.unwrap().code,
        "original_call_failed"
    );
    assert!(!result.receipt.restore_attempted);
    assert_eq!(io.writes, [0x10]);
}

#[test]
fn restore_not_submitted_keeps_both_failures_and_does_not_claim_recovery() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior.extend([
        WriteBehavior::failed(true, "original_call_failed"),
        WriteBehavior {
            submitted: false,
            effect: false,
            error: Some(failure("restore_allocation_failed")),
        },
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.recovery, Recovery::Failed);
    assert!(!result.receipt.restore_attempted);
    assert_eq!(result.receipt.restore_call_succeeded, None);
    assert_eq!(result.receipt.original_state_verified, None);
    assert_eq!(
        result.receipt.primary_error.unwrap().code,
        "original_call_failed"
    );
    assert_eq!(
        result.receipt.restore_error.unwrap().code,
        "restore_allocation_failed"
    );
    assert_eq!(io.writes, [0x10]);
}

#[test]
fn identity_change_after_restore_read_does_not_verify_old_state() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior
        .push_back(WriteBehavior::failed(true, "original_call_failed"));
    io.identities.extend([
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(SessionIdentity::new(
            "offline-candidate",
            b"replaced after restore read",
        )),
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.recovery, Recovery::Failed);
    assert_eq!(result.receipt.original_state_verified, None);
    assert!(result.receipt.identity_error.is_some());
    assert_eq!(io.writes, [0x10, 0]);
}

#[test]
fn replacement_during_second_recovery_read_cannot_verify_original_state() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.behavior
        .push_back(WriteBehavior::failed(true, "original_call_failed"));
    io.reads.extend([Ok(0), Ok(0), Ok(0x10), Ok(0)]);
    io.identities.extend([
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(SessionIdentity::new(
            "offline-candidate",
            b"replaced during recovery read",
        )),
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::FailedStateUnknown);
    assert_eq!(result.receipt.original_state_verified, None);
    assert_eq!(result.receipt.recovery, Recovery::Unknown);
    assert_eq!(io.writes, [0x10]);
    assert!(!result.receipt.restore_attempted);
}

#[test]
fn no_op_does_not_confirm_an_identity_replaced_during_final_read() {
    let mut io = OfflineIo::new(0x10);
    let plan = io.plan(true);
    io.identities.extend([
        Ok(OfflineIo::identity()),
        Ok(OfflineIo::identity()),
        Ok(SessionIdentity::new(
            "offline-candidate",
            b"replaced during no-op read",
        )),
    ]);
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
    assert_eq!(result.receipt.full_byte_verified, None);
    assert!(io.writes.is_empty());
}

#[test]
fn binding_covers_full_byte_and_native_association_with_domain_separation() {
    let identity = OfflineIo::identity();
    assert_ne!(identity.binding(0), identity.binding(2));
    assert_ne!(
        identity.binding(0),
        SessionIdentity::new("other-contract", b"fixed offline lifecycle").binding(0)
    );
    assert_ne!(
        identity.binding(0),
        SessionIdentity::new("offline-candidate", b"different lifecycle").binding(0)
    );
    assert!(contract::is_sha256(&identity.binding(0)));
}

#[test]
fn bundled_contract_is_candidate_not_a_runtime_acceptance_record() {
    let catalog = contract::bundled_catalog().unwrap();
    assert!(!catalog.contracts.is_empty());
    for contract in &catalog.contracts {
        assert_eq!(contract.stage, SupportStage::Candidate);
        assert!(contract.runtime_acceptance.is_none());
        assert_eq!(
            contract.permits_current_build(0),
            cfg!(feature = "windows-fnlock-candidate")
        );
    }
}

#[test]
fn disk_digest_without_loaded_module_association_is_not_qualification() {
    let mut io = OfflineIo::new(0);
    let plan = io.plan(true);
    io.status.evidence.loaded_module_metadata = None;
    let before_reads = io.read_count;
    let result = transaction::apply(&mut io, &plan);
    assert_eq!(result.outcome, Outcome::RejectedBeforeWrite);
    assert_eq!(result.error.unwrap().code, "windows_fnlock_not_qualified");
    assert_eq!(io.read_count, before_reads);
    assert!(io.writes.is_empty());
    assert!(!result.status.unwrap().qualified);
}

// This checks the actual production transaction's output against the actual
// production wire validator. Enabling a candidate build remains distinct from
// any Windows machine having passed runtime acceptance.
#[cfg(feature = "windows-fnlock-candidate")]
#[test]
fn production_transaction_outcomes_roundtrip_the_strict_protocol() {
    use super::protocol::{self, Operation, Request, Response};
    for scenario in 0..8 {
        let mut io = OfflineIo::new(if scenario == 1 { 0x10 } else { 0 });
        let plan = io.plan(true);
        match scenario {
            2 => io.byte = 2,
            3 => io
                .behavior
                .push_back(WriteBehavior::failed(true, "write_failed")),
            4 => io
                .behavior
                .push_back(WriteBehavior::failed(false, "write_failed")),
            5 => io.reads.extend([Ok(0), Ok(0), Ok(0x12)]),
            6 => io.behavior.extend([
                WriteBehavior::failed(true, "write_failed"),
                WriteBehavior::failed(true, "restore_failed"),
            ]),
            7 => io.identities.push_back(Err(failure("identity_failed"))),
            _ => {}
        }
        let result = transaction::apply(&mut io, &plan);
        let request = Request {
            schema_version: protocol::SCHEMA_VERSION,
            request_id: format!("offline-roundtrip-{scenario}"),
            operation: Operation::Apply {
                plan: Box::new(plan),
            },
        };
        let exit = if result.error.is_some() { 2 } else { 0 };
        let response = Response::from_result(request.request_id.clone(), result);
        let bytes = protocol::encode_response(&response)
            .unwrap_or_else(|error| panic!("scenario {scenario}: {error}"));
        let decoded = protocol::decode_response(&bytes).unwrap();
        protocol::validate_exchange(&request, &decoded, exit)
            .unwrap_or_else(|error| panic!("scenario {scenario}: {error}"));
    }
    let mut io = OfflineIo::new(0x4e);
    let result = transaction::probe(&mut io);
    let request = Request {
        schema_version: protocol::SCHEMA_VERSION,
        request_id: "offline-probe".into(),
        operation: Operation::Probe,
    };
    let response = Response::from_result(&request.request_id, result);
    protocol::validate_exchange(
        &request,
        &protocol::decode_response(&protocol::encode_response(&response).unwrap()).unwrap(),
        0,
    )
    .unwrap();
}
