//! Ordinary-user lifetime owner for explicitly authorized one-shot helpers.
//! Construction, cache access and preview never start a process or open the EC.
use super::protocol::{self, Operation, Request, Response};
use super::{ClientHelperLifetime, Outcome, Receipt, Recovery, Status, Update};
use crate::{ControlError, OemPlan, OemValue};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

/// Native transport evidence, not a guessed exit derived from a pipe reply.
/// On a wait failure native transport retains its real handles and buffers.
#[derive(Debug, Clone)]
pub(crate) struct TransportReport {
    pub reply: Option<Vec<u8>>,
    pub exit_code: Option<u32>,
    pub lifetime: ClientHelperLifetime,
    pub error: Option<ControlError>,
}

trait Transport: Send {
    fn exchange(&mut self, request: &Request) -> TransportReport;
    fn finish_pending(&mut self) -> TransportReport;
}

#[cfg(target_os = "windows")]
impl Transport for super::native::ipc::NativeTransport {
    fn exchange(&mut self, request: &Request) -> TransportReport {
        self.exchange(request)
    }
    fn finish_pending(&mut self) -> TransportReport {
        self.finish_pending()
    }
}

#[cfg(not(target_os = "windows"))]
struct UnsupportedTransport;
#[cfg(not(target_os = "windows"))]
impl Transport for UnsupportedTransport {
    fn exchange(&mut self, _request: &Request) -> TransportReport {
        TransportReport {
            reply: None, exit_code: None, lifetime: ClientHelperLifetime::NotStarted,
            error: Some(ControlError::new("windows_fnlock_unsupported", "The authenticated Windows Fn-lock helper is available only on Windows; no fallback is used")),
        }
    }
    fn finish_pending(&mut self) -> TransportReport {
        self.exchange(&Request {
            schema_version: 1,
            request_id: "not-started".into(),
            operation: Operation::Probe,
        })
    }
}

// @mark mechrevo.windows_fnlock role=interface
pub struct Client {
    transport: Box<dyn Transport>,
    cached: Option<Status>,
    needs_probe: bool,
    lifetime: ClientHelperLifetime,
    pending_request: Option<Request>,
    pending_reply: Option<Vec<u8>>,
    pending_reply_conflict: bool,
    pending_errors: Vec<ControlError>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("cached", &self.cached)
            .field("needs_probe", &self.needs_probe)
            .field("lifetime", &self.lifetime)
            .field("pending_request", &self.pending_request)
            .finish_non_exhaustive()
    }
}

impl Default for Client {
    fn default() -> Self {
        #[cfg(target_os = "windows")]
        let transport: Box<dyn Transport> = Box::new(super::native::ipc::NativeTransport::new());
        #[cfg(not(target_os = "windows"))]
        let transport: Box<dyn Transport> = Box::new(UnsupportedTransport);
        Self {
            transport,
            cached: None,
            needs_probe: true,
            lifetime: ClientHelperLifetime::NotStarted,
            pending_request: None,
            pending_reply: None,
            pending_reply_conflict: false,
            pending_errors: Vec::new(),
        }
    }
}

impl Client {
    /// Pure allocation of local state; no installation check or authorization.
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cached_status(&self) -> Option<&Status> {
        self.cached.as_ref()
    }
    pub fn helper_lifetime(&self) -> ClientHelperLifetime {
        self.lifetime
    }
    pub fn helper_reaped(&self) -> bool {
        matches!(
            self.lifetime,
            ClientHelperLifetime::NotStarted | ClientHelperLifetime::Reaped
        )
    }

    pub fn preview(&self, target: bool) -> Result<OemPlan, ControlError> {
        if !self.helper_reaped() {
            return Err(lifetime_error());
        }
        if self.needs_probe {
            return Err(probe_required());
        }
        let status = self.cached.as_ref().ok_or_else(probe_required)?;
        protocol::validate_status(status)?;
        super::preview(status, target)
    }

    /// Explicit user action. It may show UAC and waits for the actual helper.
    pub fn probe_authorized(&mut self) -> Update {
        if !self.helper_reaped() {
            return self.local_rejection(lifetime_error());
        }
        // Even a canceled new check cannot silently reactivate an older plan.
        self.needs_probe = true;
        self.perform(Operation::Probe)
    }

    /// Explicit user action; the exact local preview is checked before UAC.
    pub fn apply_authorized(&mut self, plan: &OemPlan) -> Update {
        if !self.helper_reaped() {
            return self.local_rejection(lifetime_error());
        }
        if let Err(error) = protocol::validate_plan(plan) {
            return self.local_rejection(error);
        }
        let OemValue::Boolean(target) = plan.target else {
            return self.local_rejection(ControlError::new(
                "windows_fnlock_invalid_plan",
                "Fn-lock target must be Boolean",
            ));
        };
        match self.preview(target) {
            Ok(current) if current == *plan => self.perform(Operation::Apply {
                plan: Box::new(plan.clone()),
            }),
            Ok(_) => self.local_rejection(ControlError::new(
                "windows_fnlock_plan_stale",
                "The Fn-lock plan differs from the latest completed check; create a new preview",
            )),
            Err(error) => self.local_rejection(error),
        }
    }

    /// Continue the same retained native pipe/handle, never launch or replay.
    /// There is no timeout or kill path for a potentially active writer.
    pub fn finish_pending(&mut self) -> Update {
        if self.pending_request.is_none() {
            return self.local_rejection(ControlError::new(
                "windows_fnlock_no_pending_helper",
                "No retained helper transaction is available to continue waiting",
            ));
        }
        let report = self.transport.finish_pending();
        self.consume_report(report, false)
    }

    fn perform(&mut self, operation: Operation) -> Update {
        let request = Request {
            schema_version: protocol::SCHEMA_VERSION,
            request_id: next_request_id(),
            operation,
        };
        if let Err(error) = protocol::encode_request(&request) {
            return self.local_rejection(error);
        }
        self.pending_reply = None;
        self.pending_reply_conflict = false;
        self.pending_errors.clear();
        self.pending_request = Some(request.clone());
        let report = self.transport.exchange(&request);
        self.consume_report(report, true)
    }

    fn local_rejection(&self, error: ControlError) -> Update {
        let pending = !self.helper_reaped();
        Update {
            status: self.cached.clone(),
            outcome: if pending {
                Outcome::FailedStateUnknown
            } else {
                Outcome::RejectedBeforeWrite
            },
            result: None,
            receipt: Receipt {
                writer_may_continue: pending,
                primary_error: Some(error.clone()),
                recovery: if pending {
                    Recovery::Unknown
                } else {
                    Recovery::NotNeeded
                },
                ..Receipt::default()
            },
            error: Some(error),
            helper_reaped: self.helper_reaped(),
            helper_lifetime: self.lifetime,
        }
    }

    fn consume_report(&mut self, mut report: TransportReport, new_exchange: bool) -> Update {
        let Some(request) = self.pending_request.clone() else {
            // This is an internal invariant failure, not proof of native exit.
            self.lifetime = ClientHelperLifetime::Unknown;
            self.needs_probe = true;
            return self.local_rejection(ControlError::new(
                "windows_fnlock_pending_lost",
                "Native completion has no matching retained request",
            ));
        };
        if let Some(reply) = report.reply.take() {
            if self
                .pending_reply
                .as_ref()
                .is_some_and(|earlier| earlier != &reply)
            {
                self.pending_reply_conflict = true;
                report.error = Some(ControlError::new("windows_fnlock_response_changed", "The retained one-shot response changed while waiting; no second response is accepted"));
            } else {
                self.pending_reply = Some(reply);
            }
        }
        if let Some(error) = &report.error {
            if !self.pending_errors.iter().any(|e| {
                e.code == error.code && e.message == error.message && e.details == error.details
            }) {
                self.pending_errors.push(error.clone());
            }
        }
        if report.lifetime == ClientHelperLifetime::NotStarted
            && (!new_exchange || self.pending_reply.is_some() || report.exit_code.is_some())
        {
            // Losing a handle during resume is not evidence that it never ran.
            report.lifetime = ClientHelperLifetime::Unknown;
            report.error = Some(ControlError::new("windows_fnlock_helper_liveness_unknown", "Not-started evidence contradicts a retained transaction or response; no new helper may be started"));
        }
        self.lifetime = report.lifetime;
        if self.pending_reply_conflict && report.error.is_none() {
            report.error = Some(ControlError::new(
                "windows_fnlock_response_changed",
                "Conflicting response evidence cannot be repaired by a later successful wait",
            ));
        }
        if !self.helper_reaped() {
            self.needs_probe = true;
            let mut error = report.error.unwrap_or_else(lifetime_error);
            append_prior_errors(&mut error, &self.pending_errors);
            let staged = self
                .pending_reply
                .as_deref()
                .and_then(|bytes| protocol::decode_response(bytes).ok())
                .filter(|response| protocol::validate_relation(&request, response).is_ok());
            let mut receipt = if let Some(response) = staged {
                if let Some(primary) = &response.error {
                    error
                        .details
                        .push(format!("helper_error={}", serialized_error(primary)));
                }
                response.receipt
            } else {
                Receipt { recovery:Recovery::Unknown,primary_error:Some(error.clone()),
                    recovery_reason:Some("Helper exit is not confirmed. Missing stage evidence does not mean that no hardware write occurred.".into()),..Receipt::default() }
            };
            receipt.writer_may_continue = true;
            let status = self.invalidate_cached(&error);
            return Update {
                status,
                outcome: Outcome::FailedStateUnknown,
                result: None,
                receipt,
                error: Some(error),
                helper_reaped: false,
                helper_lifetime: self.lifetime,
            };
        }
        if self.lifetime == ClientHelperLifetime::NotStarted {
            let error = report.error.unwrap_or_else(|| {
                ControlError::new(
                    "windows_fnlock_helper_not_started",
                    "The fixed authorized helper did not start",
                )
            });
            self.pending_request = None;
            self.pending_reply = None;
            self.pending_errors.clear();
            self.needs_probe = true;
            return self.local_rejection(error);
        }

        let decoded = self
            .pending_reply
            .as_deref()
            .ok_or_else(|| {
                ControlError::new(
                    "windows_fnlock_response_missing",
                    "The helper exited without a complete bounded response",
                )
            })
            .and_then(protocol::decode_response)
            .and_then(|response| {
                protocol::validate_relation(&request, &response)?;
                Ok(response)
            });
        if let Ok(response) = &decoded {
            let actual_exit = report
                .exit_code
                .ok_or_else(|| {
                    ControlError::new(
                        "windows_fnlock_exit_unknown",
                        "Reap evidence did not include the actual helper exit code",
                    )
                })
                .and_then(|exit| protocol::validate_exchange(&request, response, exit));
            if let Err(error) = actual_exit {
                if let Some(primary) = report.error.as_mut() {
                    primary.details.push(format!(
                        "exit_validation_error={}",
                        serialized_error(&error)
                    ));
                } else {
                    report.error = Some(error);
                }
            }
        }
        let update = match decoded {
            Ok(mut response) if report.error.is_none() => {
                // Earlier failed waits remain diagnostics after a later actual
                // wait proves exit; they are never silently called cancellation.
                if let Some(status) = response.status.as_mut() {
                    for error in &self.pending_errors {
                        status.diagnostics.push(format!(
                            "Earlier IPC diagnostic; helper subsequently reaped: {}",
                            serialized_error(error)
                        ));
                    }
                }
                self.absorb_verified_response(&request, response)
            }
            Ok(response) => {
                let mut error = report.error.unwrap_or_else(|| {
                    ControlError::new(
                        "windows_fnlock_transport_failed",
                        "Native transport failed after receiving stage evidence",
                    )
                });
                append_prior_errors(&mut error, &self.pending_errors);
                if let Some(primary) = &response.error {
                    error
                        .details
                        .push(format!("helper_error={}", serialized_error(primary)));
                }
                if let Some(status) = &response.status {
                    self.cached = Some(status.clone());
                }
                self.needs_probe = true;
                let status = self.invalidate_cached(&error);
                Update {
                    status,
                    outcome: Outcome::FailedStateUnknown,
                    result: None,
                    receipt: response.receipt,
                    error: Some(error),
                    helper_reaped: true,
                    helper_lifetime: ClientHelperLifetime::Reaped,
                }
            }
            Err(mut error) => {
                if let Some(transport) = report.error {
                    error
                        .details
                        .push(format!("transport_error={}", serialized_error(&transport)));
                }
                append_prior_errors(&mut error, &self.pending_errors);
                if let Some(bytes) = self.pending_reply.as_deref() {
                    // Preserve exact byte identity without echoing unexpected
                    // privileged data into the public UI or diagnostics.
                    error
                        .details
                        .push(format!("unverified_response_bytes={}", bytes.len()));
                    error.details.push(format!(
                        "unverified_response_sha256={:x}",
                        Sha256::digest(bytes)
                    ));
                }
                self.needs_probe = true;
                let status = self.invalidate_cached(&error);
                Update { status, outcome: Outcome::FailedStateUnknown, result: None,
                    receipt: Receipt { recovery: Recovery::Unknown, primary_error: Some(error.clone()),
                        recovery_reason: Some("No authenticated, matching terminal response and actual exit combination could be verified. No write was retried or restored by the client.".into()), ..Receipt::default() },
                    error: Some(error), helper_reaped: true, helper_lifetime: ClientHelperLifetime::Reaped }
            }
        };
        self.pending_request = None;
        self.pending_reply = None;
        self.pending_errors.clear();
        update
    }

    fn absorb_verified_response(&mut self, request: &Request, response: Response) -> Update {
        if let Some(status) = &response.status {
            self.cached = Some(status.clone());
        }
        self.needs_probe = match response.outcome {
            Outcome::Observed => {
                !matches!(request.operation, Operation::Probe)
                    || response.status.as_ref().is_none_or(|s| {
                        !s.qualified || s.control.is_none() || !s.blocking_errors.is_empty()
                    })
            }
            Outcome::AppliedVerified => false,
            _ => true,
        };
        if response.receipt.blocks_further_writes() {
            self.needs_probe = true;
        }
        Update {
            status: response.status,
            outcome: response.outcome,
            result: response.result,
            receipt: response.receipt,
            error: response.error,
            helper_reaped: true,
            helper_lifetime: ClientHelperLifetime::Reaped,
        }
    }

    fn invalidate_cached(&mut self, error: &ControlError) -> Option<Status> {
        if let Some(status) = self.cached.as_mut() {
            status.qualified = false;
            status.control = None;
            if !status
                .blocking_errors
                .iter()
                .any(|e| e.code == "windows_fnlock_probe_required")
            {
                let mut blocked = probe_required();
                blocked.details.push(format!(
                    "unconfirmed_transaction_error={}",
                    serialized_error(error)
                ));
                status.blocking_errors.push(blocked);
            }
        }
        self.cached.clone()
    }
}

fn lifetime_error() -> ControlError {
    ControlError::new("windows_fnlock_helper_liveness_unknown", "The previous helper has not been confirmed exited. Continue waiting for that helper; a new check or write cannot start")
}
fn probe_required() -> ControlError {
    ControlError::new(
        "windows_fnlock_probe_required",
        "Complete a new successful authorized Windows Fn-lock check before preview or apply",
    )
}
fn serialized_error(error: &ControlError) -> String {
    serde_json::to_string(error).unwrap_or_else(|_| "error serialization unavailable".into())
}
fn append_prior_errors(error: &mut ControlError, earlier: &[ControlError]) {
    for prior in earlier {
        if prior.code != error.code
            || prior.message != error.message
            || prior.details != error.details
        {
            error.details.push(format!(
                "earlier_transport_error={}",
                serialized_error(prior)
            ));
        }
    }
}
fn next_request_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "windows-fn-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::super::{
        contract, HelperResult, IdentityEvidence, LoadedModuleEvidence, SupportStage,
    };
    use super::*;
    use crate::{
        CapabilityStatus, EvidenceSource, OemApplyResult, OemControl, OemControlKind, OemDomain,
    };
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    fn status(current: bool) -> Status {
        Status {
            contract_id: Some("accepted-fixture".into()),
            support_stage: Some(SupportStage::RuntimeAccepted),
            qualified: true,
            control: Some(OemControl {
                id: contract::CONTROL_ID.into(),
                kind: OemControlKind::FnLock,
                label: contract::CONTROL_LABEL.into(),
                current: Some(OemValue::Boolean(current)),
                domain: OemDomain::Boolean,
                status: CapabilityStatus::Available,
                writable: true,
                source: EvidenceSource {
                    backend: contract::SOURCE_BACKEND.into(),
                    path: contract::SOURCE_PATH.into(),
                    device_id: Some("ACPI\\INOU0000\\fixture".into()),
                },
                binding: if current {
                    "b".repeat(64)
                } else {
                    "a".repeat(64)
                },
                readback_scope: contract::READBACK_SCOPE.into(),
                reason: None,
            }),
            evidence: IdentityEvidence {
                disk_file_sha256: Some(
                    "6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235".into(),
                ),
                loaded_module_metadata: Some(LoadedModuleEvidence {
                    service: "UWACPIDriver".into(),
                    device_instance_id: "ACPI\\INOU0000\\fixture".into(),
                    driver_inf: "oem1.inf".into(),
                    driver_version: "10.48.11.331".into(),
                    loaded_path: "C:\\Windows\\System32\\drivers\\UWACPIDriver.sys".into(),
                    association_level: "offline-test".into(),
                }),
                load_time_provenance: None,
                current_memory_measurement: None,
                runtime_acceptance_id: Some("offline-test-only".into()),
            },
            blocking_errors: vec![],
            diagnostics: vec![],
        }
    }
    fn observed(req: &Request) -> TransportReport {
        let response = Response::from_result(
            req.request_id.clone(),
            HelperResult {
                status: Some(status(false)),
                outcome: Outcome::Observed,
                result: None,
                receipt: Receipt::default(),
                error: None,
            },
        );
        TransportReport {
            reply: Some(protocol::encode_response(&response).unwrap()),
            exit_code: Some(0),
            lifetime: ClientHelperLifetime::Reaped,
            error: None,
        }
    }
    fn applied(req: &Request) -> TransportReport {
        let Operation::Apply { plan } = &req.operation else {
            panic!("fixture expected apply")
        };
        let response = Response::from_result(
            req.request_id.clone(),
            HelperResult {
                status: Some(status(true)),
                outcome: Outcome::AppliedVerified,
                result: Some(OemApplyResult {
                    control_id: plan.control_id.clone(),
                    kind: plan.kind,
                    previous: plan.previous.clone(),
                    confirmed: plan.target.clone(),
                    source: plan.source.clone(),
                    readback_scope: plan.readback_scope.clone(),
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
        TransportReport {
            reply: Some(protocol::encode_response(&response).unwrap()),
            exit_code: Some(0),
            lifetime: ClientHelperLifetime::Reaped,
            error: None,
        }
    }
    enum Step {
        Probe,
        Apply,
        Malformed,
        NotStarted,
        ApplyPending,
        ApplyBadExit(Option<u32>),
        ContradictoryNotStarted,
    }
    struct FakeTransport {
        steps: VecDeque<Step>,
        calls: Arc<Mutex<Vec<&'static str>>>,
        pending: Option<TransportReport>,
    }
    impl Transport for FakeTransport {
        fn exchange(&mut self, req: &Request) -> TransportReport {
            self.calls.lock().unwrap().push("exchange");
            match self.steps.pop_front().expect("unexpected helper launch") {
                Step::Probe => observed(req),
                Step::Apply => applied(req),
                Step::ApplyBadExit(exit_code) => TransportReport {
                    exit_code,
                    ..applied(req)
                },
                Step::ContradictoryNotStarted => TransportReport {
                    lifetime: ClientHelperLifetime::NotStarted,
                    ..observed(req)
                },
                Step::Malformed => TransportReport {
                    reply: Some(b"{broken".to_vec()),
                    exit_code: Some(0),
                    lifetime: ClientHelperLifetime::Reaped,
                    error: None,
                },
                Step::NotStarted => TransportReport {
                    reply: None,
                    exit_code: None,
                    lifetime: ClientHelperLifetime::NotStarted,
                    error: Some(ControlError::new(
                        "windows_fnlock_uac_cancelled",
                        "Explicit UAC cancellation before helper launch",
                    )),
                },
                Step::ApplyPending => {
                    let complete = applied(req);
                    self.pending = Some(complete.clone());
                    TransportReport {
                        exit_code: None,
                        lifetime: ClientHelperLifetime::Unknown,
                        error: Some(ControlError::new(
                            "windows_fnlock_wait_failed",
                            "Simulated native wait failure; handle retained",
                        )),
                        ..complete
                    }
                }
            }
        }
        fn finish_pending(&mut self) -> TransportReport {
            self.calls.lock().unwrap().push("finish");
            self.pending.take().expect("no retained helper")
        }
    }
    fn client(steps: Vec<Step>) -> (Client, Arc<Mutex<Vec<&'static str>>>) {
        let calls = Arc::new(Mutex::new(vec![]));
        let mut c = Client::new();
        c.transport = Box::new(FakeTransport {
            steps: steps.into(),
            calls: calls.clone(),
            pending: None,
        });
        (c, calls)
    }

    #[test]
    fn construction_cache_and_local_preview_do_not_start_authorization() {
        let (c, calls) = client(vec![]);
        assert!(c.cached_status().is_none());
        assert!(c.helper_reaped());
        assert_eq!(c.helper_lifetime(), ClientHelperLifetime::NotStarted);
        assert!(c.preview(true).is_err());
        assert!(calls.lock().unwrap().is_empty());
    }
    #[test]
    fn real_client_path_probes_locally_previews_applies_and_invalidates_old_plan() {
        let (mut c, calls) = client(vec![Step::Probe, Step::Apply]);
        assert_eq!(c.probe_authorized().outcome, Outcome::Observed);
        let plan = c.preview(true).unwrap();
        assert_eq!(calls.lock().unwrap().len(), 1);
        let update = c.apply_authorized(&plan);
        assert_eq!(update.outcome, Outcome::AppliedVerified);
        assert!(update.helper_reaped);
        assert_eq!(
            c.cached_status().unwrap().control.as_ref().unwrap().current,
            Some(OemValue::Boolean(true))
        );
        assert_ne!(c.preview(false).unwrap().binding, plan.binding);
        assert_eq!(
            c.apply_authorized(&plan).outcome,
            Outcome::RejectedBeforeWrite
        );
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
    #[test]
    fn malformed_terminal_result_requires_fresh_probe_and_never_replays_apply() {
        let (mut c, calls) = client(vec![Step::Probe, Step::Malformed, Step::Probe]);
        c.probe_authorized();
        let plan = c.preview(true).unwrap();
        let failed = c.apply_authorized(&plan);
        assert_eq!(failed.outcome, Outcome::FailedStateUnknown);
        assert!(failed.helper_reaped);
        assert!(c.preview(true).is_err());
        c.apply_authorized(&plan);
        assert_eq!(calls.lock().unwrap().len(), 2);
        assert_eq!(c.probe_authorized().outcome, Outcome::Observed);
        assert!(c.preview(true).is_ok());
        assert_eq!(calls.lock().unwrap().len(), 3);
    }
    #[test]
    fn unknown_lifetime_is_sticky_until_same_helper_is_actually_reaped() {
        let (mut c, calls) = client(vec![Step::Probe, Step::ApplyPending]);
        c.probe_authorized();
        let plan = c.preview(true).unwrap();
        let pending = c.apply_authorized(&plan);
        assert!(!pending.helper_reaped);
        assert!(pending.receipt.writer_may_continue);
        assert_eq!(c.helper_lifetime(), ClientHelperLifetime::Unknown);
        assert_eq!(pending.receipt.full_byte_verified, Some(true));
        assert!(pending.receipt.write_attempted);
        assert!(!pending.status.as_ref().unwrap().qualified);
        assert!(pending.status.as_ref().unwrap().control.is_none());
        assert!(!c.probe_authorized().helper_reaped);
        assert!(!c.apply_authorized(&plan).helper_reaped);
        assert!(c.preview(true).is_err());
        assert_eq!(calls.lock().unwrap().len(), 2);
        let finished = c.finish_pending();
        assert_eq!(finished.outcome, Outcome::AppliedVerified);
        assert!(finished.helper_reaped);
        assert_eq!(finished.helper_lifetime, ClientHelperLifetime::Reaped);
        assert!(!finished.status.unwrap().diagnostics.is_empty());
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["exchange", "exchange", "finish"]
        );
    }
    #[test]
    fn explicit_cancellation_is_before_write_and_no_pending_finish_is_created() {
        let (mut c, calls) = client(vec![Step::NotStarted]);
        let update = c.probe_authorized();
        assert_eq!(update.outcome, Outcome::RejectedBeforeWrite);
        assert!(update.helper_reaped);
        assert_eq!(update.helper_lifetime, ClientHelperLifetime::NotStarted);
        assert!(!update.receipt.write_attempted);
        assert_eq!(
            c.finish_pending().error.unwrap().code,
            "windows_fnlock_no_pending_helper"
        );
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
    #[test]
    fn valid_stage_receipt_survives_exit_contradiction_and_missing_exit() {
        for exit in [Some(3), None] {
            let (mut c, calls) = client(vec![Step::Probe, Step::ApplyBadExit(exit)]);
            c.probe_authorized();
            let plan = c.preview(true).unwrap();
            let update = c.apply_authorized(&plan);
            assert_eq!(update.outcome, Outcome::FailedStateUnknown);
            assert!(update.helper_reaped);
            assert!(update.receipt.write_attempted);
            assert_eq!(update.receipt.full_byte_verified, Some(true));
            assert_eq!(update.receipt.protected_bits_preserved, Some(true));
            assert!(!update.status.as_ref().unwrap().qualified);
            assert!(update.status.as_ref().unwrap().control.is_none());
            assert!(c.preview(true).is_err());
            c.apply_authorized(&plan);
            assert_eq!(calls.lock().unwrap().len(), 2);
        }
    }
    #[test]
    fn response_evidence_cannot_be_reclassified_as_a_before_launch_cancellation() {
        let (mut c, calls) = client(vec![Step::ContradictoryNotStarted]);
        let update = c.probe_authorized();
        assert_eq!(update.outcome, Outcome::FailedStateUnknown);
        assert!(!update.helper_reaped);
        assert!(update.receipt.writer_may_continue);
        assert!(!c.probe_authorized().helper_reaped);
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
}
