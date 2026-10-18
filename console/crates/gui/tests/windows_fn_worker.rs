//! Production Controller worker tests with synthetic Windows Fn DTOs only.
//! No fixture instantiates a native client, authorizes a helper or touches EC.
use mechrevo_console_core::{
    brightness::BrightnessBackend,
    cpu_power::{Action as CpuAction, Plan as CpuPlan},
    fan_curve::{client::FanClientUpdate, FanCurve, FanCurveChannel, FanCurveLayout},
    windows_oem::{
        contract, ClientHelperLifetime as Lifetime, IdentityEvidence, LoadedModuleEvidence,
        Outcome, Receipt, Recovery, Status, SupportStage, Update,
    },
    ApplyResult, BrightnessPlan, CapabilityStatus, CatalogModel, ControlError, EvidenceSource,
    OemApplyResult, OemControl, OemControlKind, OemDomain, OemPlan, OemValue, PowerProfile,
    ProfilePlan, Snapshot, ThermalSample,
};
use mechrevo_console_gui::{Backend, Controller, SubmitError};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Probe,
    Apply,
    Finish,
    Snapshot,
    Catalog,
    Thermal,
    LegacyPlan,
    LegacyApply,
    ProbeFan,
    Shutdown,
    DropStarted,
    Drop,
}
#[derive(Default)]
struct Audit {
    calls: Vec<Call>,
    submitted: Vec<OemPlan>,
    violations: Vec<String>,
}
type SharedAudit = Arc<Mutex<Audit>>;
fn record(audit: &SharedAudit, call: Call) {
    audit.lock().unwrap().calls.push(call);
}
fn calls(audit: &SharedAudit) -> Vec<Call> {
    let audit = audit.lock().unwrap();
    assert!(
        audit.violations.is_empty(),
        "fake protocol violation: {:?}",
        audit.violations
    );
    audit.calls.clone()
}
fn count(audit: &SharedAudit, call: Call) -> usize {
    calls(audit)
        .into_iter()
        .filter(|item| *item == call)
        .count()
}
struct Gate {
    entered: SyncSender<()>,
    release: Receiver<()>,
}
struct GateControl {
    entered: Receiver<()>,
    release: SyncSender<()>,
}
fn gate() -> (Gate, GateControl) {
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, delayed) = mpsc::sync_channel(1);
    (
        Gate {
            entered,
            release: delayed,
        },
        GateControl {
            entered: ready,
            release,
        },
    )
}
impl GateControl {
    fn wait(&self) {
        self.entered
            .recv_timeout(Duration::from_secs(2))
            .expect("worker did not enter scripted stage");
    }
    fn release(&self) {
        self.release.send(()).expect("worker exited before release");
    }
}
fn block(gate: Gate) {
    gate.entered.send(()).unwrap();
    gate.release
        .recv_timeout(Duration::from_secs(2))
        .expect("test did not release worker stage");
}
enum Completion {
    Reply(Box<Update>),
    Panic,
}
struct Step {
    call: Call,
    completion: Completion,
    gate: Option<Gate>,
}
fn step(call: Call, update: Update) -> Step {
    Step {
        call,
        completion: Completion::Reply(Box::new(update)),
        gate: None,
    }
}
struct FnBackend {
    audit: SharedAudit,
    steps: VecDeque<Step>,
    drop_gate: Option<Gate>,
}
impl FnBackend {
    fn call(&mut self, call: Call) -> Update {
        record(&self.audit, call);
        let Some(step) = self.steps.pop_front() else {
            self.audit
                .lock()
                .unwrap()
                .violations
                .push(format!("unexpected {call:?}"));
            panic!("unexpected Windows Fn operation");
        };
        if step.call != call {
            self.audit
                .lock()
                .unwrap()
                .violations
                .push(format!("expected {:?}, got {call:?}", step.call));
            panic!("Windows Fn operations reordered");
        }
        if let Some(gate) = step.gate {
            block(gate);
        }
        match step.completion {
            Completion::Reply(update) => *update,
            Completion::Panic => panic!("scripted Windows Fn worker panic"),
        }
    }
}
impl Backend for FnBackend {
    fn snapshot(&mut self) -> Snapshot {
        record(&self.audit, Call::Snapshot);
        snapshot()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        record(&self.audit, Call::Catalog);
        vec![]
    }
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
        record(&self.audit, Call::Thermal);
        let time = Instant::now();
        Ok(ThermalSample {
            readings: vec![],
            diagnostics: vec![],
            started_at: time,
            finished_at: time,
        })
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        self.audit
            .lock()
            .unwrap()
            .violations
            .push("unexpected OS profile plan".into());
        Err(error("fixture_forbidden"))
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        self.audit
            .lock()
            .unwrap()
            .violations
            .push("unexpected OS profile write".into());
        Err(error("fixture_forbidden"))
    }
    fn plan_oem(&mut self, _: &str, _: OemValue) -> Result<OemPlan, ControlError> {
        record(&self.audit, Call::LegacyPlan);
        Err(error("fixture_forbidden"))
    }
    fn apply_oem(&mut self, _: &OemPlan) -> Result<OemApplyResult, ControlError> {
        record(&self.audit, Call::LegacyApply);
        Err(error("fixture_forbidden"))
    }
    fn windows_fn_probe(&mut self) -> Update {
        self.call(Call::Probe)
    }
    fn windows_fn_apply(&mut self, plan: &OemPlan) -> Update {
        self.audit.lock().unwrap().submitted.push(plan.clone());
        self.call(Call::Apply)
    }
    fn windows_fn_finish_pending(&mut self) -> Update {
        self.call(Call::Finish)
    }
    fn fan_probe(&mut self) -> FanClientUpdate {
        record(&self.audit, Call::ProbeFan);
        FanClientUpdate {
            session_held: true,
            helper_reaped: false,
            cleanup_complete: false,
            ..Default::default()
        }
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        record(&self.audit, Call::Shutdown);
        FanClientUpdate::default()
    }
}
impl Drop for FnBackend {
    fn drop(&mut self) {
        record(&self.audit, Call::DropStarted);
        if let Some(gate) = self.drop_gate.take() {
            block(gate);
        }
        record(&self.audit, Call::Drop);
    }
}
fn fixture(steps: Vec<Step>) -> (Controller, SharedAudit) {
    fixture_with_drop_gate(steps, None)
}
fn fixture_with_drop_gate(steps: Vec<Step>, drop_gate: Option<Gate>) -> (Controller, SharedAudit) {
    let audit = Arc::new(Mutex::new(Audit::default()));
    (
        Controller::new(FnBackend {
            audit: audit.clone(),
            steps: steps.into(),
            drop_gate,
        })
        .unwrap(),
        audit,
    )
}
fn until(controller: &mut Controller, condition: impl Fn(&Controller) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !condition(controller) && Instant::now() < deadline {
        controller.poll();
        std::thread::yield_now();
    }
    assert!(
        condition(controller),
        "production worker did not reach expected state"
    );
}
fn finish(controller: &mut Controller) {
    until(controller, |controller| controller.activity().is_none());
}
fn probe(controller: &mut Controller) {
    controller.probe_windows_fn().unwrap();
    finish(controller);
}
fn preview(controller: &mut Controller, target: bool) {
    controller.set_windows_fn_target(target);
    controller.preview_windows_fn().unwrap();
    assert!(controller.windows_fn_preview.is_some());
}
fn apply(controller: &mut Controller) {
    controller.apply_windows_fn_preview().unwrap();
    finish(controller);
    assert!(controller.windows_fn_preview.is_none());
}
fn close(controller: &mut Controller) {
    controller.request_close();
    until(controller, Controller::close_ready);
}
fn error(code: &str) -> ControlError {
    ControlError::new(code, "synthetic Windows Fn boundary failure")
}
fn status(current: bool, binding: char) -> Status {
    let instance = "ACPI\\INOU0000\\GUI-WORKER-OFFLINE-FIXTURE";
    Status {
        contract_id: Some("offline-worker-only".into()),
        support_stage: Some(SupportStage::Candidate),
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
                device_id: Some(instance.into()),
            },
            binding: binding.to_string().repeat(64),
            readback_scope: contract::READBACK_SCOPE.into(),
            reason: Some("Synthetic fixture; no Windows runtime acceptance".into()),
        }),
        evidence: IdentityEvidence {
            disk_file_sha256: Some(
                "6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235".into(),
            ),
            loaded_module_metadata: Some(LoadedModuleEvidence {
                service: "UWACPIDriver".into(),
                device_instance_id: instance.into(),
                driver_inf: "oem-gui-offline-fixture.inf".into(),
                driver_version: "10.48.11.331".into(),
                loaded_path: "C:\\Windows\\System32\\drivers\\UWACPIDriver.sys".into(),
                association_level:
                    "Synthetic GUI worker fixture, not a Windows observation or acceptance".into(),
            }),
            ..Default::default()
        },
        diagnostics: vec![
            "Synthetic DTO only; populated identity values are not measured hardware evidence"
                .into(),
        ],
        ..Default::default()
    }
}
fn observed(current: bool, binding: char) -> Update {
    Update {
        status: Some(status(current, binding)),
        outcome: Outcome::Observed,
        result: None,
        receipt: Receipt::default(),
        error: None,
        helper_reaped: true,
        helper_lifetime: Lifetime::Reaped,
    }
}
fn applied(previous: bool, target: bool, binding: char) -> Update {
    let control = status(target, binding).control.unwrap();
    Update {
        status: Some(status(target, binding)),
        outcome: Outcome::AppliedVerified,
        result: Some(OemApplyResult {
            control_id: control.id,
            kind: control.kind,
            previous: OemValue::Boolean(previous),
            confirmed: OemValue::Boolean(target),
            source: control.source,
            readback_scope: control.readback_scope,
        }),
        receipt: Receipt {
            write_attempted: true,
            write_call_succeeded: Some(true),
            full_byte_verified: Some(true),
            protected_bits_preserved: Some(true),
            ..Default::default()
        },
        error: None,
        helper_reaped: true,
        helper_lifetime: Lifetime::Reaped,
    }
}
fn unknown(lifetime: Lifetime) -> Update {
    let primary = error("fixture_primary");
    Update {
        status: Some(status(false, 'a')),
        outcome: Outcome::FailedStateUnknown,
        result: None,
        receipt: Receipt {
            write_attempted: true,
            recovery: Recovery::Failed,
            writer_may_continue: matches!(lifetime, Lifetime::Running | Lifetime::Unknown),
            primary_error: Some(primary.clone()),
            restore_error: Some(error("fixture_restore")),
            readback_errors: vec![error("fixture_readback")],
            recovery_reason: Some("retain independent error stages".into()),
            ..Default::default()
        },
        error: Some(primary),
        helper_reaped: lifetime == Lifetime::Reaped,
        helper_lifetime: lifetime,
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        platform: "windows".into(),
        identity: Default::default(),
        telemetry: vec![],
        capabilities: vec![],
        power_policy: Default::default(),
        diagnostics: vec![],
        brightness: vec![],
        oem_controls: vec![],
    }
}
fn profile_plan() -> ProfilePlan {
    ProfilePlan {
        requested: PowerProfile::Performance,
        target: "fixture".into(),
        previous: "fixture".into(),
        scope: "fixture".into(),
        details: None,
        source: EvidenceSource {
            backend: "fixture".into(),
            path: "fixture://no-hardware".into(),
            device_id: None,
        },
    }
}
fn brightness_plan() -> BrightnessPlan {
    BrightnessPlan {
        device_id: "fixture-panel".into(),
        requested_percent: 60,
        target: 60,
        previous: 50,
        maximum: 100,
        source: profile_plan().source,
        backend: BrightnessBackend::WindowsWmi,
        logind_session: None,
    }
}
fn fan_curve() -> FanCurve {
    FanCurve {
        schema_version: 1,
        layout: FanCurveLayout::UniwillRamFan1p5,
        respective: true,
        cpu: FanCurveChannel {
            rise_celsius: std::array::from_fn(|i| 35 + i as u8 * 5),
            fall_celsius: std::array::from_fn(|i| 40 + i as u8 * 5),
            duty_raw: [51; 16],
        },
        gpu: FanCurveChannel {
            rise_celsius: [255; 15],
            fall_celsius: [255; 15],
            duty_raw: [200; 16],
        },
    }
}

#[test]
fn ordinary_refresh_and_cached_preview_never_authorize_and_fn_cache_has_no_legacy_oem_bypass() {
    let (mut controller, audit) = fixture(vec![
        step(Call::Probe, observed(false, 'a')),
        step(Call::Apply, applied(false, true, 'b')),
    ]);
    assert!(calls(&audit).is_empty());
    controller.refresh().unwrap();
    finish(&mut controller);
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert_eq!(count(&audit, Call::Probe), 0);
    assert_eq!(count(&audit, Call::Apply), 0);
    assert_eq!(
        controller.finish_windows_fn_pending(),
        Err(SubmitError::Unsupported)
    );
    probe(&mut controller);
    assert!(controller.windows_fn_visible() && controller.windows_fn_lifetime_clean());
    assert!(!controller.windows_fn_needs_probe);
    assert!(controller
        .snapshot
        .as_ref()
        .unwrap()
        .oem_controls
        .is_empty());
    controller.selected_oem_control = Some(contract::CONTROL_ID.into());
    controller.requested_oem_value = Some(OemValue::Boolean(true));
    assert!(controller.preview_oem().is_err());
    let before = calls(&audit);
    preview(&mut controller, true);
    assert_eq!(
        calls(&audit),
        before,
        "local Windows Fn preview invoked a backend"
    );
    assert_eq!(
        controller.windows_fn_preview.as_ref().unwrap().binding,
        "a".repeat(64)
    );
    apply(&mut controller);
    assert_eq!(
        controller.windows_fn_outcome,
        Some(Outcome::AppliedVerified)
    );
    assert_eq!(
        controller.windows_fn_result.as_ref().unwrap().confirmed,
        OemValue::Boolean(true)
    );
    assert!(!controller.windows_fn_state_unknown && controller.windows_fn_lifetime_clean());
    assert_eq!(
        controller.apply_windows_fn_preview(),
        Err(SubmitError::PreviewRequired)
    );
    assert_eq!(count(&audit, Call::LegacyPlan), 0);
    assert_eq!(count(&audit, Call::LegacyApply), 0);
    close(&mut controller);
    assert_eq!(
        count(&audit, Call::Apply),
        1,
        "close changed persistent Fn state"
    );
}

struct NoFnBackend(SharedAudit);
impl Backend for NoFnBackend {
    fn snapshot(&mut self) -> Snapshot {
        record(&self.0, Call::Snapshot);
        snapshot()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        record(&self.0, Call::Catalog);
        vec![]
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        Err(error("fixture_no_fallback"))
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        Err(error("fixture_no_fallback"))
    }
}

#[test]
fn backend_defaults_report_unsupported_without_full_snapshot_or_legacy_fallback() {
    let audit = Arc::new(Mutex::new(Audit::default()));
    let mut controller = Controller::new(NoFnBackend(audit.clone())).unwrap();
    probe(&mut controller);
    assert!(controller.windows_fn_error.is_some());
    assert!(controller.windows_fn_needs_probe && controller.windows_fn_lifetime_clean());
    assert!(controller.windows_fn_status.is_none());
    assert!(
        calls(&audit).is_empty(),
        "default Windows probe invoked unrelated inspection"
    );
    assert_eq!(
        controller.finish_windows_fn_pending(),
        Err(SubmitError::Unsupported)
    );
    close(&mut controller);
}

#[test]
fn windows_fn_plan_rejects_reverse_preview_paths_without_discarding_its_binding() {
    let (mut controller, audit) = fixture(vec![step(Call::Probe, observed(false, 'a'))]);
    probe(&mut controller);
    controller.set_fan_draft(fan_curve()).unwrap();
    preview(&mut controller, true);
    let plan = controller.windows_fn_preview.clone();
    let before = calls(&audit);
    assert_eq!(controller.preview_fan(), Err(SubmitError::Busy));
    assert!(controller.preview_cpu_power(CpuAction::Clear).is_err());
    assert!(controller.preview_profile().is_err());
    assert!(controller.preview_brightness().is_err());
    assert!(controller.preview_oem().is_err());
    assert_eq!(controller.windows_fn_preview, plan);
    assert!(controller.fan_preview.is_none() && controller.cpu_power_preview.is_none());
    assert!(
        controller.preview.is_none()
            && controller.brightness_preview.is_none()
            && controller.oem_preview.is_none()
    );
    assert_eq!(calls(&audit), before);
    close(&mut controller);
}

#[test]
fn every_foreign_preview_rejects_windows_fn_operations_without_discarding_any_plan() {
    for kind in 0..5 {
        let (mut controller, audit) = fixture(vec![step(Call::Probe, observed(false, 'a'))]);
        probe(&mut controller);
        preview(&mut controller, true);
        match kind {
            0 => controller.preview = Some(profile_plan()),
            1 => controller.brightness_preview = Some(brightness_plan()),
            2 => {
                controller.oem_preview = Some(
                    mechrevo_console_core::windows_oem::preview(&status(false, 'a'), false)
                        .unwrap(),
                )
            }
            3 => controller.fan_preview = Some(fan_curve().validate().unwrap()),
            4 => {
                controller.cpu_power_preview = Some(CpuPlan {
                    revision: "c".repeat(64),
                    action: CpuAction::Clear,
                })
            }
            _ => unreachable!(),
        }
        let old = (
            json!(controller.preview),
            json!(controller.brightness_preview),
            json!(controller.oem_preview),
            controller.fan_preview.clone(),
            controller.cpu_power_preview.clone(),
            controller.windows_fn_preview.clone(),
        );
        let before = calls(&audit);
        assert_eq!(controller.probe_windows_fn(), Err(SubmitError::Busy));
        assert_eq!(controller.preview_windows_fn(), Err(SubmitError::Busy));
        assert_eq!(
            controller.apply_windows_fn_preview(),
            Err(SubmitError::Busy)
        );
        assert_eq!(json!(controller.preview), old.0);
        assert_eq!(json!(controller.brightness_preview), old.1);
        assert_eq!(json!(controller.oem_preview), old.2);
        assert_eq!(controller.fan_preview, old.3);
        assert_eq!(controller.cpu_power_preview, old.4);
        assert_eq!(controller.windows_fn_preview, old.5);
        assert_eq!(calls(&audit), before);
        close(&mut controller);
    }
}

#[test]
fn busy_worker_or_fan_lease_rejects_probe_without_erasing_foreign_evidence() {
    for held in [false, true] {
        let (blocked, probe_gate) = gate();
        let mut observation = step(Call::Probe, observed(false, 'a'));
        observation.gate = Some(blocked);
        let (mut controller, audit) = fixture(if held { vec![] } else { vec![observation] });
        if held {
            controller.probe_fan().unwrap();
            finish(&mut controller);
        } else {
            controller.probe_windows_fn().unwrap();
            probe_gate.wait();
        }
        controller.preview = Some(profile_plan());
        let old = json!(controller.preview);
        controller.windows_fn_error = Some(error("earlier_fn_failure"));
        let before = calls(&audit);
        assert_eq!(controller.probe_windows_fn(), Err(SubmitError::Busy));
        assert_eq!(controller.preview_windows_fn(), Err(SubmitError::Busy));
        assert_eq!(json!(controller.preview), old);
        assert_eq!(
            controller.windows_fn_error.as_ref().unwrap().code,
            "earlier_fn_failure"
        );
        assert_eq!(calls(&audit), before);
        if !held {
            probe_gate.release();
            finish(&mut controller);
        }
        close(&mut controller);
    }
}

#[test]
fn unknown_state_and_typed_receipt_survive_reads_and_partial_probes_until_fresh_complete_probe() {
    let failure = unknown(Lifetime::Reaped);
    let receipt = json!(failure.receipt);
    let mut partial_error = observed(false, 'b');
    partial_error.error = Some(error("fixture_partial"));
    let mut unqualified = observed(false, 'b');
    unqualified.status.as_mut().unwrap().qualified = false;
    let mut malformed_binding = observed(false, 'b');
    malformed_binding
        .status
        .as_mut()
        .unwrap()
        .control
        .as_mut()
        .unwrap()
        .binding = "not-a-hash".into();
    let mut blocked = observed(false, 'b');
    blocked
        .status
        .as_mut()
        .unwrap()
        .blocking_errors
        .push(error("fixture_blocking"));
    let (mut controller, audit) = fixture(vec![
        step(Call::Probe, observed(false, 'a')),
        step(Call::Apply, failure),
        step(Call::Probe, partial_error),
        step(Call::Probe, unqualified),
        step(Call::Probe, malformed_binding),
        step(Call::Probe, blocked),
        step(Call::Probe, observed(false, 'b')),
    ]);
    probe(&mut controller);
    preview(&mut controller, true);
    apply(&mut controller);
    controller.refresh().unwrap();
    finish(&mut controller);
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert_eq!(json!(controller.windows_fn_receipt), receipt);
    assert!(controller.windows_fn_error.is_some());
    assert!(controller.windows_fn_state_unknown && controller.windows_fn_lifetime_clean());
    assert_eq!(
        controller.preview_windows_fn(),
        Err(SubmitError::Unsupported)
    );
    for _ in 0..4 {
        probe(&mut controller);
        assert!(controller.windows_fn_state_unknown && controller.windows_fn_needs_probe);
        assert_eq!(json!(controller.windows_fn_receipt), receipt);
        assert_eq!(
            controller.preview_windows_fn(),
            Err(SubmitError::Unsupported)
        );
    }
    probe(&mut controller);
    assert!(!controller.windows_fn_state_unknown && !controller.windows_fn_needs_probe);
    assert_eq!(
        json!(controller.windows_fn_receipt),
        receipt,
        "probe erased historical recovery evidence"
    );
    preview(&mut controller, true);
    assert_eq!(
        controller.windows_fn_preview.as_ref().unwrap().binding,
        "b".repeat(64)
    );
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}

#[test]
fn unknown_helper_lifetime_cannot_be_cleared_by_no_pending_and_only_finish_reaped_allows_close() {
    let mut no_pending = observed(false, 'b');
    no_pending.outcome = Outcome::RejectedBeforeWrite;
    no_pending.status = None;
    no_pending.error = Some(error("windows_fn_no_pending"));
    no_pending.helper_lifetime = Lifetime::NotStarted;
    let (mut controller, audit) = fixture(vec![
        step(Call::Probe, observed(false, 'a')),
        step(Call::Apply, unknown(Lifetime::Unknown)),
        step(Call::Finish, no_pending),
        step(Call::Finish, unknown(Lifetime::Reaped)),
    ]);
    probe(&mut controller);
    preview(&mut controller, true);
    apply(&mut controller);
    assert!(!controller.windows_fn_lifetime_clean());
    assert_eq!(controller.probe_windows_fn(), Err(SubmitError::Busy));
    controller.request_close();
    controller.poll();
    assert!(!controller.close_ready());
    controller.finish_windows_fn_pending().unwrap();
    finish(&mut controller);
    assert!(!controller.windows_fn_lifetime_clean());
    assert_eq!(controller.windows_fn_helper_lifetime, Lifetime::Unknown);
    assert!(!controller.close_ready());
    assert_eq!(count(&audit, Call::Shutdown), 0);
    controller.finish_windows_fn_pending().unwrap();
    until(&mut controller, Controller::close_ready);
    assert!(controller.windows_fn_lifetime_clean() && controller.windows_fn_state_unknown);
    assert_eq!(count(&audit, Call::Probe), 1);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Finish), 2);
}

#[test]
fn reaped_process_with_continuing_writer_receipt_still_requires_fresh_probe() {
    let mut suspicious = applied(false, true, 'b');
    suspicious.receipt.writer_may_continue = true;
    let (mut controller, audit) = fixture(vec![
        step(Call::Probe, observed(false, 'a')),
        step(Call::Apply, suspicious),
        step(Call::Probe, observed(true, 'b')),
    ]);
    probe(&mut controller);
    preview(&mut controller, true);
    apply(&mut controller);
    assert!(controller.windows_fn_lifetime_clean());
    assert!(controller.windows_fn_state_unknown && controller.windows_fn_needs_probe);
    assert_eq!(
        controller.preview_windows_fn(),
        Err(SubmitError::Unsupported)
    );
    assert_eq!(
        controller.apply_windows_fn_preview(),
        Err(SubmitError::Unsupported)
    );
    controller.refresh().unwrap();
    finish(&mut controller);
    assert!(controller.windows_fn_state_unknown);
    probe(&mut controller);
    assert!(!controller.windows_fn_state_unknown && !controller.windows_fn_needs_probe);
    preview(&mut controller, false);
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}

#[test]
fn successful_looking_result_without_process_exit_blocks_new_helper_and_close() {
    let mut pending = applied(false, true, 'b');
    pending.helper_reaped = false;
    pending.helper_lifetime = Lifetime::Running;
    let (mut controller, audit) = fixture(vec![
        step(Call::Probe, observed(false, 'a')),
        step(Call::Apply, pending),
        step(Call::Finish, applied(false, true, 'b')),
    ]);
    probe(&mut controller);
    preview(&mut controller, true);
    apply(&mut controller);
    assert!(!controller.windows_fn_lifetime_clean());
    assert!(controller.windows_fn_state_unknown && controller.windows_fn_needs_probe);
    assert_eq!(controller.probe_windows_fn(), Err(SubmitError::Busy));
    assert_eq!(
        controller.apply_windows_fn_preview(),
        Err(SubmitError::Busy)
    );
    controller.request_close();
    controller.poll();
    assert!(!controller.close_ready());
    controller.finish_windows_fn_pending().unwrap();
    until(&mut controller, Controller::close_ready);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Finish), 1);
}

#[test]
fn close_waits_for_actual_apply_and_worker_drop_without_restoring_persistent_fn_setting() {
    let (blocked_apply, apply_gate) = gate();
    let (blocked_drop, drop_gate) = gate();
    let mut transaction = step(Call::Apply, applied(false, true, 'b'));
    transaction.gate = Some(blocked_apply);
    let (mut controller, audit) = fixture_with_drop_gate(
        vec![step(Call::Probe, observed(false, 'a')), transaction],
        Some(blocked_drop),
    );
    probe(&mut controller);
    preview(&mut controller, true);
    controller.apply_windows_fn_preview().unwrap();
    apply_gate.wait();
    controller.request_close();
    controller.request_close();
    controller.poll();
    assert!(!controller.close_ready() && controller.write_in_progress());
    assert_eq!(count(&audit, Call::Shutdown), 0);
    apply_gate.release();
    until(&mut controller, |controller| {
        controller.windows_fn_result.is_some()
    });
    drop_gate.wait();
    controller.poll();
    assert!(
        !controller.close_ready(),
        "helper result is not worker exit"
    );
    assert_eq!(
        controller.windows_fn_result.as_ref().unwrap().confirmed,
        OemValue::Boolean(true)
    );
    drop_gate.release();
    until(&mut controller, Controller::close_ready);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Shutdown), 1);
}

#[test]
fn panic_keeps_windows_lifetime_unknown_without_manufacturing_a_fan_lease() {
    let (mut controller, audit) = fixture(vec![
        step(Call::Probe, observed(false, 'a')),
        Step {
            call: Call::Apply,
            completion: Completion::Panic,
            gate: None,
        },
        step(Call::Finish, unknown(Lifetime::Reaped)),
    ]);
    probe(&mut controller);
    preview(&mut controller, true);
    apply(&mut controller);
    assert_eq!(
        controller.windows_fn_outcome,
        Some(Outcome::FailedStateUnknown)
    );
    assert!(controller.windows_fn_state_unknown && !controller.windows_fn_lifetime_clean());
    assert!(controller.windows_fn_error.is_some() && controller.worker_available);
    assert!(
        !controller.fan_session_held
            && controller.fan_helper_reaped
            && controller.fan_cleanup_complete
    );
    assert!(controller.fan_error.is_none());
    assert_eq!(controller.probe_windows_fn(), Err(SubmitError::Busy));
    controller.request_close();
    controller.poll();
    assert!(!controller.close_ready());
    controller.finish_windows_fn_pending().unwrap();
    until(&mut controller, Controller::close_ready);
    assert_eq!(count(&audit, Call::Apply), 1);
}

#[test]
fn target_edits_and_same_boolean_stale_binding_do_not_rearm_or_replay_old_apply() {
    let (blocked_probe, probe_gate) = gate();
    let (blocked_apply, apply_gate) = gate();
    let mut first = step(Call::Probe, observed(false, 'a'));
    first.gate = Some(blocked_probe);
    let mut stale = observed(false, 'b');
    stale.outcome = Outcome::RejectedBeforeWrite;
    stale.error = Some(error("fixture_stale_binding"));
    stale.receipt.primary_error = stale.error.clone();
    let mut second = step(Call::Apply, stale);
    second.gate = Some(blocked_apply);
    let (mut controller, audit) =
        fixture(vec![first, second, step(Call::Probe, observed(false, 'b'))]);
    controller.probe_windows_fn().unwrap();
    probe_gate.wait();
    controller.set_windows_fn_target(true);
    probe_gate.release();
    finish(&mut controller);
    assert_eq!(controller.windows_fn_requested, Some(true));
    preview(&mut controller, true);
    controller.set_windows_fn_target(false);
    controller.set_windows_fn_target(true);
    assert_eq!(
        controller.apply_windows_fn_preview(),
        Err(SubmitError::PreviewRequired)
    );
    assert_eq!(count(&audit, Call::Apply), 0);
    preview(&mut controller, true);
    controller.apply_windows_fn_preview().unwrap();
    apply_gate.wait();
    controller.set_windows_fn_target(false);
    apply_gate.release();
    finish(&mut controller);
    assert_eq!(controller.windows_fn_requested, Some(false));
    assert!(controller.windows_fn_preview.is_none());
    assert_eq!(
        controller
            .windows_fn_status
            .as_ref()
            .unwrap()
            .control
            .as_ref()
            .unwrap()
            .binding,
        "b".repeat(64)
    );
    assert_eq!(
        controller.windows_fn_error.as_ref().unwrap().code,
        "fixture_stale_binding"
    );
    assert!(controller.apply_windows_fn_preview().is_err());
    probe(&mut controller);
    preview(&mut controller, false);
    assert_eq!(
        controller.windows_fn_preview.as_ref().unwrap().binding,
        "b".repeat(64)
    );
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(audit.lock().unwrap().submitted[0].binding, "a".repeat(64));
    close(&mut controller);
}

#[test]
fn failed_followup_probe_keeps_the_full_apply_recovery_receipt_and_its_own_identity_evidence() {
    let mut restored = unknown(Lifetime::Reaped);
    restored.outcome = Outcome::FailedRestoredVerified;
    restored.receipt.recovery = Recovery::RestoredVerified;
    restored.receipt.restore_attempted = true;
    restored.receipt.restore_call_succeeded = Some(true);
    restored.receipt.original_state_verified = Some(true);
    restored.receipt.restore_error = None;
    let original_receipt = json!(restored.receipt);
    let identity_error = error("fixture_followup_identity");
    let followup_receipt = Receipt {
        identity_error: Some(identity_error.clone()),
        ..Default::default()
    };
    let followup = Update {
        status: None,
        outcome: Outcome::RejectedBeforeWrite,
        result: None,
        receipt: followup_receipt.clone(),
        error: Some(identity_error),
        helper_reaped: true,
        helper_lifetime: Lifetime::Reaped,
    };
    let (mut controller, audit) = fixture(vec![
        step(Call::Probe, observed(false, 'a')),
        step(Call::Apply, restored),
        step(Call::Probe, followup),
        step(Call::Probe, observed(false, 'b')),
    ]);
    probe(&mut controller);
    preview(&mut controller, true);
    apply(&mut controller);
    assert!(controller.windows_fn_needs_probe);
    assert_eq!(json!(controller.windows_fn_receipt), original_receipt);
    probe(&mut controller);
    assert!(controller.windows_fn_needs_probe);
    assert_eq!(
        json!(controller.windows_fn_receipt),
        original_receipt,
        "non-writing probe erased primary/readback/restore evidence"
    );
    assert_eq!(
        json!(controller.windows_fn_followup_receipt),
        json!(followup_receipt)
    );
    probe(&mut controller);
    assert!(!controller.windows_fn_state_unknown && !controller.windows_fn_needs_probe);
    assert_eq!(json!(controller.windows_fn_receipt), original_receipt);
    assert_eq!(
        json!(controller.windows_fn_followup_receipt),
        json!(followup_receipt),
        "successful probe erased prior follow-up identity evidence"
    );
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}
