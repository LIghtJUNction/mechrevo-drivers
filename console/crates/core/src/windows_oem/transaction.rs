//! Full-byte Fn-lock ownership checks, independent readback and conditional
//! recovery. These comparisons do not constitute a firmware-wide atomic CAS.
use super::contract::{self, SessionIdentity, FN_MASK};
use super::device::{FnDevice, WriteSubmission};
use super::{HelperResult, Outcome, Receipt, Recovery, Status};
use crate::{
    CapabilityStatus, ControlError, EvidenceSource, OemApplyResult, OemControl, OemControlKind,
    OemDomain, OemPlan, OemValue,
};

/// A native implementation retains the qualified driver file, device and
/// application mutex throughout the operation. No client DTO implements it.
pub(crate) trait FnLockIo: FnDevice {
    fn revalidate(&mut self) -> Result<SessionIdentity, ControlError>;
    fn status(&self) -> Status;
}

fn error(code: &str, message: &str) -> ControlError {
    ControlError::new(code, message)
}

fn source(status: &Status) -> EvidenceSource {
    EvidenceSource {
        backend: contract::SOURCE_BACKEND.into(),
        path: contract::SOURCE_PATH.into(),
        device_id: status
            .evidence
            .loaded_module_metadata
            .as_ref()
            .map(|e| e.device_instance_id.clone()),
    }
}

fn qualified_status(io: &impl FnLockIo) -> Result<Status, ControlError> {
    let status = io.status();
    if !status.qualified
        || status.contract_id.as_ref().is_none_or(String::is_empty)
        || status.support_stage.is_none()
        || !status.blocking_errors.is_empty()
        || status.evidence.disk_file_sha256.as_deref()
            != Some("6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235")
        || status
            .evidence
            .loaded_module_metadata
            .as_ref()
            .is_none_or(|e| e.device_instance_id.is_empty())
    {
        return Err(error(
            "windows_fnlock_not_qualified",
            "The helper has not established current Fn-lock qualification",
        ));
    }
    Ok(status)
}

fn observe(mut status: Status, identity: &SessionIdentity, byte: u8) -> Status {
    let control_source = source(&status);
    status.control = Some(OemControl {
        id: contract::CONTROL_ID.into(),
        kind: OemControlKind::FnLock,
        label: contract::CONTROL_LABEL.into(),
        current: Some(OemValue::Boolean(byte & FN_MASK != 0)),
        domain: OemDomain::Boolean,
        status: CapabilityStatus::Available,
        writable: true,
        source: control_source,
        binding: identity.binding(byte),
        readback_scope: contract::READBACK_SCOPE.into(),
        reason: Some("Full EC-byte readback does not verify physical Fn behavior, persistence or the absence of third-party races".into()),
    });
    status
}

pub(super) fn preview(status: &Status, target: bool) -> Result<OemPlan, ControlError> {
    let control = status.control.as_ref().ok_or_else(|| {
        error(
            "windows_fnlock_probe_required",
            "Complete an authorized Windows Fn-lock check before preview",
        )
    })?;
    status.contract_id.as_deref().ok_or_else(|| {
        error(
            "windows_fnlock_not_qualified",
            "The cached check has no contract identity",
        )
    })?;
    if !status.qualified
        || !status.blocking_errors.is_empty()
        || status.support_stage.is_none()
        || control.id != contract::CONTROL_ID
        || control.kind != OemControlKind::FnLock
        || control.domain != OemDomain::Boolean
        || !control.writable
        || control.status != CapabilityStatus::Available
        || control.source != source(status)
        || control.label != contract::CONTROL_LABEL
        || control.readback_scope != contract::READBACK_SCOPE
        || !contract::is_sha256(&control.binding)
    {
        return Err(error(
            "windows_fnlock_not_qualified",
            "The cached Fn-lock control is not eligible for preview",
        ));
    }
    let previous = match control.current {
        Some(OemValue::Boolean(value)) => value,
        _ => {
            return Err(error(
                "windows_fnlock_probe_required",
                "The cached Fn-lock state is unknown",
            ))
        }
    };
    Ok(OemPlan {
        control_id: contract::CONTROL_ID.into(),
        kind: OemControlKind::FnLock,
        label: contract::CONTROL_LABEL.into(),
        previous: OemValue::Boolean(previous),
        target: OemValue::Boolean(target),
        domain: OemDomain::Boolean,
        source: control.source.clone(),
        binding: control.binding.clone(),
        readback_scope: contract::READBACK_SCOPE.into(),
        requires_privilege: true,
        companion_brightness: None,
    })
}

pub(crate) fn validate_plan_shape(plan: &OemPlan) -> Result<bool, ControlError> {
    let target = match plan.target {
        OemValue::Boolean(value) => value,
        _ => {
            return Err(error(
                "windows_fnlock_invalid_plan",
                "Only a Boolean Fn-lock target is permitted",
            ))
        }
    };
    if plan.control_id != contract::CONTROL_ID
        || plan.kind != OemControlKind::FnLock
        || !matches!(plan.previous, OemValue::Boolean(_))
        || plan.domain != OemDomain::Boolean
        || plan.label != contract::CONTROL_LABEL
        || !plan.requires_privilege
        || plan.companion_brightness.is_some()
        || plan.readback_scope != contract::READBACK_SCOPE
        || plan.source.backend != contract::SOURCE_BACKEND
        || plan.source.path != contract::SOURCE_PATH
        || plan.source.device_id.as_ref().is_none_or(String::is_empty)
        || !contract::is_sha256(&plan.binding)
    {
        return Err(error(
            "windows_fnlock_invalid_plan",
            "The request is not the fixed Windows Fn-lock plan",
        ));
    }
    Ok(target)
}

fn rejected(
    mut status: Option<Status>,
    primary: ControlError,
    mut receipt: Receipt,
) -> HelperResult {
    if let Some(status) = status.as_mut() {
        if status.control.is_none() {
            status.qualified = false;
            if status.blocking_errors.is_empty() {
                status.blocking_errors.push(primary.clone());
            }
        }
    }
    if receipt.primary_error.is_none() {
        receipt.primary_error = Some(primary.clone());
    }
    HelperResult {
        status,
        outcome: Outcome::RejectedBeforeWrite,
        result: None,
        receipt,
        error: Some(primary),
    }
}

fn unchanged(io: &mut impl FnLockIo, expected: &SessionIdentity) -> Result<(), ControlError> {
    let current = io.revalidate()?;
    qualified_status(io)?;
    if &current != expected {
        return Err(error(
            "windows_fnlock_identity_changed",
            "The qualified driver or firmware association changed during the operation",
        ));
    }
    Ok(())
}

// @mark mechrevo.windows_fnlock role=implementation
pub(crate) fn probe(io: &mut impl FnLockIo) -> HelperResult {
    let identity = match io.revalidate() {
        Ok(identity) => identity,
        Err(e) => return rejected(Some(io.status()), e, Receipt::default()),
    };
    let status = match qualified_status(io) {
        Ok(status) => status,
        Err(e) => return rejected(Some(io.status()), e, Receipt::default()),
    };
    let observed = match io.read_fn_byte() {
        Ok(read) => read.value,
        Err(e) => return rejected(Some(status), e, Receipt::default()),
    };
    if let Err(e) = unchanged(io, &identity) {
        return rejected(Some(io.status()), e, Receipt::default());
    }
    HelperResult {
        status: Some(observe(status, &identity, observed)),
        outcome: Outcome::Observed,
        result: None,
        receipt: Receipt::default(),
        error: None,
    }
}

fn applied(status: Status, plan: &OemPlan, mut receipt: Receipt) -> HelperResult {
    receipt.full_byte_verified = Some(true);
    receipt.protected_bits_preserved = Some(true);
    HelperResult {
        status: Some(status),
        outcome: Outcome::AppliedVerified,
        result: Some(OemApplyResult {
            control_id: contract::CONTROL_ID.into(),
            kind: OemControlKind::FnLock,
            previous: plan.previous.clone(),
            confirmed: plan.target.clone(),
            source: plan.source.clone(),
            readback_scope: contract::READBACK_SCOPE.into(),
        }),
        receipt,
        error: None,
    }
}

fn unknown(mut status: Status, receipt: Receipt) -> HelperResult {
    status.control = None;
    status.qualified = false;
    let primary = receipt.primary_error.clone().unwrap_or_else(|| {
        error(
            "windows_fnlock_state_unknown",
            "The complete Fn-lock byte could not be verified",
        )
    });
    status.blocking_errors.push(error(
        "windows_fnlock_probe_required",
        "State is unknown; a new successful authorized check is required",
    ));
    HelperResult {
        status: Some(status),
        outcome: Outcome::FailedStateUnknown,
        result: None,
        receipt,
        error: Some(primary),
    }
}

fn original_observed(
    status: Status,
    identity: &SessionIdentity,
    previous: u8,
    mut receipt: Receipt,
    restored: bool,
) -> HelperResult {
    receipt.recovery = if restored {
        Recovery::RestoredVerified
    } else {
        Recovery::OriginalObserved
    };
    receipt.original_state_verified = Some(true);
    receipt.full_byte_verified = Some(false);
    receipt.protected_bits_preserved = Some(true);
    let primary = receipt.primary_error.clone().unwrap_or_else(|| {
        error(
            "windows_fnlock_apply_failed",
            "The requested Fn-lock change was not verified",
        )
    });
    HelperResult {
        status: Some(observe(status, identity, previous)),
        outcome: Outcome::FailedRestoredVerified,
        result: None,
        receipt,
        error: Some(primary),
    }
}

fn record_read_error(receipt: &mut Receipt, e: ControlError) {
    if receipt.primary_error.is_none() {
        receipt.primary_error = Some(e.clone());
    }
    receipt.readback_errors.push(e);
}

fn record_identity_error(receipt: &mut Receipt, e: ControlError) {
    if receipt.primary_error.is_none() {
        receipt.primary_error = Some(e.clone());
    }
    receipt.identity_error = Some(e);
}

fn recover(
    io: &mut impl FnLockIo,
    status: Status,
    identity: &SessionIdentity,
    previous: u8,
    target: u8,
    current: Option<u8>,
    mut receipt: Receipt,
) -> HelperResult {
    if receipt.identity_error.is_some() || current.is_none() {
        receipt.recovery = Recovery::Unknown;
        receipt.recovery_reason =
            Some("Identity or complete readback is unknown; no restore write is safe".into());
        return unknown(status, receipt);
    }
    let current = current.unwrap();
    if current == previous {
        return original_observed(status, identity, previous, receipt, false);
    }
    if current != target {
        receipt.full_byte_verified = Some(false);
        receipt.protected_bits_preserved = Some((current & !FN_MASK) == (previous & !FN_MASK));
        receipt.recovery = Recovery::Skipped;
        receipt.recovery_reason = Some("A complete byte outside this operation's previous/target pair was observed; it was not overwritten".into());
        return unknown(status, receipt);
    }
    // Revalidate both identity and the entire target immediately before the
    // sole compensating write. This still cannot lock out firmware/OEM writers.
    if let Err(e) = unchanged(io, identity) {
        record_identity_error(&mut receipt, e);
        receipt.recovery = Recovery::Unknown;
        return unknown(status, receipt);
    }
    let before_restore = match io.read_fn_byte() {
        Ok(read) => read.value,
        Err(e) => {
            record_read_error(&mut receipt, e);
            receipt.recovery = Recovery::Unknown;
            return unknown(status, receipt);
        }
    };
    if let Err(e) = unchanged(io, identity) {
        record_identity_error(&mut receipt, e);
        receipt.recovery = Recovery::Unknown;
        return unknown(status, receipt);
    }
    if before_restore == previous {
        return original_observed(status, identity, previous, receipt, false);
    }
    if before_restore != target {
        receipt.recovery = Recovery::Skipped;
        receipt.recovery_reason = Some(
            "The complete byte changed before restore; third-party state was not overwritten"
                .into(),
        );
        return unknown(status, receipt);
    }
    match io.write_fn_byte(previous) {
        WriteSubmission::NotSubmitted(e) => {
            receipt.restore_error = Some(e);
            receipt.recovery = Recovery::Failed;
            return unknown(status, receipt);
        }
        WriteSubmission::Submitted(result) => {
            receipt.restore_attempted = true;
            receipt.restore_call_succeeded = Some(result.is_ok());
            if let Err(e) = result {
                receipt.restore_error = Some(e);
            }
        }
    }
    let restored = match io.read_fn_byte() {
        Ok(read) => Some(read.value),
        Err(e) => {
            record_read_error(&mut receipt, e);
            None
        }
    };
    if let Err(e) = unchanged(io, identity) {
        record_identity_error(&mut receipt, e);
    }
    receipt.original_state_verified = if receipt.identity_error.is_none() {
        restored.map(|value| value == previous)
    } else {
        None
    };
    if receipt.restore_error.is_none()
        && receipt.identity_error.is_none()
        && restored == Some(previous)
    {
        return original_observed(status, identity, previous, receipt, true);
    }
    // Even an independently observed old byte cannot turn a failed restore
    // invocation into a successful restoration receipt.
    receipt.recovery = Recovery::Failed;
    if restored.is_some_and(|value| value != previous) && receipt.restore_error.is_none() {
        receipt.restore_error = Some(error(
            "windows_fnlock_restore_mismatch",
            "Independent full-byte restore readback did not match the saved byte",
        ));
    }
    unknown(status, receipt)
}

// @mark mechrevo.windows_fnlock role=implementation
pub(crate) fn apply(io: &mut impl FnLockIo, plan: &OemPlan) -> HelperResult {
    let target_bit = match validate_plan_shape(plan) {
        Ok(target) => target,
        Err(e) => return rejected(Some(io.status()), e, Receipt::default()),
    };
    let identity = match io.revalidate() {
        Ok(identity) => identity,
        Err(e) => return rejected(Some(io.status()), e, Receipt::default()),
    };
    let status = match qualified_status(io) {
        Ok(status) => status,
        Err(e) => return rejected(Some(io.status()), e, Receipt::default()),
    };
    let previous = match io.read_fn_byte() {
        Ok(read) => read.value,
        Err(e) => return rejected(Some(status), e, Receipt::default()),
    };
    let status = observe(status, &identity, previous);
    let expected_plan = match preview(&status, target_bit) {
        Ok(plan) => plan,
        Err(e) => return rejected(Some(status), e, Receipt::default()),
    };
    if &expected_plan != plan {
        return rejected(
            Some(status),
            error(
                "windows_fnlock_stale_plan",
                "The fresh full-byte binding or fixed plan metadata differs from preview",
            ),
            Receipt::default(),
        );
    }
    let target = (previous & !FN_MASK) | if target_bit { FN_MASK } else { 0 };
    if let Err(e) = unchanged(io, &identity) {
        return rejected(Some(io.status()), e, Receipt::default());
    }
    let before_write = match io.read_fn_byte() {
        Ok(read) => read.value,
        Err(e) => return rejected(Some(status), e, Receipt::default()),
    };
    if before_write != previous {
        return rejected(
            Some(observe(status, &identity, before_write)),
            error(
                "windows_fnlock_changed_before_write",
                "The full EC byte changed immediately before writing; no write was attempted",
            ),
            Receipt::default(),
        );
    }
    if previous == target {
        if let Err(e) = unchanged(io, &identity) {
            return rejected(Some(io.status()), e, Receipt::default());
        }
        return applied(observe(status, &identity, target), plan, Receipt::default());
    }
    let mut receipt = Receipt::default();
    match io.write_fn_byte(target) {
        WriteSubmission::NotSubmitted(e) => return rejected(Some(status), e, receipt),
        WriteSubmission::Submitted(result) => {
            receipt.write_attempted = true;
            receipt.write_call_succeeded = Some(result.is_ok());
            if let Err(e) = result {
                receipt.primary_error = Some(e);
            }
        }
    }
    let observed = match io.read_fn_byte() {
        Ok(read) => Some(read.value),
        Err(e) => {
            record_read_error(&mut receipt, e);
            None
        }
    };
    if let Err(e) = unchanged(io, &identity) {
        record_identity_error(&mut receipt, e);
    }
    if receipt.primary_error.is_none() && observed == Some(target) {
        return applied(observe(status, &identity, target), plan, receipt);
    }
    if receipt.primary_error.is_none() {
        receipt.primary_error = Some(error(
            "windows_fnlock_readback_mismatch",
            "Independent complete-byte readback did not confirm the requested byte",
        ));
    }
    recover(io, status, &identity, previous, target, observed, receipt)
}
