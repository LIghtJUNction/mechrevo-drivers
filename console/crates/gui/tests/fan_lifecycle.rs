#![cfg(feature = "gui")]
//! Exercise the production controller worker and native close path, with a
//! scripted broker boundary. Nothing in this fixture opens a device or helper.
use eframe::egui::{self, Context, RawInput, Rect, ViewportCommand, ViewportEvent, ViewportId};
use mechrevo_console_core::{
    fan_curve::{
        broker::{BrokerPhase, BrokerState},
        client::FanClientUpdate,
        FanCurve, FanCurveChannel, FanCurveLayout, ValidatedFanCurve,
    },
    ApplyResult, CatalogModel, ControlError, PowerProfile, ProfilePlan, Snapshot,
};
use mechrevo_console_gui::{desktop::DesktopApp, Backend, Controller, Preferences, SubmitError};
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
    Refresh,
    Apply,
    Restore,
    Shutdown,
    Lifecycle,
    DropStarted { held: bool },
    Drop { held: bool },
}
#[derive(Default)]
struct Audit {
    calls: Vec<Call>,
    applied: Vec<ValidatedFanCurve>,
    violations: Vec<String>,
    planned_panics: usize,
}
type SharedAudit = Arc<Mutex<Audit>>;

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
            .expect("worker never entered scripted broker call");
    }
    fn release(&self) {
        self.release
            .send(())
            .expect("worker exited before the test released the call");
    }
}

enum Completion {
    Reply(Box<FanClientUpdate>),
    Panic,
}
struct Step {
    call: Call,
    completion: Completion,
    gate: Option<Gate>,
    /// Independent fake-owner state: dropping a backend is observable even
    /// when the UI discarded or misinterpreted a receipt.
    held_after: bool,
}
impl Step {
    fn reply(call: Call, update: FanClientUpdate) -> Self {
        let held_after = update.session_held;
        Self {
            call,
            completion: Completion::Reply(Box::new(update)),
            gate: None,
            held_after,
        }
    }
    fn blocked(call: Call, update: FanClientUpdate, gate: Gate) -> Self {
        let mut step = Self::reply(call, update);
        step.gate = Some(gate);
        step
    }
    fn panic(call: Call) -> Self {
        Self {
            call,
            completion: Completion::Panic,
            gate: None,
            held_after: true,
        }
    }
}
struct BrokerBackend {
    steps: VecDeque<Step>,
    audit: SharedAudit,
    held: bool,
    drop_gate: Option<Gate>,
    lifecycle: Receiver<FanClientUpdate>,
}
impl BrokerBackend {
    fn call(&mut self, call: Call) -> FanClientUpdate {
        self.audit.lock().unwrap().calls.push(call);
        let Some(step) = self.steps.pop_front() else {
            self.audit
                .lock()
                .unwrap()
                .violations
                .push(format!("unexpected broker operation {call:?}"));
            panic!("unexpected broker operation");
        };
        if step.call != call {
            self.audit
                .lock()
                .unwrap()
                .violations
                .push(format!("expected {:?}, observed {call:?}", step.call));
            panic!("controller reordered the broker lifecycle");
        }
        // Acquisition may occur before an Apply call returns. Restore and
        // Shutdown retain their previous owner until completion is released.
        if call == Call::Apply {
            self.held = true;
        }
        if let Some(gate) = step.gate {
            gate.entered.send(()).unwrap();
            gate.release
                .recv_timeout(Duration::from_secs(2))
                .expect("UI blocked instead of releasing the transaction");
        }
        self.held = step.held_after;
        match step.completion {
            Completion::Reply(update) => *update,
            Completion::Panic => {
                self.audit.lock().unwrap().planned_panics += 1;
                panic!("scripted fan worker panic after ownership may have changed")
            }
        }
    }
}
impl Drop for BrokerBackend {
    fn drop(&mut self) {
        self.audit
            .lock()
            .unwrap()
            .calls
            .push(Call::DropStarted { held: self.held });
        if let Some(gate) = self.drop_gate.take() {
            let _ = gate.entered.send(());
            let _ = gate.release.recv_timeout(Duration::from_secs(2));
        }
        self.audit
            .lock()
            .unwrap()
            .calls
            .push(Call::Drop { held: self.held });
    }
}
impl Backend for BrokerBackend {
    fn snapshot(&mut self) -> Snapshot {
        serde_json::from_value(json!({
            "platform":"linux", "identity":{"support_note":"fixture only; no hardware", "mechrevo_brand_detected":false},
            "telemetry":[], "brightness":[], "capabilities":[], "diagnostics":[]
        })).unwrap()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        vec![]
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        unreachable!("fan fixture never plans power changes")
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        unreachable!("fan fixture never applies power changes")
    }
    fn fan_probe(&mut self) -> FanClientUpdate {
        self.call(Call::Probe)
    }
    fn fan_refresh(&mut self) -> FanClientUpdate {
        self.call(Call::Refresh)
    }
    fn fan_apply(&mut self, curve: &ValidatedFanCurve) -> FanClientUpdate {
        self.audit.lock().unwrap().applied.push(curve.clone());
        self.call(Call::Apply)
    }
    fn fan_restore(&mut self) -> FanClientUpdate {
        self.call(Call::Restore)
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        self.call(Call::Shutdown)
    }
    fn fan_poll(&mut self) -> Option<FanClientUpdate> {
        let update = self.lifecycle.try_recv().ok()?;
        self.audit.lock().unwrap().calls.push(Call::Lifecycle);
        self.held = update.session_held;
        Some(update)
    }
}

fn controller(steps: Vec<Step>) -> (Controller, SharedAudit) {
    controller_with_drop_gate(steps, None)
}
fn controller_with_drop_gate(
    steps: Vec<Step>,
    drop_gate: Option<Gate>,
) -> (Controller, SharedAudit) {
    let (controller, audit, _lifecycle) = controller_fixture(steps, drop_gate);
    (controller, audit)
}
fn controller_fixture(
    steps: Vec<Step>,
    drop_gate: Option<Gate>,
) -> (Controller, SharedAudit, SyncSender<FanClientUpdate>) {
    let audit = Arc::new(Mutex::new(Audit::default()));
    let (lifecycle, events) = mpsc::sync_channel(1);
    let controller = Controller::new(BrokerBackend {
        steps: steps.into(),
        audit: audit.clone(),
        held: false,
        drop_gate,
        lifecycle: events,
    })
    .unwrap();
    (controller, audit, lifecycle)
}
fn calls(audit: &SharedAudit) -> Vec<Call> {
    let audit = audit.lock().unwrap();
    assert!(
        audit.violations.is_empty(),
        "fixture protocol violation swallowed by production panic handling: {:?}",
        audit.violations
    );
    audit.calls.clone()
}
fn count(audit: &SharedAudit, call: Call) -> usize {
    calls(audit)
        .iter()
        .filter(|candidate| **candidate == call)
        .count()
}
fn state(phase: BrokerPhase) -> BrokerState {
    let owner = matches!(
        phase,
        BrokerPhase::Acquired | BrokerPhase::Active | BrokerPhase::RecoveryRequired
    );
    BrokerState {
        backend: "fixture-broker".into(),
        device_path: "fixture://fan-no-hardware".into(),
        epoch: 1,
        generation: 7,
        lease_id: if owner { 17 } else { 0 },
        last_request_id: 23,
        phase,
        capabilities: 7,
        owner,
        revoked: false,
        modified_or_uncertain: phase == BrokerPhase::RecoveryRequired,
        state_is_current: true,
        restored_verified: false,
        live_error: 0,
        last_error: 0,
        restore_error: 0,
        live_table: Some(vec![51; 96]),
        original_table: owner.then(|| vec![255; 96]),
        live_controls: Some([0xb5, 0xa9, 0x12, 0x81, 21, 31]),
        original_controls: owner.then_some([0xb5, 0xa9, 0x12, 0x81, 21, 31]),
        cached_original_targets: owner.then_some([57, 79, 29, 61]),
        applied_curve: (phase == BrokerPhase::Active).then(curve),
    }
}
fn idle() -> FanClientUpdate {
    FanClientUpdate {
        state: Some(state(BrokerPhase::Idle)),
        ..Default::default()
    }
}
fn held(phase: BrokerPhase) -> FanClientUpdate {
    FanClientUpdate {
        state: Some(state(phase)),
        error: None,
        session_held: true,
        helper_reaped: false,
        restoration_confirmed: false,
        cleanup_complete: false,
    }
}
fn restored_helper_alive() -> FanClientUpdate {
    let mut state = state(BrokerPhase::Idle);
    state.restored_verified = true;
    FanClientUpdate {
        state: Some(state),
        error: None,
        session_held: false,
        helper_reaped: false,
        restoration_confirmed: true,
        cleanup_complete: true,
    }
}
fn shutdown_restored() -> FanClientUpdate {
    FanClientUpdate {
        helper_reaped: true,
        ..restored_helper_alive()
    }
}
fn clean() -> FanClientUpdate {
    FanClientUpdate::default()
}
fn unknown(code: &str) -> FanClientUpdate {
    FanClientUpdate {
        error: Some(ControlError::new(
            code,
            "fixture result cannot confirm ownership or restoration",
        )),
        ..held(BrokerPhase::RecoveryRequired)
    }
}
fn curve() -> FanCurve {
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
fn prepare(controller: &mut Controller) -> ValidatedFanCurve {
    let draft = curve();
    controller.set_fan_draft(draft.clone()).unwrap();
    controller.preview_fan().unwrap();
    // Fixture equivalent of an already completed read-only observation. The
    // explicit Probe worker path is exercised separately below.
    if controller.fan_state.is_none() {
        controller.fan_state = Some(state(BrokerPhase::Idle));
    }
    draft.validate().unwrap()
}
fn until(controller: &mut Controller, condition: impl Fn(&Controller) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !condition(controller) && Instant::now() < deadline {
        controller.poll();
        std::thread::yield_now();
    }
    assert!(
        condition(controller),
        "production worker did not reach the expected state"
    );
}
fn finish(controller: &mut Controller) {
    until(controller, |controller| controller.activity().is_none());
}
fn close(controller: &mut Controller) {
    controller.request_close();
    until(controller, Controller::close_ready);
}
fn assert_owner_not_dropped(audit: &SharedAudit) {
    assert!(
        !calls(audit)
            .iter()
            .any(|call| matches!(call, Call::DropStarted { .. } | Call::Drop { .. })),
        "backend/fd dropped before confirmed cleanup"
    );
}
fn render(ctx: &Context, app: &mut DesktopApp, close: bool) -> Vec<ViewportCommand> {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 700.0),
        )),
        ..Default::default()
    };
    if close {
        input
            .viewports
            .get_mut(&ViewportId::ROOT)
            .unwrap()
            .events
            .push(ViewportEvent::Close);
    }
    let output = ctx.run_ui(input, |ui| {
        app.render(ui);
    });
    let commands = output
        .viewport_output
        .get(&ViewportId::ROOT)
        .unwrap()
        .commands
        .clone();
    output.drop_without_applying_deltas();
    commands
}
fn app(controller: Controller) -> DesktopApp {
    DesktopApp::with_controller(
        controller,
        Preferences {
            auto_refresh: false,
            ..Default::default()
        },
    )
}

#[test]
fn construction_import_preview_and_draft_validation_never_contact_broker() {
    let (mut controller, audit) = controller(vec![Step::reply(Call::Shutdown, clean())]);
    assert!(calls(&audit).is_empty());
    let valid = curve();
    controller
        .import_fan_curve(&serde_json::to_vec(&valid).unwrap())
        .unwrap();
    controller.preview_fan().unwrap();
    assert!(controller.fan_preview.is_some());
    controller.import_fan_curve(b"{invalid").unwrap_err();
    assert!(controller.fan_draft.is_none() && controller.fan_preview.is_none());
    let mut invalid = valid;
    invalid.cpu.duty_raw[0] = 201;
    controller.set_fan_draft(invalid).unwrap_err();
    assert!(controller.fan_draft.is_none() && controller.fan_preview.is_none());
    assert!(calls(&audit).is_empty());
    assert!(!controller.fan_session_held);
    // The shutdown request itself is explicit and must not manufacture a lease.
    close(&mut controller);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

#[test]
fn probe_is_explicit_read_only_and_does_not_make_a_preview_apply_automatically() {
    let (mut controller, audit) = controller(vec![
        Step::reply(Call::Probe, idle()),
        Step::reply(Call::Shutdown, clean()),
    ]);
    prepare(&mut controller);
    controller.probe_fan().unwrap();
    finish(&mut controller);
    assert_eq!(calls(&audit), [Call::Probe]);
    assert!(!controller.fan_session_held);
    assert!(controller.fan_applied.is_none());
    assert_eq!(count(&audit, Call::Apply), 0);
    close(&mut controller);
}

#[test]
fn successful_apply_consumes_preview_and_retains_owner_until_explicit_restore_and_reap() {
    let (mut controller, audit) = controller(vec![
        Step::reply(Call::Apply, held(BrokerPhase::Active)),
        Step::reply(Call::Restore, restored_helper_alive()),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    let expected = prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    assert_eq!(controller.fan_applied.as_ref(), Some(&expected));
    assert!(controller.fan_preview.is_none());
    assert!(controller.fan_session_held);
    assert!(!controller.close_ready());
    assert_owner_not_dropped(&audit);
    assert_eq!(controller.apply_fan(), Err(SubmitError::PreviewRequired));
    assert_eq!(audit.lock().unwrap().applied, [expected]);
    controller.restore_fan().unwrap();
    finish(&mut controller);
    assert!(!controller.fan_session_held);
    assert!(controller.fan_restoration_confirmed);
    assert!(!controller.fan_helper_reaped);
    assert!(
        !controller.close_ready(),
        "live helper still needs an observed shutdown"
    );
    close(&mut controller);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Restore), 1);
    assert_eq!(count(&audit, Call::Shutdown), 1);
    assert!(calls(&audit).ends_with(&[Call::Drop { held: false }]));
}

#[test]
fn failed_apply_with_verified_restore_keeps_failure_and_does_not_claim_applied() {
    let mut update = restored_helper_alive();
    update.error = Some(ControlError::new(
        "fan_apply_failed",
        "the requested curve failed and original bytes were restored",
    ));
    let (mut controller, audit) = controller(vec![
        Step::reply(Call::Apply, update),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    assert!(controller.fan_applied.is_none() && controller.fan_preview.is_none());
    assert_eq!(
        controller.fan_error.as_ref().unwrap().code,
        "fan_apply_failed"
    );
    assert!(controller.fan_restoration_confirmed && !controller.fan_session_held);
    close(&mut controller);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

#[test]
fn apply_and_shutdown_uncertainty_keep_close_blocked_until_explicit_recovery() {
    let (mut controller, audit) = controller(vec![
        Step::reply(Call::Apply, unknown("fan_confirmation_unknown")),
        Step::reply(Call::Shutdown, unknown("fan_shutdown_unknown")),
        Step::reply(Call::Restore, restored_helper_alive()),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    assert!(controller.fan_applied.is_none());
    assert!(controller.fan_session_held && !controller.fan_cleanup_complete);
    controller.request_close();
    finish(&mut controller);
    assert!(!controller.close_ready());
    assert_owner_not_dropped(&audit);
    for _ in 0..3 {
        controller.request_close();
        controller.poll();
    }
    assert_eq!(
        count(&audit, Call::Shutdown),
        1,
        "repeated close must not replay a failed cleanup command"
    );
    controller.restore_fan().unwrap();
    until(&mut controller, Controller::close_ready);
    assert_eq!(count(&audit, Call::Restore), 1);
    assert_eq!(count(&audit, Call::Shutdown), 2);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

#[test]
fn duplicate_apply_is_rejected_while_the_single_worker_owns_the_transaction() {
    let (blocked, control) = gate();
    let (mut controller, audit) = controller(vec![
        Step::blocked(Call::Apply, held(BrokerPhase::Active), blocked),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    control.wait();
    assert!(controller.write_in_progress());
    assert_eq!(controller.apply_fan(), Err(SubmitError::Busy));
    assert_eq!(controller.refresh_fan(), Err(SubmitError::Busy));
    assert_eq!(controller.restore_fan(), Err(SubmitError::Busy));
    assert_eq!(count(&audit, Call::Apply), 1);
    control.release();
    finish(&mut controller);
    close(&mut controller);
}

#[test]
fn worker_panic_keeps_the_backend_available_for_explicit_restore() {
    let (mut controller, audit) = controller(vec![
        Step::panic(Call::Apply),
        Step::reply(Call::Restore, restored_helper_alive()),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    assert!(controller.fan_applied.is_none());
    assert!(controller.fan_error.is_some());
    assert!(controller.fan_session_held && !controller.fan_cleanup_complete);
    assert!(
        controller.worker_available,
        "fan recovery must retain the backend after panic"
    );
    assert_eq!(audit.lock().unwrap().planned_panics, 1);
    assert_owner_not_dropped(&audit);
    controller.restore_fan().unwrap();
    finish(&mut controller);
    close(&mut controller);
    assert_eq!(count(&audit, Call::Restore), 1);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

#[test]
fn fresh_revocation_clears_old_applied_display_without_replaying_the_curve() {
    let mut revoked = restored_helper_alive();
    let state = revoked.state.as_mut().unwrap();
    state.epoch += 1;
    state.revoked = true;
    let (mut controller, audit) = controller(vec![
        Step::reply(Call::Apply, held(BrokerPhase::Active)),
        Step::reply(Call::Refresh, revoked),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    assert!(controller.fan_applied.is_some());
    controller.refresh_fan().unwrap();
    finish(&mut controller);
    assert!(controller.fan_state.as_ref().unwrap().revoked);
    assert!(
        controller.fan_applied.is_none(),
        "a revoked lease cannot keep an applied badge"
    );
    assert!(!controller.fan_session_held);
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}

#[test]
fn native_close_during_apply_waits_for_shutdown_receipt_without_freezing_or_duplicate_writes() {
    let (apply_gate, apply_control) = gate();
    let (shutdown_gate, shutdown_control) = gate();
    let (mut controller, audit) = controller(vec![
        Step::blocked(Call::Apply, held(BrokerPhase::Active), apply_gate),
        Step::blocked(Call::Shutdown, shutdown_restored(), shutdown_gate),
    ]);
    prepare(&mut controller);
    let mut app = app(controller);
    let ctx = Context::default();
    render(&ctx, &mut app, false);
    app.controller.apply_fan().unwrap();
    apply_control.wait();
    let start = Instant::now();
    let commands = render(&ctx, &mut app, true);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "close blocked the render thread"
    );
    assert!(commands.contains(&ViewportCommand::CancelClose));
    assert!(!commands.contains(&ViewportCommand::Close));
    assert_owner_not_dropped(&audit);
    let commands = render(&ctx, &mut app, true);
    assert!(!commands.contains(&ViewportCommand::Close));
    assert_eq!(count(&audit, Call::Shutdown), 0);
    apply_control.release();
    until(&mut app.controller, |_| count(&audit, Call::Shutdown) == 1);
    shutdown_control.wait();
    assert_eq!(count(&audit, Call::Shutdown), 1);
    let commands = render(&ctx, &mut app, true);
    assert!(!commands.contains(&ViewportCommand::Close));
    assert_owner_not_dropped(&audit);
    shutdown_control.release();
    until(&mut app.controller, Controller::close_ready);
    let commands = render(&ctx, &mut app, false);
    assert!(commands.contains(&ViewportCommand::Close));
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Shutdown), 1);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

#[test]
fn native_close_with_idle_held_lease_starts_cleanup_even_without_an_inflight_write() {
    for phase in [BrokerPhase::Acquired, BrokerPhase::Active] {
        let (blocked, control) = gate();
        let (mut controller, audit) = controller(vec![
            Step::reply(Call::Refresh, held(phase)),
            Step::blocked(Call::Shutdown, shutdown_restored(), blocked),
        ]);
        controller.refresh_fan().unwrap();
        finish(&mut controller);
        assert!(controller.activity().is_none());
        assert!(controller.fan_session_held);
        let mut app = app(controller);
        let ctx = Context::default();
        let commands = render(&ctx, &mut app, true);
        assert!(commands.contains(&ViewportCommand::CancelClose));
        assert!(!commands.contains(&ViewportCommand::Close));
        control.wait();
        assert_owner_not_dropped(&audit);
        let commands = render(&ctx, &mut app, true);
        assert!(!commands.contains(&ViewportCommand::Close));
        assert_eq!(count(&audit, Call::Shutdown), 1);
        control.release();
        until(&mut app.controller, Controller::close_ready);
        assert!(render(&ctx, &mut app, false).contains(&ViewportCommand::Close));
        assert!(!calls(&audit).contains(&Call::Drop { held: true }));
    }
}

#[test]
fn no_write_shutdown_does_not_require_a_fabricated_restoration_receipt() {
    let (mut controller, audit) = controller(vec![Step::reply(Call::Shutdown, clean())]);
    assert!(!controller.fan_restoration_confirmed);
    close(&mut controller);
    assert!(
        controller.fan_helper_reaped
            && controller.fan_cleanup_complete
            && !controller.fan_session_held
    );
    assert!(!controller.fan_restoration_confirmed);
    assert_eq!(count(&audit, Call::Apply), 0);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

#[test]
fn unavailable_stale_foreign_recovery_and_invalid_raw_observations_never_apply() {
    for case in 0..18 {
        let (mut controller, audit) = controller(vec![Step::reply(Call::Shutdown, clean())]);
        prepare(&mut controller);
        match case {
            0 => controller.fan_state = None,
            1 => controller.fan_state.as_mut().unwrap().state_is_current = false,
            2 => controller.fan_state.as_mut().unwrap().live_error = -5,
            3 => controller.fan_state.as_mut().unwrap().live_table = None,
            4 => controller.fan_state.as_mut().unwrap().live_table = Some(vec![0; 95]),
            5 => controller.fan_state.as_mut().unwrap().live_table = Some(vec![0; 97]),
            6 => controller.fan_state.as_mut().unwrap().live_controls = None,
            7 => controller.fan_state.as_mut().unwrap().capabilities = 0,
            8 => controller.fan_state.as_mut().unwrap().capabilities = 3,
            9 => controller.fan_state = Some(state(BrokerPhase::Active)),
            10 => {
                controller.fan_state = Some(state(BrokerPhase::RecoveryRequired));
                controller.fan_session_held = true;
            }
            11 => controller.fan_state.as_mut().unwrap().owner = true,
            12 => controller.fan_state.as_mut().unwrap().modified_or_uncertain = true,
            13 => controller.fan_state.as_mut().unwrap().revoked = true,
            14 => controller.fan_state = Some(state(BrokerPhase::Acquired)),
            15 => controller.fan_session_held = true,
            16 => {
                controller.fan_state = Some(state(BrokerPhase::Active));
                controller.fan_state.as_mut().unwrap().owner = false;
                controller.fan_session_held = true;
            }
            17 => controller.fan_state.as_mut().unwrap().capabilities = 6,
            _ => unreachable!(),
        }
        assert_eq!(
            controller.apply_fan(),
            Err(SubmitError::Unsupported),
            "availability case {case}"
        );
        assert_eq!(
            count(&audit, Call::Apply),
            0,
            "availability case {case} wrote to broker"
        );
        assert!(controller.fan_applied.is_none());
        close(&mut controller);
    }
}

#[test]
fn missing_mismatched_stale_or_failed_apply_receipts_cannot_set_applied_badge() {
    for case in 0..4 {
        let mut update = held(BrokerPhase::Active);
        let observation = update.state.as_mut().unwrap();
        match case {
            0 => observation.applied_curve = None,
            1 => observation.applied_curve.as_mut().unwrap().cpu.duty_raw[0] = 53,
            2 => observation.state_is_current = false,
            3 => observation.last_error = -5,
            _ => unreachable!(),
        }
        let (mut controller, audit) = controller(vec![
            Step::reply(Call::Apply, update),
            Step::reply(Call::Shutdown, shutdown_restored()),
        ]);
        prepare(&mut controller);
        controller.apply_fan().unwrap();
        finish(&mut controller);
        assert!(
            controller.fan_applied.is_none(),
            "unconfirmed receipt {case} became an applied badge"
        );
        assert_eq!(
            controller.fan_error.as_ref().unwrap().code,
            "fan_apply_unconfirmed"
        );
        assert!(controller.fan_session_held && !controller.fan_cleanup_complete);
        close(&mut controller);
        assert_eq!(count(&audit, Call::Apply), 1);
    }
}

#[test]
fn a_late_apply_receipt_keeps_the_submitted_curve_separate_from_new_local_edits() {
    let (blocked, control) = gate();
    let (mut controller, audit) = controller(vec![
        Step::blocked(Call::Apply, held(BrokerPhase::Active), blocked),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    let submitted = prepare(&mut controller);
    controller.apply_fan().unwrap();
    control.wait();
    let mut edited = curve();
    edited.cpu.duty_raw[0] = 53;
    let expected_edit = edited.clone().validate().unwrap();
    controller.set_fan_draft(edited).unwrap();
    assert!(controller.fan_preview.is_none());
    control.release();
    finish(&mut controller);
    assert_eq!(controller.fan_applied.as_ref(), Some(&submitted));
    assert_eq!(controller.fan_draft.as_ref(), Some(&expected_edit));
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}

#[test]
fn acknowledged_cleanup_cannot_close_before_the_worker_has_finished_dropping_and_joined() {
    let (drop_gate, drop_control) = gate();
    let (mut controller, audit) = controller_with_drop_gate(
        vec![
            Step::reply(Call::Apply, held(BrokerPhase::Active)),
            Step::reply(Call::Shutdown, shutdown_restored()),
        ],
        Some(drop_gate),
    );
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    controller.request_close();
    until(&mut controller, |controller| {
        controller.fan_cleanup_complete
            && controller.fan_helper_reaped
            && !controller.fan_session_held
    });
    drop_control.wait();
    assert!(
        !controller.close_ready(),
        "a cleanup receipt is not evidence that the worker has exited"
    );
    let mut app = app(controller);
    let ctx = Context::default();
    let start = Instant::now();
    let commands = render(&ctx, &mut app, true);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "joining an unfinished worker blocked the UI"
    );
    assert!(commands.contains(&ViewportCommand::CancelClose));
    assert!(!commands.contains(&ViewportCommand::Close));
    assert!(calls(&audit).contains(&Call::DropStarted { held: false }));
    assert!(!calls(&audit)
        .iter()
        .any(|call| matches!(call, Call::Drop { .. })));
    drop_control.release();
    until(&mut app.controller, Controller::close_ready);
    assert!(render(&ctx, &mut app, false).contains(&ViewportCommand::Close));
    assert!(calls(&audit).ends_with(&[Call::Drop { held: false }]));
}

#[test]
fn idle_worker_observes_helper_failure_without_manual_refresh_or_new_authorization() {
    let (mut controller, audit, lifecycle) = controller_fixture(
        vec![
            Step::reply(Call::Apply, held(BrokerPhase::Active)),
            Step::reply(Call::Restore, restored_helper_alive()),
            Step::reply(Call::Shutdown, shutdown_restored()),
        ],
        None,
    );
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    assert!(controller.fan_applied.is_some());
    assert!(controller.activity().is_none());
    lifecycle.send(unknown("fan_child_exit_unknown")).unwrap();
    until(&mut controller, |controller| {
        controller
            .fan_error
            .as_ref()
            .is_some_and(|error| error.code == "fan_child_exit_unknown")
    });
    assert!(
        controller.fan_applied.is_none(),
        "a dead helper cannot leave a current applied badge"
    );
    assert!(controller.fan_session_held && !controller.fan_cleanup_complete);
    assert!(!controller.close_ready());
    assert_eq!(count(&audit, Call::Lifecycle), 1);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(
        count(&audit, Call::Probe),
        0,
        "idle polling must not start a new device probe"
    );
    assert_eq!(
        count(&audit, Call::Refresh),
        0,
        "idle polling must not perform an EC refresh"
    );
    assert_owner_not_dropped(&audit);
    controller.restore_fan().unwrap();
    finish(&mut controller);
    close(&mut controller);
    assert_eq!(count(&audit, Call::Restore), 1);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

#[test]
fn idle_verified_revocation_updates_ownership_without_reapplying_or_refreshing_hardware() {
    let (mut controller, audit, lifecycle) = controller_fixture(
        vec![
            Step::reply(Call::Apply, held(BrokerPhase::Active)),
            Step::reply(Call::Shutdown, shutdown_restored()),
        ],
        None,
    );
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    let mut revoked = shutdown_restored();
    let state = revoked.state.as_mut().unwrap();
    state.epoch += 1;
    state.revoked = true;
    lifecycle.send(revoked).unwrap();
    until(&mut controller, |controller| {
        controller
            .fan_state
            .as_ref()
            .is_some_and(|state| state.revoked)
    });
    assert!(controller.fan_applied.is_none());
    assert!(!controller.fan_session_held);
    assert!(
        controller.fan_helper_reaped
            && controller.fan_restoration_confirmed
            && controller.fan_cleanup_complete
    );
    assert_eq!(count(&audit, Call::Lifecycle), 1);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Probe), 0);
    assert_eq!(count(&audit, Call::Refresh), 0);
    assert_eq!(
        count(&audit, Call::Restore),
        0,
        "a verified idle receipt must not trigger a redundant restore"
    );
    close(&mut controller);
}

#[test]
fn local_edits_preview_and_invalid_import_preserve_the_actual_failure_separately() {
    let (mut controller, audit) = controller(vec![
        Step::reply(Call::Apply, unknown("fan_original_failure")),
        Step::reply(Call::Restore, restored_helper_alive()),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    let original_message = controller.fan_error.as_ref().unwrap().message.clone();
    let mut edited = curve();
    edited.cpu.duty_raw[0] = 53;
    controller.set_fan_draft(edited).unwrap();
    controller.preview_fan().unwrap();
    assert!(controller.fan_draft_error.is_none());
    assert_eq!(
        controller.fan_error.as_ref().unwrap().code,
        "fan_original_failure"
    );
    assert_eq!(
        controller.fan_error.as_ref().unwrap().message,
        original_message
    );
    controller.import_fan_curve(b"{invalid").unwrap_err();
    assert!(controller.fan_draft_error.is_some());
    assert_eq!(
        controller.fan_error.as_ref().unwrap().code,
        "fan_original_failure"
    );
    let mut invalid = curve();
    invalid.cpu.duty_raw[0] = 201;
    controller.set_fan_draft(invalid).unwrap_err();
    assert_eq!(
        controller.fan_draft_error.as_ref().unwrap().code,
        "fan_curve_duty_out_of_range"
    );
    assert_eq!(
        controller.fan_error.as_ref().unwrap().code,
        "fan_original_failure"
    );
    assert!(controller.fan_session_held && !controller.fan_cleanup_complete);
    assert!(controller.fan_applied.is_none());
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Probe), 0);
    assert_eq!(count(&audit, Call::Refresh), 0);
    controller.restore_fan().unwrap();
    finish(&mut controller);
    close(&mut controller);
}

#[test]
fn contradictory_clean_shutdown_with_a_current_local_owner_cannot_drop_or_close() {
    let mut contradictory = held(BrokerPhase::Active);
    contradictory.session_held = false;
    contradictory.helper_reaped = true;
    contradictory.cleanup_complete = true;
    contradictory.restoration_confirmed = true;
    let mut failed_shutdown = Step::reply(Call::Shutdown, contradictory);
    // The fake owner remains live independently of the contradictory receipt.
    failed_shutdown.held_after = true;
    let (mut controller, audit) = controller(vec![
        Step::reply(Call::Apply, held(BrokerPhase::Active)),
        failed_shutdown,
        Step::reply(Call::Restore, restored_helper_alive()),
        Step::reply(Call::Shutdown, shutdown_restored()),
    ]);
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    controller.request_close();
    finish(&mut controller);
    assert!(!controller.close_ready());
    assert!(controller.fan_session_held);
    assert!(!controller.fan_cleanup_complete && !controller.fan_restoration_confirmed);
    assert_eq!(
        controller.fan_error.as_ref().unwrap().code,
        "fan_cleanup_unconfirmed"
    );
    assert_owner_not_dropped(&audit);
    assert_eq!(count(&audit, Call::Shutdown), 1);
    controller.restore_fan().unwrap();
    until(&mut controller, Controller::close_ready);
    assert_eq!(count(&audit, Call::Restore), 1);
    assert_eq!(count(&audit, Call::Shutdown), 2);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

#[test]
fn passive_verified_cleanup_after_failed_close_finishes_worker_without_explicit_restore() {
    let (mut controller, audit, lifecycle) = controller_fixture(
        vec![
            Step::reply(Call::Apply, held(BrokerPhase::Active)),
            Step::reply(Call::Shutdown, unknown("fan_shutdown_unknown")),
            Step::reply(Call::Shutdown, shutdown_restored()),
        ],
        None,
    );
    prepare(&mut controller);
    controller.apply_fan().unwrap();
    finish(&mut controller);
    controller.request_close();
    finish(&mut controller);
    assert!(!controller.close_ready());
    assert!(controller.fan_session_held && !controller.fan_cleanup_complete);
    assert_eq!(count(&audit, Call::Shutdown), 1);
    assert_owner_not_dropped(&audit);
    lifecycle.send(shutdown_restored()).unwrap();
    until(&mut controller, Controller::close_ready);
    assert_eq!(count(&audit, Call::Lifecycle), 1);
    assert_eq!(count(&audit, Call::Restore), 0);
    assert_eq!(count(&audit, Call::Apply), 1);
    assert_eq!(count(&audit, Call::Shutdown), 2);
    assert_eq!(count(&audit, Call::Probe), 0);
    assert_eq!(count(&audit, Call::Refresh), 0);
    assert!(calls(&audit).ends_with(&[Call::Drop { held: false }]));
}
