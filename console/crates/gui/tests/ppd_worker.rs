//! Exercise the production Controller's OS-profile worker with in-memory PPD
//! observations. No fixture opens D-Bus, authorizes a request or sets a profile.
use mechrevo_console_core::{
    brightness::BrightnessBackend,
    fan_curve::{client::FanClientUpdate, FanCurve, FanCurveChannel, FanCurveLayout},
    power_profiles_daemon::{
        AdvertisedProfile, ProfileApplyOutcome, ProfileApplyReceipt, ProfileApplyStage,
        ProfileDaemonIdentity, ProfileDetails, ProfileHold, ProfileHoldEffect,
        PROFILE_RECEIPT_MARKER,
    },
    ApplyResult, BrightnessPlan, Capability, CapabilityStatus, CatalogModel, ControlError,
    EvidenceSource, OemControlKind, OemDomain, OemPlan, OemValue, PowerPolicy, PowerProfile,
    ProfilePlan, Snapshot, ThermalSample,
};
use mechrevo_console_gui::{Backend, Controller, SubmitError};
use serde_json::json;
use std::{
    collections::{BTreeMap, VecDeque},
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
    Plan,
    Apply,
    Shutdown,
    DropStarted,
    Drop,
}
#[derive(Default)]
struct Audit {
    calls: Vec<Call>,
    planned: Vec<PowerProfile>,
    submitted: Vec<ProfilePlan>,
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
        "fixture protocol violation: {:?}",
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
        .expect("test did not release worker");
}
struct Provider {
    current: String,
    holds: Vec<ProfileHold>,
}
type SharedProvider = Arc<Mutex<Provider>>;
struct ApplyStep {
    error: Option<ControlError>,
    side_effect_before_error: bool,
    gate: Option<Gate>,
}
fn success() -> ApplyStep {
    ApplyStep {
        error: None,
        side_effect_before_error: false,
        gate: None,
    }
}
struct PpdBackend {
    audit: SharedAudit,
    provider: SharedProvider,
    applies: VecDeque<ApplyStep>,
    preview_gate: Option<Gate>,
    thermal_gate: Option<Gate>,
    drop_gate: Option<Gate>,
    preview_errors: VecDeque<ControlError>,
    snapshot_panics: VecDeque<bool>,
}
impl Backend for PpdBackend {
    fn snapshot(&mut self) -> Snapshot {
        record(&self.audit, Call::Snapshot);
        assert!(
            !self.snapshot_panics.pop_front().unwrap_or(false),
            "scripted ancillary snapshot panic"
        );
        snapshot(&self.provider.lock().unwrap())
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        record(&self.audit, Call::Catalog);
        vec![]
    }
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
        record(&self.audit, Call::Thermal);
        if let Some(gate) = self.thermal_gate.take() {
            block(gate);
        }
        let time = Instant::now();
        Ok(ThermalSample {
            readings: vec![],
            diagnostics: vec![],
            started_at: time,
            finished_at: time,
        })
    }
    fn plan(&mut self, profile: PowerProfile) -> Result<ProfilePlan, ControlError> {
        record(&self.audit, Call::Plan);
        self.audit.lock().unwrap().planned.push(profile);
        if let Some(gate) = self.preview_gate.take() {
            block(gate);
        }
        if let Some(error) = self.preview_errors.pop_front() {
            return Err(error);
        }
        let state = self.provider.lock().unwrap();
        let mut plan_details = details(state.holds.clone());
        plan_details.holds_will_be_released =
            profile.to_string() != state.current && !state.holds.is_empty();
        // This is a scripted provider result. Eligibility and the PPD versus
        // platform-profile namespace are tested through production core below.
        Ok(ProfilePlan {
            requested: profile,
            target: profile.to_string(),
            previous: state.current.clone(),
            scope: "fixture PPD system profile".into(),
            source: source(),
            details: Some(plan_details),
        })
    }
    fn apply(&mut self, plan: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        record(&self.audit, Call::Apply);
        self.audit.lock().unwrap().submitted.push(plan.clone());
        let Some(step) = self.applies.pop_front() else {
            self.audit
                .lock()
                .unwrap()
                .violations
                .push("unexpected setter/rollback/retry".into());
            panic!("unexpected PPD apply");
        };
        if let Some(gate) = step.gate {
            block(gate);
        }
        let before = self.provider.lock().unwrap().holds.clone();
        if step.error.is_none() || step.side_effect_before_error {
            let mut state = self.provider.lock().unwrap();
            state.current = plan.target.clone();
            state.holds.clear();
        }
        if let Some(error) = step.error {
            return Err(error);
        }
        Ok(ApplyResult {
            requested: plan.requested,
            confirmed: plan.target.clone(),
            previous: plan.previous.clone(),
            scope: plan.scope.clone(),
            receipt: Some(confirmed_receipt(&plan.target, before)),
        })
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        record(&self.audit, Call::Shutdown);
        FanClientUpdate::default()
    }
}
impl Drop for PpdBackend {
    fn drop(&mut self) {
        record(&self.audit, Call::DropStarted);
        if let Some(gate) = self.drop_gate.take() {
            block(gate);
        }
        record(&self.audit, Call::Drop);
    }
}
fn source() -> EvidenceSource {
    EvidenceSource {
        backend: "linux_power_profiles_daemon".into(),
        path: "fixture://system-bus/PowerProfiles".into(),
        device_id: Some(":1.42".into()),
    }
}
fn hold() -> ProfileHold {
    ProfileHold {
        application_id: "fixture.app".into(),
        profile: PowerProfile::Performance,
        reason: "synthetic workload".into(),
        attributes: BTreeMap::new(),
    }
}
fn details(holds: Vec<ProfileHold>) -> ProfileDetails {
    ProfileDetails {
        daemon: ProfileDaemonIdentity {
            bus_id: "0".repeat(32),
            unique_owner: ":1.42".into(),
            bus_name: "org.freedesktop.UPower.PowerProfiles".into(),
            object_path: "/org/freedesktop/UPower/PowerProfiles".into(),
            interface: "org.freedesktop.UPower.PowerProfiles".into(),
            unix_user: 0,
        },
        performance_degraded: "fixture degraded reason".into(),
        holds_will_be_released: false,
        holds,
        advertised_profiles: [
            PowerProfile::Balanced,
            PowerProfile::Performance,
            PowerProfile::PowerSaver,
        ]
        .into_iter()
        .map(|profile| AdvertisedProfile {
            profile,
            attributes: BTreeMap::from([
                ("Driver".into(), "multiple".into()),
                ("CpuDriver".into(), "intel_pstate".into()),
            ]),
        })
        .collect(),
    }
}
fn snapshot(state: &Provider) -> Snapshot {
    let options = vec![
        "balanced".into(),
        "performance".into(),
        "power-saver".into(),
    ];
    Snapshot {
        platform: "linux".into(),
        identity: Default::default(),
        telemetry: vec![],
        diagnostics: vec![],
        brightness: vec![],
        oem_controls: vec![],
        capabilities: vec![Capability {
            id: "power_profile".into(),
            label: "fixture system profile".into(),
            status: CapabilityStatus::Available,
            reason: "synthetic PPD observation".into(),
            source: Some(source()),
            options: options.clone(),
        }],
        power_policy: PowerPolicy {
            scope: "fixture PPD system profile".into(),
            current: Some(state.current.clone()),
            options,
            source: Some(source()),
            details: Some(details(state.holds.clone())),
        },
    }
}
fn confirmed_receipt(target: &str, holds_before: Vec<ProfileHold>) -> ProfileApplyReceipt {
    ProfileApplyReceipt {
        outcome: ProfileApplyOutcome::Confirmed,
        stage: ProfileApplyStage::Complete,
        submitted: true,
        acknowledged: Some(true),
        writer_may_continue: false,
        readback: Some(target.into()),
        authority_unchanged: Some(true),
        holds_before,
        holds_after: Some(vec![]),
        hold_effect: ProfileHoldEffect::ReleasedObserved,
        primary_error: None,
        readback_error: None,
    }
}
fn failed_receipt() -> ProfileApplyReceipt {
    ProfileApplyReceipt {
        outcome: ProfileApplyOutcome::Unknown,
        stage: ProfileApplyStage::Readback,
        submitted: true,
        acknowledged: Some(false),
        writer_may_continue: true,
        readback: None,
        authority_unchanged: Some(true),
        holds_before: vec![hold()],
        holds_after: None,
        hold_effect: ProfileHoldEffect::Unknown,
        primary_error: Some(ControlError::new(
            "fixture_setter_failed",
            "setter failed after possible side effects",
        )),
        readback_error: Some(ControlError::new(
            "fixture_readback_failed",
            "independent readback failed",
        )),
    }
}
fn failure(receipt: &ProfileApplyReceipt) -> ControlError {
    let mut error = ControlError::new(
        "fixture_profile_failed",
        "profile transaction failed; outcome remains unknown",
    );
    error.details = vec![
        "primary setter evidence".into(),
        "readback evidence".into(),
        format!(
            "{PROFILE_RECEIPT_MARKER}{}",
            serde_json::to_string(receipt).unwrap()
        ),
    ];
    error
}
fn fixture(applies: Vec<ApplyStep>) -> (Controller, SharedAudit, SharedProvider) {
    configured_fixture(applies, None, None, None, vec![], vec![])
}
fn configured_fixture(
    applies: Vec<ApplyStep>,
    preview_gate: Option<Gate>,
    thermal_gate: Option<Gate>,
    drop_gate: Option<Gate>,
    preview_errors: Vec<ControlError>,
    snapshot_panics: Vec<bool>,
) -> (Controller, SharedAudit, SharedProvider) {
    let audit = Arc::new(Mutex::new(Audit::default()));
    let provider = Arc::new(Mutex::new(Provider {
        current: "balanced".into(),
        holds: vec![hold()],
    }));
    let mut controller = Controller::new(PpdBackend {
        audit: audit.clone(),
        provider: provider.clone(),
        applies: applies.into(),
        preview_gate,
        thermal_gate,
        drop_gate,
        preview_errors: preview_errors.into(),
        snapshot_panics: snapshot_panics.into(),
    })
    .unwrap();
    controller.snapshot = Some(snapshot(&provider.lock().unwrap()));
    controller.last_refresh = Some(Instant::now() - Duration::from_secs(30));
    controller.selected_profile = PowerProfile::PowerSaver;
    (controller, audit, provider)
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
    controller.preview_profile().unwrap();
    finish(controller);
    assert!(controller.preview.is_some());
}
fn apply(controller: &mut Controller) {
    controller.apply_preview().unwrap();
    finish(controller);
    assert!(controller.preview.is_none());
}
fn close(controller: &mut Controller) {
    controller.request_close();
    until(controller, Controller::close_ready);
}
fn foreign_brightness() -> BrightnessPlan {
    BrightnessPlan {
        device_id: "fixture-panel".into(),
        requested_percent: 60,
        target: 60,
        previous: 50,
        maximum: 100,
        source: source(),
        backend: BrightnessBackend::LinuxSysfs,
        logind_session: None,
    }
}
fn foreign_oem() -> OemPlan {
    OemPlan {
        control_id: "fixture-fn".into(),
        kind: OemControlKind::FnLock,
        label: "fixture".into(),
        previous: OemValue::Boolean(false),
        target: OemValue::Boolean(true),
        domain: OemDomain::Boolean,
        source: source(),
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
fn refresh_selection_and_preview_are_read_only_and_power_saver_uses_the_ppd_namespace() {
    let (mut controller, audit, _) = fixture(vec![]);
    assert!(calls(&audit).is_empty());
    controller.refresh().unwrap();
    finish(&mut controller);
    controller.select_profile(PowerProfile::Performance);
    controller.select_profile(PowerProfile::PowerSaver);
    assert_eq!(count(&audit, Call::Plan), 0);
    assert_eq!(count(&audit, Call::Apply), 0);
    let snapshot = controller.snapshot.as_ref().unwrap();
    assert!(mechrevo_console_core::profile_available(
        snapshot,
        PowerProfile::PowerSaver
    ));
    let mut raw_platform = snapshot.clone();
    raw_platform.power_policy.source.as_mut().unwrap().backend = "linux-platform-profile".into();
    assert!(!mechrevo_console_core::profile_available(
        &raw_platform,
        PowerProfile::PowerSaver
    ));
    raw_platform.power_policy.options[2] = "low-power".into();
    assert!(mechrevo_console_core::profile_available(
        &raw_platform,
        PowerProfile::PowerSaver
    ));
    preview(&mut controller);
    let plan = controller.preview.as_ref().unwrap();
    assert_eq!(plan.requested, PowerProfile::PowerSaver);
    assert_eq!(plan.target, "power-saver");
    assert!(plan.details.as_ref().unwrap().holds_will_be_released);
    assert_eq!(plan.details.as_ref().unwrap().holds.len(), 1);
    assert_eq!(count(&audit, Call::Apply), 0);
    close(&mut controller);
    assert_eq!(count(&audit, Call::Apply), 0);
}

#[test]
fn a_to_b_to_a_during_preview_does_not_revive_the_old_plan() {
    let (blocked, preview_gate) = gate();
    let (mut controller, audit, _) =
        configured_fixture(vec![success()], Some(blocked), None, None, vec![], vec![]);
    controller.preview_profile().unwrap();
    preview_gate.wait();
    controller.select_profile(PowerProfile::Performance);
    controller.select_profile(PowerProfile::PowerSaver);
    preview_gate.release();
    finish(&mut controller);
    assert!(controller.preview.is_none());
    assert_eq!(
        controller.apply_preview(),
        Err(SubmitError::PreviewRequired)
    );
    assert_eq!(count(&audit, Call::Apply), 0);
    preview(&mut controller);
    apply(&mut controller);
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}

#[test]
fn rejected_busy_preview_preserves_foreign_previews_and_error_evidence() {
    let (blocked, thermal_gate) = gate();
    let (mut controller, audit, _) =
        configured_fixture(vec![], None, Some(blocked), None, vec![], vec![]);
    controller.refresh_thermal().unwrap();
    thermal_gate.wait();
    controller.brightness_preview = Some(foreign_brightness());
    controller.oem_preview = Some(foreign_oem());
    controller.fan_preview = Some(fan_curve().validate().unwrap());
    controller.error = Some(ControlError::new(
        "fixture_existing_failure",
        "retain unrelated error",
    ));
    let old = (
        json!(controller.brightness_preview),
        json!(controller.oem_preview),
        controller.fan_preview.clone(),
    );
    let before = calls(&audit);
    assert_eq!(controller.preview_profile(), Err(SubmitError::Busy));
    assert_eq!(json!(controller.brightness_preview), old.0);
    assert_eq!(json!(controller.oem_preview), old.1);
    assert_eq!(controller.fan_preview, old.2);
    assert_eq!(
        controller.error.as_ref().unwrap().code,
        "fixture_existing_failure"
    );
    assert_eq!(calls(&audit), before);
    thermal_gate.release();
    finish(&mut controller);
    close(&mut controller);
}

#[test]
fn failed_setter_with_side_effects_retains_both_errors_and_receipt_through_fresh_observation() {
    let receipt = failed_receipt();
    let error = failure(&receipt);
    let (mut controller, audit, provider) = fixture(vec![ApplyStep {
        error: Some(error.clone()),
        side_effect_before_error: true,
        gate: None,
    }]);
    preview(&mut controller);
    apply(&mut controller);
    assert_eq!(provider.lock().unwrap().current, "power-saver");
    assert!(provider.lock().unwrap().holds.is_empty());
    assert!(controller.applied.is_none());
    controller.refresh().unwrap();
    finish(&mut controller);
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert_eq!(
        controller
            .snapshot
            .as_ref()
            .unwrap()
            .power_policy
            .current
            .as_deref(),
        Some("power-saver")
    );
    assert!(
        controller.applied.is_none(),
        "matching later observation is not a successful transaction receipt"
    );
    assert_eq!(json!(controller.profile_receipt), json!(receipt));
    assert!(
        controller
            .profile_receipt
            .as_ref()
            .unwrap()
            .writer_may_continue
    );
    let retained = controller.profile_error.as_ref().unwrap();
    assert_eq!(retained.code, error.code);
    assert_eq!(retained.message, error.message);
    assert_eq!(retained.details, error.details);
    assert_eq!(
        controller.apply_preview(),
        Err(SubmitError::PreviewRequired)
    );
    assert_eq!(
        count(&audit, Call::Apply),
        1,
        "failure automatically retried or rolled back the setter"
    );
    close(&mut controller);
    assert_eq!(count(&audit, Call::Apply), 1);
}

#[test]
fn successful_apply_keeps_its_receipt_when_the_ancillary_snapshot_panics() {
    let (mut controller, audit, _) =
        configured_fixture(vec![success()], None, None, None, vec![], vec![true]);
    let old_snapshot = json!(controller.snapshot);
    let old_refresh = controller.last_refresh;
    preview(&mut controller);
    apply(&mut controller);
    assert_eq!(
        controller.applied.as_ref().unwrap().confirmed,
        "power-saver"
    );
    assert_eq!(
        controller.profile_receipt.as_ref().unwrap().outcome,
        ProfileApplyOutcome::Confirmed
    );
    assert!(controller.profile_error.is_none() && controller.profile_refresh_error.is_some());
    assert_eq!(json!(controller.snapshot), old_snapshot);
    assert_eq!(
        controller.last_refresh, old_refresh,
        "failed ancillary read invented a new snapshot timestamp"
    );
    assert!(controller.worker_available && controller.fan_error.is_none());
    assert!(
        !controller.fan_session_held
            && controller.fan_helper_reaped
            && controller.fan_cleanup_complete
    );
    assert_eq!(count(&audit, Call::Apply), 1);
    let receipt = json!(controller.profile_receipt);
    controller.refresh().unwrap();
    finish(&mut controller);
    assert!(controller.profile_refresh_error.is_none());
    assert_eq!(json!(controller.profile_receipt), receipt);
    assert_eq!(
        controller
            .snapshot
            .as_ref()
            .unwrap()
            .power_policy
            .current
            .as_deref(),
        Some("power-saver")
    );
    assert_eq!(count(&audit, Call::Apply), 1);
    close(&mut controller);
}

#[test]
fn close_waits_for_actual_apply_and_backend_exit_on_success_or_side_effect_failure() {
    for fails in [false, true] {
        let (blocked_apply, apply_gate) = gate();
        let (blocked_drop, drop_gate) = gate();
        let transaction = ApplyStep {
            error: fails.then(|| failure(&failed_receipt())),
            side_effect_before_error: fails,
            gate: Some(blocked_apply),
        };
        let (mut controller, audit, _) = configured_fixture(
            vec![transaction],
            None,
            None,
            Some(blocked_drop),
            vec![],
            vec![],
        );
        preview(&mut controller);
        controller.apply_preview().unwrap();
        apply_gate.wait();
        controller.request_close();
        controller.request_close();
        controller.poll();
        assert!(!controller.close_ready() && controller.write_in_progress());
        assert_eq!(count(&audit, Call::Shutdown), 0);
        apply_gate.release();
        until(&mut controller, |controller| {
            if fails {
                controller.profile_error.is_some()
            } else {
                controller.applied.is_some()
            }
        });
        drop_gate.wait();
        controller.poll();
        assert!(
            !controller.close_ready(),
            "transaction reply is not worker reaping"
        );
        assert_eq!(
            controller.profile_receipt.as_ref().unwrap().outcome,
            if fails {
                ProfileApplyOutcome::Unknown
            } else {
                ProfileApplyOutcome::Confirmed
            }
        );
        drop_gate.release();
        until(&mut controller, Controller::close_ready);
        assert_eq!(count(&audit, Call::Apply), 1);
        assert_eq!(count(&audit, Call::Shutdown), 1);
    }
}

#[test]
fn close_during_read_only_preview_waits_and_never_applies_the_late_plan() {
    let (blocked, preview_gate) = gate();
    let (mut controller, audit, _) =
        configured_fixture(vec![], Some(blocked), None, None, vec![], vec![]);
    controller.preview_profile().unwrap();
    preview_gate.wait();
    controller.request_close();
    controller.poll();
    assert!(!controller.close_ready());
    assert_eq!(count(&audit, Call::Shutdown), 0);
    preview_gate.release();
    until(&mut controller, Controller::close_ready);
    assert!(controller.preview.is_none());
    assert_eq!(count(&audit, Call::Apply), 0);
}

#[test]
fn failed_preview_retains_read_only_error_until_an_explicit_successful_preview() {
    let mut error = ControlError::new("fixture_preview_failed", "PPD observation failed");
    error.details = vec!["daemon owner changed while reading".into()];
    let (mut controller, audit, _) =
        configured_fixture(vec![], None, None, None, vec![error.clone()], vec![]);
    controller.preview_profile().unwrap();
    finish(&mut controller);
    assert!(controller.preview.is_none() && controller.profile_error.is_none());
    assert_eq!(
        controller.profile_preview_error.as_ref().unwrap().details,
        error.details
    );
    controller.refresh().unwrap();
    finish(&mut controller);
    assert_eq!(
        controller.profile_preview_error.as_ref().unwrap().details,
        error.details
    );
    preview(&mut controller);
    assert!(controller.profile_preview_error.is_none());
    assert_eq!(count(&audit, Call::Apply), 0);
    close(&mut controller);
}
