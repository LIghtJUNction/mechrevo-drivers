//! Production Controller/worker coverage with scripted providers only. No test
//! starts a helper, authorizes a request, or opens hardware or persistent state.
use mechrevo_console_core::{
    brightness::BrightnessBackend,
    cpu_power::{Action, CpuPowerUpdate, Outcome, Plan, Policy, Status},
    fan_curve::{
        broker::{BrokerPhase, BrokerState},
        client::FanClientUpdate,
        FanCurve, FanCurveChannel, FanCurveLayout,
    },
    ApplyResult, BrightnessApplyResult, BrightnessPlan, CatalogModel, ControlError, EvidenceSource,
    OemApplyResult, OemControlKind, OemDomain, OemPlan, OemValue, PowerProfile, ProfilePlan,
    Snapshot, ThermalSample,
};
use mechrevo_console_gui::{Backend, Controller, SubmitError};
use serde_json::{json, Value};
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
    Execute,
    Snapshot,
    Catalog,
    Thermal,
    ProfilePlan,
    ProfileApply,
    BrightnessPlan,
    BrightnessApply,
    OemPlan,
    OemApply,
    FanApply,
    Shutdown,
    DropStarted { managed: bool },
    Drop { managed: bool },
}
#[derive(Default)]
struct Audit {
    calls: Vec<Call>,
    submitted: Vec<Plan>,
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
fn count(audit: &SharedAudit, expected: Call) -> usize {
    calls(audit)
        .into_iter()
        .filter(|call| *call == expected)
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
        self.release
            .send(())
            .expect("worker exited before scripted release");
    }
}
fn block(gate: Gate) {
    gate.entered.send(()).unwrap();
    gate.release
        .recv_timeout(Duration::from_secs(2))
        .expect("test did not release worker stage");
}
enum Completion {
    Update(Box<CpuPowerUpdate>),
    Panic,
}
struct Step {
    call: Call,
    completion: Completion,
    gate: Option<Gate>,
}
fn step(call: Call, update: CpuPowerUpdate) -> Step {
    Step {
        call,
        completion: Completion::Update(Box::new(update)),
        gate: None,
    }
}

struct CpuBackend {
    audit: SharedAudit,
    steps: VecDeque<Step>,
    policy: Option<Policy>,
    drop_gate: Option<Gate>,
}
impl CpuBackend {
    fn cpu_call(&mut self, call: Call) -> CpuPowerUpdate {
        record(&self.audit, call);
        let Some(step) = self.steps.pop_front() else {
            self.audit
                .lock()
                .unwrap()
                .violations
                .push(format!("unexpected {call:?}"));
            panic!("unexpected CPU operation");
        };
        if step.call != call {
            self.audit
                .lock()
                .unwrap()
                .violations
                .push(format!("expected {:?}, got {call:?}", step.call));
            panic!("CPU operations reordered");
        }
        if let Some(gate) = step.gate {
            block(gate);
        }
        match step.completion {
            Completion::Update(update) => {
                let update = *update;
                if let Some(status) = &update.status {
                    self.policy = status.current_policy.clone();
                }
                update
            }
            Completion::Panic => panic!("scripted potentially started CPU writer panic"),
        }
    }
}
impl Backend for CpuBackend {
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
        record(&self.audit, Call::ProfilePlan);
        Ok(profile_plan())
    }
    fn apply(&mut self, plan: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        record(&self.audit, Call::ProfileApply);
        Ok(ApplyResult {
            requested: plan.requested,
            confirmed: plan.target.clone(),
            previous: plan.previous.clone(),
            scope: plan.scope.clone(),
        })
    }
    fn plan_brightness(&mut self, _: u8, _: &str) -> Result<BrightnessPlan, ControlError> {
        record(&self.audit, Call::BrightnessPlan);
        Ok(brightness_plan())
    }
    fn apply_brightness(
        &mut self,
        _: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        record(&self.audit, Call::BrightnessApply);
        Err(ControlError::new(
            "fixture_forbidden",
            "ordinary write must be interlocked",
        ))
    }
    fn plan_oem(&mut self, _: &str, _: OemValue) -> Result<OemPlan, ControlError> {
        record(&self.audit, Call::OemPlan);
        Ok(oem_plan())
    }
    fn apply_oem(&mut self, _: &OemPlan) -> Result<OemApplyResult, ControlError> {
        record(&self.audit, Call::OemApply);
        Err(ControlError::new(
            "fixture_forbidden",
            "ordinary write must be interlocked",
        ))
    }
    fn fan_apply(
        &mut self,
        _: &mechrevo_console_core::fan_curve::ValidatedFanCurve,
    ) -> FanClientUpdate {
        record(&self.audit, Call::FanApply);
        FanClientUpdate::default()
    }
    fn cpu_power_probe(&mut self) -> CpuPowerUpdate {
        self.cpu_call(Call::Probe)
    }
    fn cpu_power_execute(&mut self, plan: &Plan) -> CpuPowerUpdate {
        self.audit.lock().unwrap().submitted.push(plan.clone());
        self.cpu_call(Call::Execute)
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        // Closing the CPU UI only ends the worker, never clears its policy.
        record(&self.audit, Call::Shutdown);
        FanClientUpdate::default()
    }
}
impl Drop for CpuBackend {
    fn drop(&mut self) {
        let managed = self.policy.is_some();
        record(&self.audit, Call::DropStarted { managed });
        if let Some(gate) = self.drop_gate.take() {
            block(gate);
        }
        record(&self.audit, Call::Drop { managed });
    }
}
fn controller(steps: Vec<Step>) -> (Controller, SharedAudit) {
    controller_with_drop_gate(steps, None)
}
fn controller_with_drop_gate(
    steps: Vec<Step>,
    drop_gate: Option<Gate>,
) -> (Controller, SharedAudit) {
    let audit = Arc::new(Mutex::new(Audit::default()));
    let controller = Controller::new(CpuBackend {
        audit: audit.clone(),
        steps: steps.into(),
        policy: None,
        drop_gate,
    })
    .unwrap();
    (controller, audit)
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
    until(controller, |c| c.activity().is_none());
}
fn close(controller: &mut Controller) {
    controller.request_close();
    until(controller, Controller::close_ready);
}
fn probe(controller: &mut Controller) {
    controller.probe_cpu_power().unwrap();
    finish(controller);
}
fn preview(controller: &mut Controller, action: Action) {
    controller.preview_cpu_power(action).unwrap();
    assert!(controller.cpu_power_preview_pending());
}
fn execute(controller: &mut Controller) {
    controller.apply_cpu_power_preview().unwrap();
    finish(controller);
    assert!(
        controller.cpu_power_preview.is_none(),
        "CPU plan is single-use"
    );
}
fn policy() -> Policy {
    Policy {
        cap_uw: 55_000_000,
        target_celsius: Some(90),
        polling_seconds: Some(1),
        rapl_debounce_seconds: Some(0),
    }
}
fn status(management: &str, revision: u8) -> Status {
    let managed = management == "managed";
    let allowed = match management {
        "native" => json!(["set"]),
        "managed" => json!(["set", "clear", "reapply"]),
        "recovery_pending" => json!(["recover"]),
        _ => json!([]),
    };
    serde_json::from_value(json!({
        "management":management, "revision":format!("{revision:064x}"), "qualified":true,
        "allowed_actions":allowed, "current_policy":if managed { json!(policy()) } else { Value::Null },
        "operational_ceiling_uw":80_000_000, "minimum_limit_uw":35_000_000,
        "thermal_spec_reference_uw":55_000_000,
        "observed":[{"interface":"msr","pl1_uw":if managed {55_000_000} else {80_000_000}},
                    {"interface":"mmio","pl1_uw":if managed {55_000_000} else {80_000_000}}],
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
fn applied(status: Status, action: &str) -> CpuPowerUpdate {
    CpuPowerUpdate {
        status: Some(status),
        outcome: Outcome::AppliedVerified,
        error: None,
        receipt: Some(json!({"fixture_transaction":true,"action":action})),
        helper_reaped: true,
    }
}
fn unknown(reaped: bool) -> CpuPowerUpdate {
    CpuPowerUpdate {
        status: None,
        outcome: Outcome::FailedStateUnknown,
        error: Some(ControlError::new(
            "fixture_cpu_unknown",
            "CPU result could not be confirmed",
        )),
        receipt: Some(json!({"primary":"write_failed","rollback":"unconfirmed"})),
        helper_reaped: reaped,
    }
}
fn source() -> EvidenceSource {
    EvidenceSource {
        backend: "fixture-only".into(),
        path: "fixture://no-hardware".into(),
        device_id: None,
    }
}
fn profile_plan() -> ProfilePlan {
    ProfilePlan {
        requested: PowerProfile::Performance,
        target: "performance".into(),
        previous: "balanced".into(),
        scope: "fixture".into(),
        source: source(),
    }
}
fn brightness_plan() -> BrightnessPlan {
    BrightnessPlan {
        device_id: "fixture-display".into(),
        requested_percent: 60,
        target: 60,
        previous: 50,
        maximum: 100,
        source: source(),
        backend: BrightnessBackend::LinuxSysfs,
        logind_session: None,
    }
}
fn oem_plan() -> OemPlan {
    OemPlan {
        control_id: "fixture-fn".into(),
        kind: OemControlKind::FnLock,
        label: "fixture".into(),
        previous: OemValue::Boolean(false),
        target: OemValue::Boolean(true),
        domain: OemDomain::Boolean,
        source: source(),
        binding: "fixture-identity".into(),
        readback_scope: "fixture driver request".into(),
        requires_privilege: true,
        companion_brightness: None,
    }
}
fn snapshot() -> Snapshot {
    serde_json::from_value(json!({
        "platform":"linux", "identity":{"support_note":"fixture only; no hardware","mechrevo_brand_detected":false},
        "telemetry":[],"diagnostics":[],
        "capabilities":[{"id":"power_profile","label":"fixture","status":"available","reason":"fixture","source":null,"options":["balanced","performance"]}],
        "power_policy":{"scope":"fixture","current":"balanced","options":["balanced","performance"],"source":null},
        "brightness":[{"id":"fixture-display","label":"fixture","current_percent":50.0,"control_value":50,"actual_value":50,
            "maximum":100,"source":source(),"status":"available","readable":true,"writable":true,"write_backend":"linux_sysfs","logind_session":null,"supported_percentages":[],"reason":null}],
        "oem_controls":[{"id":"fixture-fn","kind":"fn_lock","label":"fixture","current":{"type":"boolean","value":false},
            "domain":{"type":"boolean"},"status":"available","writable":false,"source":source(),"binding":"fixture-identity",
            "readback_scope":"fixture driver request","reason":null}]
    })).unwrap()
}
fn prepare_ordinary(controller: &mut Controller) {
    controller.snapshot = Some(snapshot());
    controller.selected_profile = PowerProfile::Performance;
    controller.selected_brightness_device = Some("fixture-display".into());
    controller.requested_brightness = 60;
    controller.selected_oem_control = Some("fixture-fn".into());
    controller.requested_oem_value = Some(OemValue::Boolean(true));
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
fn prepare_fan(controller: &mut Controller) {
    controller.fan_state = Some(BrokerState {
        backend: "fixture".into(),
        device_path: "fixture://fan".into(),
        epoch: 1,
        generation: 1,
        lease_id: 0,
        last_request_id: 1,
        phase: BrokerPhase::Idle,
        capabilities: 7,
        owner: false,
        revoked: false,
        modified_or_uncertain: false,
        state_is_current: true,
        restored_verified: false,
        live_error: 0,
        last_error: 0,
        restore_error: 0,
        live_table: Some(vec![51; 96]),
        original_table: None,
        live_controls: Some([0; 6]),
        original_controls: None,
        cached_original_targets: None,
        applied_curve: None,
    });
    let curve = fan_curve().validate().unwrap();
    controller.fan_draft = Some(curve.clone());
    controller.fan_preview = Some(curve);
}

#[test]
fn explicit_probe_and_local_preview_preserve_full_policy_through_set_reapply_clear_recover() {
    let (mut controller, audit) = controller(vec![
        step(Call::Probe, observed(status("native", 1))),
        step(Call::Execute, applied(status("managed", 2), "set")),
        step(Call::Execute, applied(status("managed", 3), "reapply")),
        step(Call::Execute, applied(status("native", 4), "clear")),
        step(Call::Probe, observed(status("recovery_pending", 5))),
        step(Call::Execute, applied(status("native", 6), "recover")),
    ]);
    assert!(calls(&audit).is_empty(), "construction authorized a helper");
    controller.refresh().unwrap();
    finish(&mut controller);
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert_eq!(count(&audit, Call::Probe), 0);
    assert_eq!(count(&audit, Call::Execute), 0);
    probe(&mut controller);
    let before_preview = calls(&audit);
    controller.set_cpu_power_draft(policy());
    assert_eq!(controller.cpu_power_draft, Some(policy()));
    let action = Action::Set { policy: policy() };
    preview(&mut controller, action.clone());
    assert_eq!(
        calls(&audit),
        before_preview,
        "local preview contacted a backend"
    );
    assert_eq!(
        controller.cpu_power_preview.as_ref().unwrap().action,
        action
    );
    execute(&mut controller);
    assert_eq!(
        controller.cpu_power_status.as_ref().unwrap().current_policy,
        Some(policy())
    );
    assert_eq!(controller.cpu_power_draft, Some(policy()));
    preview(&mut controller, Action::Reapply);
    execute(&mut controller);
    preview(&mut controller, Action::Clear);
    execute(&mut controller);
    assert!(controller
        .cpu_power_status
        .as_ref()
        .unwrap()
        .current_policy
        .is_none());
    probe(&mut controller);
    preview(&mut controller, Action::Recover);
    execute(&mut controller);
    let submitted = audit.lock().unwrap().submitted.clone();
    assert_eq!(submitted.len(), 4);
    assert_eq!(submitted[0].action, Action::Set { policy: policy() });
    assert_eq!(submitted[1].action, Action::Reapply);
    assert_eq!(submitted[2].action, Action::Clear);
    assert_eq!(submitted[3].action, Action::Recover);
    assert_eq!(
        submitted
            .iter()
            .map(|plan| plan.revision.clone())
            .collect::<Vec<_>>(),
        [1, 2, 3, 5].map(|revision| format!("{revision:064x}"))
    );
    assert_eq!(count(&audit, Call::ProfileApply), 0);
    close(&mut controller);
    assert_eq!(
        count(&audit, Call::Execute),
        4,
        "close executed a CPU action"
    );
}

/// Leaves both CPU Backend methods at their production unsupported defaults.
struct UnsupportedBackend(SharedAudit);
impl Backend for UnsupportedBackend {
    fn snapshot(&mut self) -> Snapshot {
        record(&self.0, Call::Snapshot);
        snapshot()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        record(&self.0, Call::Catalog);
        vec![]
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        record(&self.0, Call::ProfilePlan);
        Err(ControlError::new("fixture", "no fallback"))
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        record(&self.0, Call::ProfileApply);
        Err(ControlError::new("fixture", "no fallback"))
    }
}
#[test]
fn unsupported_cpu_provider_never_falls_back_to_snapshot_or_system_profile() {
    let audit = Arc::new(Mutex::new(Audit::default()));
    let mut controller = Controller::new(UnsupportedBackend(audit.clone())).unwrap();
    probe(&mut controller);
    assert!(controller.cpu_power_error.is_some());
    assert!(controller.cpu_power_preview.is_none());
    assert!(
        calls(&audit).is_empty(),
        "CPU probe fell back to an unrelated provider"
    );
    close(&mut controller);
}

#[test]
fn each_foreign_preview_blocks_cpu_operations_without_discarding_any_plan() {
    for kind in 0..4 {
        let (mut controller, audit) =
            controller(vec![step(Call::Probe, observed(status("native", 1)))]);
        probe(&mut controller);
        controller.set_cpu_power_draft(policy());
        assert_eq!(controller.cpu_power_draft, Some(policy()));
        preview(&mut controller, Action::Set { policy: policy() });
        let cpu_plan = controller.cpu_power_preview.clone();
        match kind {
            0 => controller.preview = Some(profile_plan()),
            1 => controller.brightness_preview = Some(brightness_plan()),
            2 => controller.oem_preview = Some(oem_plan()),
            3 => {
                let curve = fan_curve().validate().unwrap();
                controller.fan_draft = Some(curve.clone());
                controller.fan_preview = Some(curve);
            }
            _ => unreachable!(),
        }
        let old = (
            json!(controller.preview),
            json!(controller.brightness_preview),
            json!(controller.oem_preview),
            controller.fan_preview.clone(),
        );
        let before = calls(&audit);
        assert_eq!(controller.probe_cpu_power(), Err(SubmitError::Busy));
        assert_eq!(
            controller.preview_cpu_power(Action::Set { policy: policy() }),
            Err(SubmitError::Busy)
        );
        assert_eq!(controller.apply_cpu_power_preview(), Err(SubmitError::Busy));
        assert_eq!(controller.cpu_power_preview, cpu_plan);
        assert_eq!(json!(controller.preview), old.0);
        assert_eq!(json!(controller.brightness_preview), old.1);
        assert_eq!(json!(controller.oem_preview), old.2);
        assert_eq!(controller.fan_preview, old.3);
        assert_eq!(calls(&audit), before);
        close(&mut controller);
    }
}

#[test]
fn cpu_preview_blocks_old_preview_and_refresh_paths_before_they_invalidate_it() {
    let (mut controller, audit) =
        controller(vec![step(Call::Probe, observed(status("native", 1)))]);
    probe(&mut controller);
    prepare_ordinary(&mut controller);
    controller.set_cpu_power_draft(policy());
    assert_eq!(controller.cpu_power_draft, Some(policy()));
    preview(&mut controller, Action::Set { policy: policy() });
    let plan = controller.cpu_power_preview.clone();
    controller.cpu_power_error = Some(ControlError::new(
        "earlier_cpu_error",
        "Keep evidence visible",
    ));
    let before = calls(&audit);
    assert_eq!(controller.preview_profile(), Err(SubmitError::Busy));
    assert_eq!(controller.preview_brightness(), Err(SubmitError::Busy));
    assert_eq!(controller.preview_oem(), Err(SubmitError::Busy));
    assert_eq!(controller.refresh(), Err(SubmitError::Busy));
    assert_eq!(controller.cpu_power_preview, plan);
    assert_eq!(controller.cpu_power_draft, Some(policy()));
    assert_eq!(
        controller.cpu_power_error.as_ref().unwrap().code,
        "earlier_cpu_error"
    );
    assert!(
        controller.preview.is_none()
            && controller.brightness_preview.is_none()
            && controller.oem_preview.is_none()
    );
    assert_eq!(calls(&audit), before);
    close(&mut controller);
}

#[test]
fn restored_failure_retains_error_details_and_receipt_through_unrelated_reads() {
    let mut failure = ControlError::new("fixture_primary", "Primary write failed");
    failure.details = vec![
        "primary: denied".into(),
        "rollback: verified".into(),
        "receipt-write: failed".into(),
        "post-read: retained".into(),
    ];
    let receipt = json!({"original":"native","rollback":"verified","receipt_error":"disk-full"});
    let update = CpuPowerUpdate {
        status: Some(status("native", 2)),
        outcome: Outcome::FailedRestoredVerified,
        error: Some(failure.clone()),
        receipt: Some(receipt.clone()),
        helper_reaped: true,
    };
    let (mut controller, audit) = controller(vec![
        step(Call::Probe, observed(status("native", 1))),
        step(Call::Execute, update),
    ]);
    probe(&mut controller);
    preview(&mut controller, Action::Set { policy: policy() });
    execute(&mut controller);
    controller.refresh().unwrap();
    finish(&mut controller);
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert_eq!(
        controller.cpu_power_outcome,
        Some(Outcome::FailedRestoredVerified)
    );
    let retained = controller.cpu_power_error.as_ref().unwrap();
    assert_eq!(retained.code, failure.code);
    assert_eq!(retained.message, failure.message);
    assert_eq!(retained.details, failure.details);
    assert_eq!(controller.cpu_power_receipt, Some(receipt));
    assert!(controller.cpu_power_helper_reaped && !controller.cpu_power_state_unknown);
    assert_eq!(count(&audit, Call::Execute), 1);
    close(&mut controller);
}

#[test]
fn state_unknown_survives_reads_and_partial_probes_until_a_fresh_qualified_probe() {
    let mut invalid = Vec::new();
    let mut unqualified = status("managed", 3);
    unqualified.qualified = false;
    invalid.push(observed(unqualified));
    let mut no_revision = status("managed", 4);
    no_revision.revision = None;
    invalid.push(observed(no_revision));
    let mut blocked = status("managed", 5);
    blocked
        .blocking_errors
        .push("fixture qualification failed".into());
    invalid.push(observed(blocked));
    let mut partial = observed(status("managed", 6));
    partial.error = Some(ControlError::new("fixture_partial", "Only partial status"));
    invalid.push(partial);
    let mut steps = vec![
        step(Call::Probe, observed(status("native", 1))),
        step(Call::Execute, unknown(true)),
    ];
    steps.extend(invalid.into_iter().map(|update| step(Call::Probe, update)));
    steps.push(step(Call::Probe, observed(status("managed", 7))));
    let (mut controller, audit) = controller(steps);
    probe(&mut controller);
    preview(&mut controller, Action::Set { policy: policy() });
    execute(&mut controller);
    let receipt = controller.cpu_power_receipt.clone();
    assert!(controller.cpu_power_state_unknown && controller.cpu_power_helper_reaped);
    controller.refresh().unwrap();
    finish(&mut controller);
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert!(controller.cpu_power_state_unknown);
    assert_eq!(controller.cpu_power_receipt, receipt);
    prepare_ordinary(&mut controller);
    controller.preview = Some(profile_plan());
    assert!(controller.apply_preview().is_err());
    controller.preview = None;
    controller.brightness_preview = Some(brightness_plan());
    assert!(controller.apply_brightness_preview().is_err());
    controller.brightness_preview = None;
    controller.oem_preview = Some(oem_plan());
    assert!(controller.apply_oem_preview().is_err());
    controller.oem_preview = None;
    prepare_fan(&mut controller);
    assert!(controller.apply_fan().is_err());
    controller.invalidate_fan_preview();
    for _ in 0..4 {
        probe(&mut controller);
        assert!(
            controller.cpu_power_state_unknown,
            "partial/unqualified probe erased write interlock"
        );
        assert_eq!(
            controller.cpu_power_receipt, receipt,
            "read-only probe discarded earlier execution evidence"
        );
    }
    probe(&mut controller);
    assert!(!controller.cpu_power_state_unknown);
    preview(&mut controller, Action::Clear);
    assert!(controller.cpu_power_unavailable_reason().is_none());
    assert_eq!(count(&audit, Call::ProfileApply), 0);
    assert_eq!(count(&audit, Call::BrightnessApply), 0);
    assert_eq!(count(&audit, Call::OemApply), 0);
    assert_eq!(count(&audit, Call::FanApply), 0);
    close(&mut controller);
}

#[test]
fn unknown_helper_lifetime_blocks_another_probe_and_close_without_clearing_policy() {
    let (mut controller, audit) = controller(vec![
        step(Call::Probe, observed(status("native", 1))),
        step(Call::Execute, unknown(false)),
    ]);
    probe(&mut controller);
    preview(&mut controller, Action::Set { policy: policy() });
    execute(&mut controller);
    assert!(controller.cpu_power_state_unknown && !controller.cpu_power_helper_reaped);
    assert_eq!(controller.probe_cpu_power(), Err(SubmitError::Busy));
    assert_eq!(
        controller.preview_cpu_power(Action::Set { policy: policy() }),
        Err(SubmitError::Busy)
    );
    controller.request_close();
    controller.request_close();
    controller.poll();
    assert!(!controller.close_ready());
    assert_eq!(count(&audit, Call::Probe), 1);
    assert_eq!(count(&audit, Call::Execute), 1);
    assert_eq!(count(&audit, Call::Shutdown), 0);
}

#[test]
fn close_waits_for_execute_and_worker_reaping_without_automatically_clearing_persistent_policy() {
    let (blocked_execute, execute_gate) = gate();
    let (blocked_drop, drop_gate) = gate();
    let mut execution = step(Call::Execute, applied(status("managed", 2), "set"));
    execution.gate = Some(blocked_execute);
    let (mut controller, audit) = controller_with_drop_gate(
        vec![step(Call::Probe, observed(status("native", 1))), execution],
        Some(blocked_drop),
    );
    probe(&mut controller);
    preview(&mut controller, Action::Set { policy: policy() });
    controller.apply_cpu_power_preview().unwrap();
    execute_gate.wait();
    controller.request_close();
    controller.request_close();
    controller.poll();
    assert!(!controller.close_ready());
    assert!(controller.write_in_progress());
    assert_eq!(controller.probe_cpu_power(), Err(SubmitError::Busy));
    assert_eq!(count(&audit, Call::Shutdown), 0);
    execute_gate.release();
    until(&mut controller, |c| {
        c.cpu_power_outcome == Some(Outcome::AppliedVerified)
    });
    drop_gate.wait();
    controller.poll();
    assert!(controller.cpu_power_helper_reaped);
    assert!(
        !controller.close_ready(),
        "helper reaping does not mean the platform worker has exited"
    );
    assert_eq!(
        controller.cpu_power_status.as_ref().unwrap().current_policy,
        Some(policy())
    );
    assert_eq!(count(&audit, Call::Execute), 1);
    assert!(calls(&audit).contains(&Call::DropStarted { managed: true }));
    drop_gate.release();
    until(&mut controller, Controller::close_ready);
    assert!(calls(&audit).contains(&Call::Drop { managed: true }));
    assert_eq!(
        count(&audit, Call::Execute),
        1,
        "close sent Clear/Recover to the CPU provider"
    );
    assert_eq!(count(&audit, Call::Shutdown), 1);
}

#[test]
fn cpu_execute_panic_is_typed_unknown_and_does_not_fabricate_a_clean_helper_or_fan_receipt() {
    let (mut controller, audit) = controller(vec![
        step(Call::Probe, observed(status("native", 1))),
        Step {
            call: Call::Execute,
            completion: Completion::Panic,
            gate: None,
        },
    ]);
    probe(&mut controller);
    preview(&mut controller, Action::Set { policy: policy() });
    execute(&mut controller);
    assert_eq!(
        controller.cpu_power_outcome,
        Some(Outcome::FailedStateUnknown)
    );
    assert!(controller.cpu_power_error.is_some());
    assert!(controller.cpu_power_state_unknown && !controller.cpu_power_helper_reaped);
    assert!(
        controller.worker_available,
        "CPU panic must retain the platform worker"
    );
    assert!(
        !controller.fan_session_held
            && controller.fan_helper_reaped
            && controller.fan_cleanup_complete
    );
    assert!(controller.fan_error.is_none());
    assert_eq!(controller.probe_cpu_power(), Err(SubmitError::Busy));
    controller.request_close();
    controller.poll();
    assert!(!controller.close_ready());
    assert_eq!(count(&audit, Call::Shutdown), 0);
    assert!(!calls(&audit)
        .iter()
        .any(|call| matches!(call, Call::DropStarted { .. } | Call::Drop { .. })));
}

#[test]
fn late_probe_and_execution_do_not_overwrite_a_newer_complete_local_draft() {
    let (blocked_probe, probe_gate) = gate();
    let (blocked_execute, execute_gate) = gate();
    let mut observation = step(Call::Probe, observed(status("native", 1)));
    observation.gate = Some(blocked_probe);
    let mut execution = step(Call::Execute, applied(status("managed", 2), "set"));
    execution.gate = Some(blocked_execute);
    let (mut controller, audit) = controller(vec![observation, execution]);
    let edited = Policy {
        cap_uw: 60_000_000,
        target_celsius: None,
        polling_seconds: None,
        rapl_debounce_seconds: None,
    };
    controller.probe_cpu_power().unwrap();
    probe_gate.wait();
    controller.set_cpu_power_draft(edited.clone());
    assert_eq!(controller.cpu_power_draft, Some(edited.clone()));
    probe_gate.release();
    finish(&mut controller);
    assert_eq!(
        controller.cpu_power_draft,
        Some(edited.clone()),
        "late observation replaced the user's draft"
    );
    preview(
        &mut controller,
        Action::Set {
            policy: edited.clone(),
        },
    );
    assert_eq!(
        controller.cpu_power_preview.as_ref().unwrap().action,
        Action::Set {
            policy: edited.clone()
        },
        "native options must stay None rather than silently acquiring 90/1/0"
    );
    controller.set_cpu_power_draft(policy());
    assert_eq!(controller.cpu_power_draft, Some(policy()));
    preview(&mut controller, Action::Set { policy: policy() });
    controller.apply_cpu_power_preview().unwrap();
    execute_gate.wait();
    controller.set_cpu_power_draft(edited.clone());
    assert_eq!(controller.cpu_power_draft, Some(edited.clone()));
    execute_gate.release();
    finish(&mut controller);
    assert_eq!(
        controller.cpu_power_status.as_ref().unwrap().current_policy,
        Some(policy())
    );
    assert_eq!(controller.cpu_power_draft, Some(edited));
    assert!(controller.cpu_power_preview.is_none());
    assert_eq!(
        audit.lock().unwrap().submitted[0].action,
        Action::Set { policy: policy() }
    );
    assert_eq!(controller.cpu_power_outcome, Some(Outcome::AppliedVerified));
    close(&mut controller);
}

#[test]
fn cancelled_probe_keeps_cached_status_without_allowing_it_to_authorize_a_new_plan() {
    let cancelled = CpuPowerUpdate {
        status: None,
        outcome: Outcome::RejectedBeforeWrite,
        error: Some(ControlError::new(
            "fixture_authorization_cancelled",
            "Authorization cancelled",
        )),
        receipt: None,
        helper_reaped: true,
    };
    let (mut controller, audit) = controller(vec![
        step(Call::Probe, observed(status("native", 1))),
        step(Call::Probe, cancelled),
        step(Call::Probe, observed(status("native", 3))),
    ]);
    assert!(controller.cpu_power_needs_probe);
    probe(&mut controller);
    assert!(!controller.cpu_power_needs_probe);
    preview(&mut controller, Action::Set { policy: policy() });
    probe(&mut controller);
    assert_eq!(controller.cpu_power_status, Some(status("native", 1)));
    assert!(controller.cpu_power_needs_probe && !controller.cpu_power_state_unknown);
    assert!(controller.cpu_power_preview.is_none());
    let before = calls(&audit);
    assert_eq!(
        controller.preview_cpu_power(Action::Set { policy: policy() }),
        Err(SubmitError::Unsupported)
    );
    assert_eq!(
        controller.apply_cpu_power_preview(),
        Err(SubmitError::Unsupported)
    );
    assert_eq!(
        calls(&audit),
        before,
        "stale cached status started another CPU request"
    );
    probe(&mut controller);
    assert!(!controller.cpu_power_needs_probe);
    preview(&mut controller, Action::Set { policy: policy() });
    assert_eq!(count(&audit, Call::Execute), 0);
    close(&mut controller);
}

#[test]
fn restored_failure_requires_fresh_probe_before_cached_qualified_status_can_be_previewed_again() {
    let receipt = json!({"restore":"verified","transaction":"fixture-failed-write"});
    let restored = CpuPowerUpdate {
        status: Some(status("native", 2)),
        outcome: Outcome::FailedRestoredVerified,
        error: Some(ControlError::new(
            "fixture_write_restored",
            "Write failed; restoration verified",
        )),
        receipt: Some(receipt.clone()),
        helper_reaped: true,
    };
    let (mut controller, audit) = controller(vec![
        step(Call::Probe, observed(status("native", 1))),
        step(Call::Execute, restored),
        step(Call::Probe, observed(status("native", 3))),
    ]);
    probe(&mut controller);
    preview(&mut controller, Action::Set { policy: policy() });
    execute(&mut controller);
    assert!(controller.cpu_power_status.as_ref().unwrap().qualified);
    assert!(controller.cpu_power_needs_probe && !controller.cpu_power_state_unknown);
    controller.refresh().unwrap();
    finish(&mut controller);
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert!(
        controller.cpu_power_needs_probe,
        "unrelated reads erased the Client probe requirement"
    );
    assert_eq!(controller.cpu_power_receipt, Some(receipt.clone()));
    let before = calls(&audit);
    assert_eq!(
        controller.preview_cpu_power(Action::Set { policy: policy() }),
        Err(SubmitError::Unsupported)
    );
    assert_eq!(
        controller.apply_cpu_power_preview(),
        Err(SubmitError::Unsupported)
    );
    assert_eq!(calls(&audit), before);
    probe(&mut controller);
    assert!(!controller.cpu_power_needs_probe);
    assert_eq!(controller.cpu_power_receipt, Some(receipt));
    preview(&mut controller, Action::Set { policy: policy() });
    assert_eq!(count(&audit, Call::Execute), 1);
    close(&mut controller);
}
