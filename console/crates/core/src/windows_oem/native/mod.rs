//! One authenticated helper request owns its mutex, protected driver/package
//! files and fixed device handle until all synchronous I/O and close complete.
pub(super) mod device;
pub(super) mod identity;
pub(super) mod ipc;
pub(super) mod mutex;

use super::{
    contract::SessionIdentity,
    device::{FnByteRead, FnDevice, TransferEvidence, WriteSubmission},
    transaction::{self, FnLockIo},
    HelperResult, Outcome, Receipt, Status,
};
use crate::{ControlError, OemPlan};
use std::sync::Arc;

/// A native capability rather than caller-supplied qualification metadata. It
/// cannot be serialized or constructed outside this module.
struct QualifiedSession {
    // Drop the device and held files before releasing the cross-session mutex.
    device: device::Device,
    driver: Arc<identity::QualifiedDriverGuard>,
    _mutex: mutex::FnLockMutex,
    transport_trace: Vec<String>,
    omitted_transport_calls: u32,
}
impl FnDevice for QualifiedSession {
    fn read_fn_byte(&mut self) -> Result<FnByteRead, ControlError> {
        let result = self.device.read_fn_byte();
        match &result {
            Ok(read) => self.trace_transfer("read", &read.transfer),
            Err(error) => self.trace_failure("read", "failed", error),
        }
        result
    }
    fn write_fn_byte(&mut self, value: u8) -> WriteSubmission {
        let result = self.device.write_fn_byte(value);
        match &result {
            WriteSubmission::Submitted(Ok(evidence)) => {
                self.trace_transfer("write_submitted", evidence)
            }
            WriteSubmission::Submitted(Err(error)) => {
                self.trace_failure("write", "submitted_failed", error)
            }
            WriteSubmission::NotSubmitted(error) => {
                self.trace_failure("write", "not_submitted", error)
            }
        }
        result
    }
}
impl FnLockIo for QualifiedSession {
    fn revalidate(&mut self) -> Result<SessionIdentity, ControlError> {
        self.driver.revalidate()
    }
    fn status(&self) -> Status {
        self.driver.status()
    }
}
impl QualifiedSession {
    /// At most 32 fixed-call summaries, never a raw buffer or a hardware ACK.
    /// Failed transport evidence remains in its typed error; no lengths/guards
    /// are invented for a call that did not return successful evidence.
    fn trace_line(&mut self, line: String) {
        if self.transport_trace.len() < 32 {
            self.transport_trace.push(line);
        } else {
            self.omitted_transport_calls = self.omitted_transport_calls.saturating_add(1);
        }
    }
    fn trace_transfer(&mut self, operation: &str, e: &TransferEvidence) {
        self.trace_line(format!(
            "Fn transport v1 operation={operation} input_bytes={} output_capacity_bytes={} returned_bytes={} input_guard_intact={} output_guard_intact={} input_unchanged={} output_tail_matches_input={} output_prefix_le=0x{:08x} changed_output_bytes={} first_changed_offset={:?} last_changed_offset={:?}; transfer metadata only, not an EC write acknowledgement",
            e.input_bytes, e.output_capacity_bytes, e.returned_bytes,
            e.input_guard_intact, e.output_guard_intact, e.input_unchanged,
            e.output_tail_matches_input, e.output_prefix_le, e.changed_output_bytes,
            e.first_changed_output_offset, e.last_changed_output_offset,
        ));
    }
    fn trace_failure(&mut self, operation: &str, stage: &str, error: &ControlError) {
        let code: String = error.code.chars().take(96).collect();
        self.trace_line(format!("Fn transport v1 operation={operation} stage={stage} error_code={code}; no successful transfer metadata or hardware acknowledgement inferred"));
    }
    fn open(for_apply: bool) -> Result<Self, identity::QualificationFailure> {
        let mutex = mutex::FnLockMutex::acquire().map_err(identity::QualificationFailure::empty)?;
        if for_apply && mutex.abandoned() {
            return Err(identity::QualificationFailure::empty(ControlError::new("windows_oem_abandoned_requires_probe", "A prior helper abandoned this application mutex; a new authorized Probe is required before Apply")));
        }
        let driver = identity::qualify()?;
        let device = device::Device::open(Arc::clone(&driver))
            .map_err(|e| identity::QualificationFailure::new(driver.status(), e))?;
        // Recheck after opening; no EC call is needed to discover an association
        // change during CreateFile. This is not a kernel atomic binding/CAS.
        driver
            .revalidate()
            .map_err(|e| identity::QualificationFailure::new(driver.status(), e))?;
        Ok(Self {
            device,
            driver,
            _mutex: mutex,
            transport_trace: Vec::new(),
            omitted_transport_calls: 0,
        })
    }
    fn finish(&mut self, mut result: HelperResult) -> HelperResult {
        // Some failure paths have no current observation. Preserve the trace in
        // a partial, unqualified status instead of losing actual call metadata.
        if result.status.is_none() && !self.transport_trace.is_empty() {
            let mut partial = self.driver.status();
            partial.qualified = false;
            partial.control = None;
            partial.diagnostics.push("Identity metadata below belongs to the initial held session; no current hardware observation is claimed for this failure.".into());
            if let Some(error) = result.error.as_ref() {
                partial.blocking_errors.push(error.clone());
            }
            result.status = Some(partial);
        }
        if let Some(status) = result.status.as_mut() {
            status
                .diagnostics
                .extend(self.transport_trace.iter().cloned());
            if self.omitted_transport_calls != 0 {
                status.diagnostics.push(format!(
                    "Fn transport summaries omitted={} after the fixed 32-call limit",
                    self.omitted_transport_calls
                ));
            }
        }
        if let Err(error) = self.device.close() {
            // The hardware stages remain in the receipt. Resource cleanup did
            // fail, so a nominal success is not silently returned as exit zero.
            if let Some(status) = result.status.as_mut() {
                status.control = None;
                status.qualified = false;
                status.blocking_errors.push(error.clone());
                status.diagnostics.push("The fixed device handle close reported a failure after synchronous I/O; hardware stage evidence is retained separately.".into());
            }
            result.result = None;
            if result.receipt.identity_error.is_none() {
                result.receipt.identity_error = Some(error.clone());
            }
            if result.receipt.primary_error.is_none() {
                result.receipt.primary_error = Some(error.clone());
            }
            if let Some(primary) = result.error.as_mut() {
                primary.details.push(format!(
                    "device_close_error={}: {}",
                    error.code, error.message
                ));
            } else {
                result.error = Some(error);
            }
            result.outcome = if result.receipt.write_attempted {
                Outcome::FailedStateUnknown
            } else {
                Outcome::RejectedBeforeWrite
            };
        }
        result
    }
}
fn rejected(failure: identity::QualificationFailure) -> HelperResult {
    HelperResult {
        status: Some(failure.status),
        outcome: Outcome::RejectedBeforeWrite,
        result: None,
        receipt: Receipt::default(),
        error: Some(failure.error),
    }
}

// @mark mechrevo.ec_bridge role=implementation contract=windows-fixed-fnlock
pub(crate) fn probe() -> HelperResult {
    let mut session = match QualifiedSession::open(false) {
        Ok(s) => s,
        Err(e) => return rejected(e),
    };
    let result = transaction::probe(&mut session);
    session.finish(result)
}
pub(crate) fn apply(plan: &OemPlan) -> HelperResult {
    let mut session = match QualifiedSession::open(true) {
        Ok(s) => s,
        Err(e) => return rejected(e),
    };
    let result = transaction::apply(&mut session, plan);
    session.finish(result)
}
