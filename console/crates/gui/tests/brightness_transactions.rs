//! Exercise Controller routing, retained transaction evidence and worker close
//! ordering. All devices, routes, writes and lifecycle events are in memory.
use mechrevo_console_core::{
    brightness::{
        BrightnessBackend, BrightnessOutcome, BrightnessRecovery, BrightnessTransaction,
        LogindSessionBinding, BRIGHTNESS_TRANSACTION_MARKER,
    },
    cpu_power::{Action as CpuAction, Plan as CpuPlan},
    fan_curve::{client::FanClientUpdate, FanCurve, FanCurveChannel, FanCurveLayout},
    ApplyResult, BrightnessApplyResult, BrightnessDevice, BrightnessPlan, CapabilityStatus,
    CatalogModel, ControlError, EvidenceSource, OemControlKind, OemDomain, OemPlan, OemValue,
    PowerProfile, ProfilePlan, Snapshot, ThermalSample,
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
    Snapshot,
    Catalog,
    Thermal,
    Preview,
    Apply,
    ProbeFan,
    Shutdown,
    DropStarted,
    Drop,
}
#[derive(Default)]
struct Audit {
    calls: Vec<Call>,
    submitted: Vec<BrightnessPlan>,
    violations: Vec<String>,
}
type SharedAudit = Arc<Mutex<Audit>>;
type Devices = Arc<Mutex<Vec<BrightnessDevice>>>;
fn record(audit: &SharedAudit, call: Call) {
    audit.lock().unwrap().calls.push(call);
}
fn calls(audit: &SharedAudit) -> Vec<Call> {
    let audit = audit.lock().unwrap();
    assert!(
        audit.violations.is_empty(),
        "unexpected fixture access: {:?}",
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
            .expect("worker did not reach scripted stage");
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
    Confirmed,
    Error(ControlError),
    Panic,
}
struct Step {
    completion: Completion,
    gate: Option<Gate>,
}
fn failure(transaction: BrightnessTransaction) -> Step {
    Step {
        completion: Completion::Error(error_with_transaction(transaction)),
        gate: None,
    }
}
fn confirmed() -> Step {
    Step {
        completion: Completion::Confirmed,
        gate: None,
    }
}
struct DisplayBackend {
    devices: Devices,
    audit: SharedAudit,
    steps: VecDeque<Step>,
    preview_errors: VecDeque<ControlError>,
    preview_gate: Option<Gate>,
    drop_gate: Option<Gate>,
}
impl Backend for DisplayBackend {
    fn snapshot(&mut self) -> Snapshot {
        record(&self.audit, Call::Snapshot);
        snapshot(self.devices.lock().unwrap().clone())
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
            .push("unexpected profile planning".into());
        Err(ControlError::new(
            "fixture_forbidden",
            "brightness does not change OS power profiles",
        ))
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        self.audit
            .lock()
            .unwrap()
            .violations
            .push("unexpected profile write".into());
        Err(ControlError::new(
            "fixture_forbidden",
            "brightness does not change OS power profiles",
        ))
    }
    fn plan_brightness(&mut self, percent: u8, id: &str) -> Result<BrightnessPlan, ControlError> {
        record(&self.audit, Call::Preview);
        if let Some(gate) = self.preview_gate.take() {
            block(gate);
        }
        if let Some(error) = self.preview_errors.pop_front() {
            return Err(error);
        }
        let device = self
            .devices
            .lock()
            .unwrap()
            .iter()
            .find(|device| device.id == id)
            .cloned()
            .unwrap();
        Ok(BrightnessPlan {
            device_id: device.id,
            requested_percent: percent,
            target: (u32::from(percent) * device.maximum.unwrap() + 50) / 100,
            previous: device.control_value.unwrap(),
            maximum: device.maximum.unwrap(),
            source: device.source,
            backend: device.write_backend.unwrap(),
            logind_session: device.logind_session,
        })
    }
    fn apply_brightness(
        &mut self,
        plan: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        record(&self.audit, Call::Apply);
        self.audit.lock().unwrap().submitted.push(plan.clone());
        let Some(step) = self.steps.pop_front() else {
            self.audit
                .lock()
                .unwrap()
                .violations
                .push("unexpected brightness apply".into());
            panic!("unexpected brightness apply");
        };
        if let Some(gate) = step.gate {
            block(gate);
        }
        match step.completion {
            Completion::Confirmed => {
                let mut devices = self.devices.lock().unwrap();
                let device = devices
                    .iter_mut()
                    .find(|device| device.id == plan.device_id)
                    .unwrap();
                device.control_value = Some(plan.target);
                device.current_percent = Some(f64::from(plan.requested_percent));
                Ok(BrightnessApplyResult {
                    device_id: plan.device_id.clone(),
                    requested_percent: plan.requested_percent,
                    confirmed: plan.target,
                    previous: plan.previous,
                    actual_value: device.actual_value,
                    source: plan.source.clone(),
                    transaction: confirmed_transaction(plan.target),
                })
            }
            Completion::Error(error) => Err(error),
            Completion::Panic => panic!("scripted brightness writer panic"),
        }
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
impl Drop for DisplayBackend {
    fn drop(&mut self) {
        record(&self.audit, Call::DropStarted);
        if let Some(gate) = self.drop_gate.take() {
            block(gate);
        }
        record(&self.audit, Call::Drop);
    }
}
fn session(suffix: &str) -> LogindSessionBinding {
    LogindSessionBinding {
        session_id: suffix.into(),
        object_path: format!("/org/freedesktop/login1/session/{suffix}"),
        uid: 1000,
        seat_id: "seat0".into(),
        seat_path: "/org/freedesktop/login1/seat/seat0".into(),
    }
}
fn device(id: &str) -> BrightnessDevice {
    BrightnessDevice {
        id: id.into(),
        label: format!("fixture {id}"),
        current_percent: Some(50.0),
        control_value: Some(50),
        actual_value: Some(45),
        maximum: Some(100),
        source: EvidenceSource {
            backend: "linux-sysfs".into(),
            path: format!("fixture://canonical/{id}/brightness"),
            device_id: Some(id.into()),
        },
        status: CapabilityStatus::Available,
        readable: true,
        writable: false,
        write_backend: Some(BrightnessBackend::LinuxLogind),
        logind_session: Some(session("one")),
        supported_percentages: vec![],
        reason: None,
    }
}
fn snapshot(brightness: Vec<BrightnessDevice>) -> Snapshot {
    Snapshot {
        platform: "linux".into(),
        identity: Default::default(),
        telemetry: vec![],
        capabilities: vec![],
        power_policy: Default::default(),
        diagnostics: vec![],
        brightness,
        oem_controls: vec![],
    }
}
fn fixture(steps: Vec<Step>) -> (Controller, SharedAudit, Devices) {
    fixture_with_gates(steps, None, None)
}
fn fixture_with_gates(
    steps: Vec<Step>,
    preview_gate: Option<Gate>,
    drop_gate: Option<Gate>,
) -> (Controller, SharedAudit, Devices) {
    configured_fixture(steps, preview_gate, drop_gate, vec![])
}
fn configured_fixture(
    steps: Vec<Step>,
    preview_gate: Option<Gate>,
    drop_gate: Option<Gate>,
    preview_errors: Vec<ControlError>,
) -> (Controller, SharedAudit, Devices) {
    let devices = Arc::new(Mutex::new(vec![device("panel-a")]));
    let audit = Arc::new(Mutex::new(Audit::default()));
    let mut controller = Controller::new(DisplayBackend {
        devices: devices.clone(),
        audit: audit.clone(),
        steps: steps.into(),
        preview_errors: preview_errors.into(),
        preview_gate,
        drop_gate,
    })
    .unwrap();
    controller.snapshot = Some(snapshot(devices.lock().unwrap().clone()));
    controller.selected_brightness_device = Some("panel-a".into());
    controller.requested_brightness = 60;
    (controller, audit, devices)
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
fn preview(controller: &mut Controller) {
    controller.preview_brightness().unwrap();
    finish(controller);
    assert!(controller.brightness_preview.is_some());
}
fn apply(controller: &mut Controller) {
    controller.apply_brightness_preview().unwrap();
    finish(controller);
    assert!(controller.brightness_preview.is_none());
}
fn close(controller: &mut Controller) {
    controller.request_close();
    until(controller, Controller::close_ready);
}
fn confirmed_transaction(control: u32) -> BrightnessTransaction {
    BrightnessTransaction {
        outcome: BrightnessOutcome::Confirmed,
        write_attempted: true,
        writer_may_continue: false,
        observed_control: Some(control),
        primary_error: None,
        rollback_error: None,
        readback_error: None,
        recovery: BrightnessRecovery::NotNeeded,
        recovery_reason: None,
    }
}
fn failed_transaction(outcome: BrightnessOutcome, continues: bool) -> BrightnessTransaction {
    BrightnessTransaction {
        outcome,
        write_attempted: true,
        writer_may_continue: continues,
        observed_control: Some(57),
        primary_error: Some(ControlError::new(
            "fixture_primary",
            "initial setter failed",
        )),
        rollback_error: Some(ControlError::new(
            "fixture_rollback",
            "recovery setter failed",
        )),
        readback_error: Some(ControlError::new(
            "fixture_readback",
            "recovery read failed",
        )),
        recovery: if outcome == BrightnessOutcome::RecoveryFailed {
            BrightnessRecovery::Failed
        } else {
            BrightnessRecovery::Unknown
        },
        recovery_reason: Some("fixture: keep all three error layers".into()),
    }
}
fn error_with_transaction(transaction: BrightnessTransaction) -> ControlError {
    let mut error = ControlError::new(
        "fixture_brightness_failed",
        "typed brightness transaction failed",
    );
    error.details.push(format!(
        "{BRIGHTNESS_TRANSACTION_MARKER}{}",
        serde_json::to_string(&transaction).unwrap()
    ));
    error
}
fn profile_plan() -> ProfilePlan {
    ProfilePlan {
        requested: PowerProfile::Performance,
        target: "performance".into(),
        previous: "balanced".into(),
        scope: "fixture".into(),
        details: None,
        source: device("panel-a").source,
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
        source: device("panel-a").source,
        binding: "fixture".into(),
        readback_scope: "fixture".into(),
        requires_privilege: true,
        companion_brightness: None,
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
fn delegated_logind_route_can_preview_and_apply_without_falsely_claiming_direct_writability() {
    let (mut controller, audit, devices) = fixture(vec![confirmed()]);
    assert!(!devices.lock().unwrap()[0].writable);
    assert!(devices.lock().unwrap()[0].can_control());
    assert!(controller.brightness_unavailable_reason().is_none());
    preview(&mut controller);
    let plan = controller.brightness_preview.clone().unwrap();
    assert_eq!(plan.backend, BrightnessBackend::LinuxLogind);
    assert_eq!(plan.logind_session, Some(session("one")));
    assert_eq!(count(&audit, Call::Apply), 0);
    apply(&mut controller);
    let result = controller.brightness_applied.as_ref().unwrap();
    assert_eq!(result.confirmed, 60);
    assert_eq!(
        result.actual_value,
        Some(45),
        "requested control is separate from physical readback"
    );
    assert_eq!(
        controller.brightness_transaction.as_ref().unwrap().outcome,
        BrightnessOutcome::Confirmed
    );
    assert_eq!(json!(controller.brightness_last_plan), json!(plan));
    assert!(!controller.brightness_needs_refresh && !controller.brightness_writer_may_continue);
    assert!(controller.brightness_error.is_none());
    assert!(!controller.snapshot.as_ref().unwrap().brightness[0].writable);
    assert_eq!(
        controller.apply_brightness_preview(),
        Err(SubmitError::PreviewRequired)
    );
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}

#[test]
fn typed_failure_retains_primary_recovery_and_readback_evidence_through_unrelated_reads() {
    for outcome in [
        BrightnessOutcome::Unknown,
        BrightnessOutcome::RecoveryFailed,
    ] {
        let transaction = failed_transaction(outcome, false);
        let (mut controller, audit, _) = fixture(vec![failure(transaction.clone())]);
        preview(&mut controller);
        let plan = controller.brightness_preview.clone();
        apply(&mut controller);
        let error = controller.brightness_error.clone().unwrap();
        controller.refresh().unwrap();
        finish(&mut controller);
        controller.refresh_thermal().unwrap();
        finish(&mut controller);
        assert_eq!(json!(controller.brightness_transaction), json!(transaction));
        assert_eq!(json!(controller.brightness_last_plan), json!(plan));
        let retained = controller.brightness_error.as_ref().unwrap();
        assert_eq!(retained.code, error.code);
        assert_eq!(retained.message, error.message);
        assert_eq!(retained.details, error.details);
        assert!(controller.brightness_needs_refresh && !controller.brightness_writer_may_continue);
        assert!(controller.preview_brightness().is_err());
        assert_eq!(
            controller.apply_brightness_preview(),
            Err(SubmitError::PreviewRequired)
        );
        assert_eq!(count(&audit, Call::Apply), 1);
        close(&mut controller);
    }
}

#[test]
fn possible_continuing_writer_remains_locked_after_explicit_observation_but_can_be_closed_after_return(
) {
    let transaction = failed_transaction(BrightnessOutcome::Unknown, true);
    let (mut controller, audit, devices) = fixture(vec![failure(transaction.clone())]);
    preview(&mut controller);
    apply(&mut controller);
    devices.lock().unwrap()[0].control_value = Some(57);
    controller.refresh_brightness_state().unwrap();
    finish(&mut controller);
    assert_eq!(
        controller.snapshot.as_ref().unwrap().brightness[0].control_value,
        Some(57)
    );
    assert!(controller.brightness_needs_refresh && controller.brightness_writer_may_continue);
    assert_eq!(json!(controller.brightness_transaction), json!(transaction));
    assert!(controller.preview_brightness().is_err());
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
    assert_eq!(
        count(&audit, Call::Apply),
        1,
        "close must not automatically restore or retry brightness"
    );
}

#[test]
fn only_explicit_same_source_legal_read_unlocks_and_next_preview_rebinds_the_current_route() {
    let (mut controller, audit, devices) = fixture(vec![
        failure(failed_transaction(BrightnessOutcome::Unknown, false)),
        confirmed(),
    ]);
    preview(&mut controller);
    apply(&mut controller);
    let failure_evidence = json!(controller.brightness_transaction);
    let original = devices.lock().unwrap()[0].clone();
    for variant in 0..7 {
        let mut changed = original.clone();
        match variant {
            0 => changed.source.path = "fixture://different-canonical-target".into(),
            1 => changed.maximum = Some(200),
            2 => changed.control_value = None,
            3 => changed.maximum = Some(0),
            4 => changed.control_value = Some(101),
            5 => changed.readable = false,
            6 => changed.status = CapabilityStatus::Unavailable,
            _ => unreachable!(),
        }
        devices.lock().unwrap()[0] = changed;
        controller.refresh_brightness_state().unwrap();
        finish(&mut controller);
        assert!(
            controller.brightness_needs_refresh,
            "invalid or foreign observation unlocked variant {variant}"
        );
    }
    devices.lock().unwrap()[0] = original.clone();
    controller.refresh().unwrap();
    finish(&mut controller);
    assert!(
        controller.brightness_needs_refresh,
        "ordinary refresh must not acknowledge recovery"
    );
    let mut read_only = original;
    read_only.write_backend = None;
    read_only.logind_session = None;
    devices.lock().unwrap()[0] = read_only;
    controller.refresh_brightness_state().unwrap();
    finish(&mut controller);
    assert!(
        !controller.brightness_needs_refresh,
        "legal read may confirm state even without a writable route"
    );
    assert_eq!(json!(controller.brightness_transaction), failure_evidence);
    assert!(
        controller.brightness_error.is_some(),
        "explicit observation must retain the failed transaction evidence"
    );
    assert!(controller.preview_brightness().is_err());
    assert_eq!(
        controller.apply_brightness_preview(),
        Err(SubmitError::PreviewRequired)
    );
    {
        let mut devices = devices.lock().unwrap();
        devices[0].write_backend = Some(BrightnessBackend::LinuxLogind);
        devices[0].logind_session = Some(session("two"));
    }
    controller.refresh().unwrap();
    finish(&mut controller);
    preview(&mut controller);
    assert_eq!(
        controller
            .brightness_preview
            .as_ref()
            .unwrap()
            .logind_session,
        Some(session("two"))
    );
    apply(&mut controller);
    let submitted = audit.lock().unwrap().submitted.clone();
    assert_eq!(submitted.len(), 2);
    assert_eq!(submitted[0].logind_session, Some(session("one")));
    assert_eq!(submitted[1].logind_session, Some(session("two")));
    close(&mut controller);
}

#[test]
fn rejected_busy_fan_lease_or_cpu_plan_does_not_discard_foreign_previews() {
    for cause in 0..3 {
        let (blocked, preview_gate) = gate();
        let (mut controller, audit, _) =
            fixture_with_gates(vec![], (cause == 0).then_some(blocked), None);
        if cause == 0 {
            controller.preview_brightness().unwrap();
            preview_gate.wait();
        }
        if cause == 1 {
            controller.probe_fan().unwrap();
            finish(&mut controller);
            assert!(controller.fan_session_held);
        }
        controller.preview = Some(profile_plan());
        controller.oem_preview = Some(oem_plan());
        controller.fan_preview = Some(fan_curve().validate().unwrap());
        if cause == 2 {
            controller.cpu_power_preview = Some(CpuPlan {
                revision: "a".repeat(64),
                action: CpuAction::Clear,
            });
        }
        let old = (
            json!(controller.preview),
            json!(controller.oem_preview),
            controller.fan_preview.clone(),
            controller.cpu_power_preview.clone(),
        );
        let before = calls(&audit);
        assert!(controller.preview_brightness().is_err());
        assert_eq!(json!(controller.preview), old.0);
        assert_eq!(json!(controller.oem_preview), old.1);
        assert_eq!(controller.fan_preview, old.2);
        assert_eq!(controller.cpu_power_preview, old.3);
        assert_eq!(calls(&audit), before);
        if cause == 0 {
            preview_gate.release();
            finish(&mut controller);
        }
        close(&mut controller);
    }
}

#[test]
fn close_waits_for_actual_apply_and_worker_exit_without_initiating_another_write() {
    let (blocked_apply, apply_gate) = gate();
    let (blocked_drop, drop_gate) = gate();
    let (mut controller, audit, _) = fixture_with_gates(
        vec![Step {
            completion: Completion::Confirmed,
            gate: Some(blocked_apply),
        }],
        None,
        Some(blocked_drop),
    );
    preview(&mut controller);
    controller.apply_brightness_preview().unwrap();
    apply_gate.wait();
    controller.request_close();
    controller.request_close();
    controller.poll();
    assert!(controller.write_in_progress());
    assert!(!controller.close_ready());
    assert_eq!(count(&audit, Call::Shutdown), 0);
    apply_gate.release();
    until(&mut controller, |controller| {
        controller.brightness_applied.is_some()
    });
    drop_gate.wait();
    controller.poll();
    assert!(
        !controller.close_ready(),
        "completed write does not mean the backend thread is reaped"
    );
    assert_eq!(count(&audit, Call::Apply), 1);
    drop_gate.release();
    until(&mut controller, Controller::close_ready);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Shutdown), 1);
}

#[test]
fn untyped_or_malformed_transaction_errors_are_unknown_until_explicit_refresh() {
    for details in [
        vec![],
        vec![format!("{BRIGHTNESS_TRANSACTION_MARKER}{{broken-json")],
    ] {
        let mut error = ControlError::new(
            "fixture_plain_error",
            "provider did not return trustworthy transaction evidence",
        );
        error.details = details;
        let (mut controller, audit, _) = fixture(vec![Step {
            completion: Completion::Error(error.clone()),
            gate: None,
        }]);
        preview(&mut controller);
        apply(&mut controller);
        assert_eq!(
            controller.brightness_transaction.as_ref().unwrap().outcome,
            BrightnessOutcome::Unknown
        );
        assert!(controller.brightness_needs_refresh && !controller.brightness_writer_may_continue);
        assert_eq!(
            controller.brightness_error.as_ref().unwrap().details,
            error.details
        );
        assert!(controller.preview_brightness().is_err());
        controller.refresh_brightness_state().unwrap();
        finish(&mut controller);
        assert!(!controller.brightness_needs_refresh);
        preview(&mut controller);
        assert_eq!(count(&audit, Call::Apply), 1);
        close(&mut controller);
    }
}

#[test]
fn apply_panic_is_brightness_unknown_with_continuing_writer_without_fabricating_fan_failure() {
    let (mut controller, audit, _) = fixture(vec![Step {
        completion: Completion::Panic,
        gate: None,
    }]);
    preview(&mut controller);
    apply(&mut controller);
    assert_eq!(
        controller.brightness_transaction.as_ref().unwrap().outcome,
        BrightnessOutcome::Unknown
    );
    assert!(controller.brightness_needs_refresh && controller.brightness_writer_may_continue);
    assert!(controller.worker_available && controller.fan_error.is_none());
    assert!(
        !controller.fan_session_held
            && controller.fan_helper_reaped
            && controller.fan_cleanup_complete
    );
    controller.refresh_brightness_state().unwrap();
    finish(&mut controller);
    assert!(controller.brightness_needs_refresh && controller.brightness_writer_may_continue);
    assert!(controller.preview_brightness().is_err());
    close(&mut controller);
    assert_eq!(count(&audit, Call::Apply), 1);
}

#[test]
fn late_failure_stays_bound_to_submitted_device_and_cannot_be_acknowledged_by_another_display() {
    let (blocked, apply_gate) = gate();
    let (mut controller, audit, devices) = fixture(vec![Step {
        completion: Completion::Error(error_with_transaction(failed_transaction(
            BrightnessOutcome::Unknown,
            false,
        ))),
        gate: Some(blocked),
    }]);
    devices.lock().unwrap().push(device("panel-b"));
    controller.snapshot = Some(snapshot(devices.lock().unwrap().clone()));
    preview(&mut controller);
    controller.apply_brightness_preview().unwrap();
    apply_gate.wait();
    controller.select_brightness_device("panel-b");
    controller.set_brightness_target(70);
    apply_gate.release();
    finish(&mut controller);
    assert_eq!(
        controller.selected_brightness_device.as_deref(),
        Some("panel-b")
    );
    assert_eq!(controller.requested_brightness, 70);
    assert!(controller.brightness_applied.is_none());
    assert_eq!(
        controller.brightness_last_plan.as_ref().unwrap().device_id,
        "panel-a"
    );
    assert!(controller.brightness_needs_refresh);
    controller.refresh_brightness_state().unwrap();
    finish(&mut controller);
    assert!(controller.brightness_needs_refresh, "refreshing selected display B must not acknowledge display A even when A is in the snapshot");
    assert_eq!(
        controller.selected_brightness_device.as_deref(),
        Some("panel-b")
    );
    assert_eq!(controller.requested_brightness, 70);
    *devices.lock().unwrap() = vec![device("panel-b")];
    controller.refresh_brightness_state().unwrap();
    finish(&mut controller);
    assert!(
        controller.brightness_needs_refresh,
        "display B observation acknowledged display A's failed write"
    );
    assert_eq!(
        controller.selected_brightness_device.as_deref(),
        Some("panel-b")
    );
    assert_eq!(controller.requested_brightness, 70);
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}

#[test]
fn successful_retry_clears_own_preview_error_without_erasing_distinct_shared_evidence() {
    let mut preview_error =
        ControlError::new("fixture_preview_failed", "Preview could not be created");
    preview_error.details = vec!["original preview-only evidence".into()];
    let (mut controller, audit, _) =
        configured_fixture(vec![confirmed()], None, None, vec![preview_error.clone()]);
    controller.preview_brightness().unwrap();
    finish(&mut controller);
    assert!(controller.brightness_preview.is_none());
    let retained = controller.brightness_preview_error.as_ref().unwrap();
    assert_eq!(retained.code, preview_error.code);
    assert_eq!(retained.message, preview_error.message);
    assert_eq!(retained.details, preview_error.details);
    assert!(
        controller.error.is_none(),
        "preview-only error leaked into the shared write-error channel"
    );
    preview(&mut controller);
    assert!(controller.brightness_preview_error.is_none());
    assert!(controller.error.is_none());
    apply(&mut controller);
    assert!(controller.brightness_preview_error.is_none() && controller.brightness_error.is_none());
    assert!(
        controller.error.is_none(),
        "successful retry left the old preview failure visible"
    );
    assert_eq!(count(&audit, Call::Preview), 2);
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);

    // A real earlier Apply failure supplies the dedicated write error. Give
    // the shared channel a distinct event with identical code/message; only
    // the details distinguish its ownership when the next Apply succeeds.
    let transaction = BrightnessTransaction {
        outcome: BrightnessOutcome::NotApplied,
        write_attempted: false,
        writer_may_continue: false,
        observed_control: Some(50),
        primary_error: Some(preview_error),
        rollback_error: None,
        readback_error: None,
        recovery: BrightnessRecovery::NotNeeded,
        recovery_reason: None,
    };
    let (mut controller, audit, _) = fixture(vec![failure(transaction), confirmed()]);
    preview(&mut controller);
    apply(&mut controller);
    let original = controller.brightness_error.clone().unwrap();
    assert!(!controller.brightness_needs_refresh);
    let mut unrelated = original.clone();
    unrelated.details = vec!["a different control's retained evidence".into()];
    controller.error = Some(unrelated.clone());
    preview(&mut controller);
    apply(&mut controller);
    assert!(controller.brightness_error.is_none());
    let retained = controller
        .error
        .as_ref()
        .expect("successful brightness Apply erased unrelated shared evidence");
    assert_eq!(retained.code, original.code);
    assert_eq!(retained.message, original.message);
    assert_eq!(retained.details, unrelated.details);
    assert_eq!(count(&audit, Call::Apply), 2);
    close(&mut controller);
}
