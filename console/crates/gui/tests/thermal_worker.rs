//! Exercise the production bounded worker with scripted, in-memory providers.
//! These tests never open a sensor, device, authentication helper or real lease.
use mechrevo_console_core::{
    fan_curve::{
        broker::{BrokerPhase, BrokerState},
        client::FanClientUpdate,
        FanCurve, FanCurveChannel, FanCurveLayout, ValidatedFanCurve,
    },
    ApplyResult, CapabilityStatus, CatalogModel, ControlError, EvidenceSource, OemApplyResult,
    OemPlan, OemValue, PowerProfile, ProfilePlan, SensorReading, Snapshot, ThermalSample,
};
use mechrevo_console_gui::{Backend, Controller, SubmitError};
use std::{
    collections::VecDeque,
    sync::{
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread::ThreadId,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Snapshot,
    Catalog,
    Plan,
    Apply,
    PlanOem,
    ApplyOem,
    Probe,
    ApplyFan,
    RefreshFan,
    Thermal,
    Shutdown,
    Drop { held: bool },
}

#[derive(Default)]
struct Audit {
    calls: Vec<(Call, ThreadId)>,
}
type SharedAudit = Arc<Mutex<Audit>>;

fn record(audit: &SharedAudit, call: Call) {
    audit
        .lock()
        .unwrap()
        .calls
        .push((call, std::thread::current().id()));
}
fn calls(audit: &SharedAudit) -> Vec<Call> {
    audit
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|(call, _)| *call)
        .collect()
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
            .expect("worker did not enter the thermal provider");
    }
    fn release(&self) {
        self.release
            .send(())
            .expect("thermal worker exited before release");
    }
}

enum SampleStep {
    Success {
        temperature: f64,
        rpm: f64,
        gate: Option<Gate>,
    },
    Error,
    Panic,
}
fn success(temperature: f64, rpm: f64) -> SampleStep {
    SampleStep::Success {
        temperature,
        rpm,
        gate: None,
    }
}

struct ThermalBackend {
    audit: SharedAudit,
    samples: VecDeque<SampleStep>,
    held: bool,
    applied_curve: Option<FanCurve>,
}
impl ThermalBackend {
    fn update(&self, recovery: bool) -> FanClientUpdate {
        let phase = if recovery {
            BrokerPhase::RecoveryRequired
        } else if self.held {
            BrokerPhase::Active
        } else {
            BrokerPhase::Idle
        };
        FanClientUpdate {
            state: Some(BrokerState {
                backend: "thermal-fixture".into(),
                device_path: "fixture://no-device".into(),
                epoch: 1,
                generation: 7,
                lease_id: if self.held { 17 } else { 0 },
                last_request_id: 23,
                phase,
                capabilities: 7,
                owner: self.held,
                revoked: false,
                modified_or_uncertain: recovery,
                state_is_current: true,
                restored_verified: false,
                live_error: 0,
                last_error: 0,
                restore_error: 0,
                live_table: Some(vec![51; 96]),
                original_table: self.held.then(|| vec![255; 96]),
                live_controls: Some([0xb5, 0xa9, 0x12, 0x81, 21, 31]),
                original_controls: self.held.then_some([0xb5, 0xa9, 0x12, 0x81, 21, 31]),
                cached_original_targets: self.held.then_some([57, 79, 29, 61]),
                applied_curve: self.applied_curve.clone(),
            }),
            error: recovery.then(|| {
                ControlError::new("fixture_restore_required", "Keep recovery proof visible")
            }),
            session_held: self.held,
            helper_reaped: !self.held,
            restoration_confirmed: false,
            cleanup_complete: !self.held,
        }
    }
}
impl Backend for ThermalBackend {
    fn snapshot(&mut self) -> Snapshot {
        record(&self.audit, Call::Snapshot);
        fixture_snapshot()
    }
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
        record(&self.audit, Call::Thermal);
        let started_at = Instant::now();
        match self
            .samples
            .pop_front()
            .expect("unexpected extra thermal read")
        {
            SampleStep::Success {
                temperature,
                rpm,
                gate,
            } => {
                if let Some(gate) = gate {
                    gate.entered.send(()).unwrap();
                    gate.release
                        .recv_timeout(Duration::from_secs(2))
                        .expect("test failed to release the bounded thermal read");
                }
                Ok(ThermalSample {
                    readings: thermal_readings(temperature, rpm),
                    diagnostics: vec![],
                    started_at,
                    finished_at: Instant::now(),
                })
            }
            SampleStep::Error => Err(ControlError::new(
                "fixture_read_failed",
                "Sensor read failed",
            )),
            SampleStep::Panic => panic!("scripted read-only thermal provider panic"),
        }
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        record(&self.audit, Call::Catalog);
        vec![]
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        record(&self.audit, Call::Plan);
        Err(ControlError::new(
            "fixture_forbidden",
            "Not part of thermal sampling",
        ))
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        record(&self.audit, Call::Apply);
        Err(ControlError::new(
            "fixture_forbidden",
            "Not part of thermal sampling",
        ))
    }
    fn plan_oem(&mut self, _: &str, _: OemValue) -> Result<OemPlan, ControlError> {
        record(&self.audit, Call::PlanOem);
        Err(ControlError::new(
            "fixture_forbidden",
            "Not part of thermal sampling",
        ))
    }
    fn apply_oem(&mut self, _: &OemPlan) -> Result<OemApplyResult, ControlError> {
        record(&self.audit, Call::ApplyOem);
        Err(ControlError::new(
            "fixture_forbidden",
            "Not part of thermal sampling",
        ))
    }
    fn fan_probe(&mut self) -> FanClientUpdate {
        record(&self.audit, Call::Probe);
        self.update(false)
    }
    fn fan_apply(&mut self, curve: &ValidatedFanCurve) -> FanClientUpdate {
        record(&self.audit, Call::ApplyFan);
        self.held = true;
        self.applied_curve = Some(curve.curve().clone());
        self.update(false)
    }
    fn fan_refresh(&mut self) -> FanClientUpdate {
        record(&self.audit, Call::RefreshFan);
        self.update(true)
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        record(&self.audit, Call::Shutdown);
        self.held = false;
        self.applied_curve = None;
        let mut update = self.update(false);
        update.restoration_confirmed = true;
        update.state.as_mut().unwrap().restored_verified = true;
        update
    }
}
impl Drop for ThermalBackend {
    fn drop(&mut self) {
        record(&self.audit, Call::Drop { held: self.held });
    }
}

fn sensor(id: &str, kind: &str, value: f64, unit: &str) -> SensorReading {
    SensorReading {
        id: id.into(),
        label: id.into(),
        device_id: "fixture-cpu".into(),
        kind: kind.into(),
        value: Some(value),
        unit: unit.into(),
        source: EvidenceSource {
            backend: "fixture".into(),
            path: format!("fixture://{id}"),
            device_id: Some("fixture-cpu".into()),
        },
        status: CapabilityStatus::Available,
        reason: None,
    }
}
fn thermal_readings(temperature: f64, rpm: f64) -> Vec<SensorReading> {
    vec![
        sensor("cpu-temperature", "temperature", temperature, "°C"),
        sensor("cpu-fan", "fan_speed", rpm, "RPM"),
    ]
}
fn fixture_snapshot() -> Snapshot {
    let mut telemetry = thermal_readings(42.0, 1800.0);
    telemetry.push(sensor("battery-power", "power", 12.5, "W"));
    Snapshot {
        platform: "linux".into(),
        identity: Default::default(),
        telemetry,
        capabilities: vec![],
        power_policy: Default::default(),
        diagnostics: vec![],
        brightness: vec![],
        oem_controls: vec![],
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
fn controller(samples: Vec<SampleStep>) -> (Controller, SharedAudit) {
    let audit = Arc::new(Mutex::new(Audit::default()));
    let controller = Controller::new(ThermalBackend {
        audit: audit.clone(),
        samples: samples.into(),
        held: false,
        applied_curve: None,
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
        "production worker did not reach the expected state"
    );
}
fn finish(controller: &mut Controller) {
    until(controller, |controller| controller.activity().is_none());
}
fn acquire(controller: &mut Controller) {
    controller.refresh().unwrap();
    finish(controller);
    controller.probe_fan().unwrap();
    finish(controller);
    controller.set_fan_draft(curve()).unwrap();
    controller.preview_fan().unwrap();
    controller.apply_fan().unwrap();
    finish(controller);
    assert!(controller.fan_session_held);
    assert!(controller.fan_applied.is_some());
    assert!(!controller.fan_helper_reaped && !controller.fan_cleanup_complete);
}
fn close(controller: &mut Controller) {
    controller.request_close();
    until(controller, Controller::close_ready);
}
fn value(readings: &[SensorReading], id: &str) -> Option<f64> {
    readings
        .iter()
        .find(|reading| reading.id == id)
        .unwrap()
        .value
}
fn assert_no_ordinary_control(audit: &SharedAudit) {
    assert_eq!(count(audit, Call::Plan), 0);
    assert_eq!(count(audit, Call::Apply), 0);
    assert_eq!(count(audit, Call::PlanOem), 0);
    assert_eq!(count(audit, Call::ApplyOem), 0);
}

#[test]
fn held_lease_samples_temperature_and_rpm_without_full_discovery_or_oem_calls() {
    let (mut controller, audit) = controller(vec![success(55.0, 2600.0)]);
    acquire(&mut controller);
    // A real full refresh already obtained its own thermal envelope; it must
    // not invoke the additional narrow provider as a hidden fallback.
    assert_eq!(count(&audit, Call::Thermal), 0);
    let full_refresh = controller.last_refresh;
    let full_sample = controller.thermal_sample.as_ref().unwrap();
    assert!(full_sample.started_at <= full_sample.finished_at);
    assert_eq!(
        full_refresh,
        Some(full_sample.finished_at),
        "full-device freshness must use the actual read completion, not catalog or UI receipt time"
    );
    assert_eq!(controller.refresh(), Err(SubmitError::Busy));

    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    let sample = controller.thermal_sample.as_ref().unwrap();
    assert_eq!(value(&sample.readings, "cpu-temperature"), Some(55.0));
    assert_eq!(value(&sample.readings, "cpu-fan"), Some(2600.0));
    assert!(sample.finished_at >= sample.started_at);
    let readings = &controller.snapshot.as_ref().unwrap().telemetry;
    assert_eq!(value(readings, "cpu-temperature"), Some(55.0));
    assert_eq!(value(readings, "cpu-fan"), Some(2600.0));
    assert_eq!(value(readings, "battery-power"), Some(12.5));
    assert_eq!(
        controller.last_refresh, full_refresh,
        "narrow sampling must not refresh the OEM/device clock"
    );
    assert!(controller.fan_session_held && controller.fan_applied.is_some());
    assert_eq!(count(&audit, Call::Snapshot), 1);
    assert_eq!(count(&audit, Call::Catalog), 1);
    assert_eq!(count(&audit, Call::Thermal), 1);
    assert_no_ordinary_control(&audit);
    close(&mut controller);
    assert!(!calls(&audit).contains(&Call::Drop { held: true }));
}

/// Deliberately does not implement Backend::sample_thermal. A default provider
/// must report unsupported without trying this backend's full snapshot.
struct UnsupportedBackend(SharedAudit);
impl Backend for UnsupportedBackend {
    fn snapshot(&mut self) -> Snapshot {
        record(&self.0, Call::Snapshot);
        fixture_snapshot()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        record(&self.0, Call::Catalog);
        vec![]
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        unreachable!("unsupported thermal request must not plan controls")
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        unreachable!("unsupported thermal request must not apply controls")
    }
}

#[test]
fn default_thermal_provider_is_an_error_without_snapshot_or_catalog_fallback() {
    let audit = Arc::new(Mutex::new(Audit::default()));
    let mut controller = Controller::new(UnsupportedBackend(audit.clone())).unwrap();
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert_eq!(
        controller.thermal_error.as_ref().unwrap().code,
        "thermal_unavailable"
    );
    assert!(controller.snapshot.is_none() && controller.thermal_sample.is_none());
    assert!(controller.worker_available);
    assert!(
        calls(&audit).is_empty(),
        "default provider touched full discovery"
    );
    close(&mut controller);
}

#[test]
fn thermal_read_error_retains_last_values_and_existing_fan_recovery_proof() {
    let (mut controller, audit) = controller(vec![success(51.0, 2300.0), SampleStep::Error]);
    acquire(&mut controller);
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    let previous = controller.thermal_sample.clone().unwrap();
    controller.refresh_fan().unwrap();
    finish(&mut controller);
    let fan_state = controller.fan_state.clone();
    assert_eq!(
        controller.fan_error.as_ref().unwrap().code,
        "fixture_restore_required"
    );
    controller.error = Some(ControlError::new(
        "earlier_write_failure",
        "Retain unrelated write error",
    ));

    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert_eq!(
        controller.thermal_error.as_ref().unwrap().code,
        "fixture_read_failed"
    );
    let retained = controller.thermal_sample.as_ref().unwrap();
    assert_eq!(retained.started_at, previous.started_at);
    assert_eq!(retained.finished_at, previous.finished_at);
    assert_eq!(value(&retained.readings, "cpu-temperature"), Some(51.0));
    assert_eq!(value(&retained.readings, "cpu-fan"), Some(2300.0));
    assert!(controller.thermal_last_attempt.unwrap() >= previous.finished_at);
    assert_eq!(controller.fan_state, fan_state);
    assert_eq!(
        controller.fan_error.as_ref().unwrap().code,
        "fixture_restore_required"
    );
    assert_eq!(
        controller.error.as_ref().unwrap().code,
        "earlier_write_failure"
    );
    assert!(controller.fan_session_held);
    assert!(!controller.fan_helper_reaped && !controller.fan_cleanup_complete);
    assert_eq!(controller.refresh(), Err(SubmitError::Busy));
    assert_eq!(count(&audit, Call::Snapshot), 1);
    assert_eq!(count(&audit, Call::Catalog), 1);
    assert_no_ordinary_control(&audit);
    close(&mut controller);
}

#[test]
fn thermal_panic_keeps_same_worker_and_known_lease_for_explicit_retry() {
    let (mut controller, audit) = controller(vec![SampleStep::Panic, success(58.0, 2900.0)]);
    acquire(&mut controller);
    let fan_state = controller.fan_state.clone();
    let previous = controller.thermal_sample.clone().unwrap();
    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert_eq!(
        controller.thermal_error.as_ref().unwrap().code,
        "worker_panicked"
    );
    assert_eq!(
        controller.thermal_sample.as_ref().unwrap().started_at,
        previous.started_at
    );
    assert_eq!(controller.fan_state, fan_state);
    assert!(controller.fan_error.is_none());
    assert!(controller.worker_available && controller.fan_session_held);
    assert!(!controller.fan_helper_reaped && !controller.fan_cleanup_complete);
    assert!(!calls(&audit)
        .iter()
        .any(|call| matches!(call, Call::Drop { .. })));

    controller.refresh_thermal().unwrap();
    finish(&mut controller);
    assert!(controller.thermal_error.is_none());
    assert_eq!(
        value(
            &controller.thermal_sample.as_ref().unwrap().readings,
            "cpu-fan"
        ),
        Some(2900.0)
    );
    let thermal_threads: Vec<_> = audit
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter_map(|(call, thread)| (*call == Call::Thermal).then_some(*thread))
        .collect();
    assert_eq!(thermal_threads.len(), 2);
    assert_eq!(
        thermal_threads[0], thermal_threads[1],
        "retry must use the retained worker/backend"
    );
    assert_eq!(controller.fan_state, fan_state);
    assert!(controller.fan_session_held && controller.fan_applied.is_some());
    assert_eq!(count(&audit, Call::Snapshot), 1);
    assert_eq!(count(&audit, Call::Catalog), 1);
    assert_no_ordinary_control(&audit);
    close(&mut controller);
}

#[test]
fn close_waits_for_bounded_thermal_read_then_prioritizes_shutdown_and_reaping() {
    let (blocked, gate) = gate();
    let (mut controller, audit) = controller(vec![SampleStep::Success {
        temperature: 53.0,
        rpm: 2500.0,
        gate: Some(blocked),
    }]);
    acquire(&mut controller);
    controller.refresh_thermal().unwrap();
    gate.wait();
    controller.request_close();
    controller.poll();
    assert!(
        !controller.close_ready(),
        "an in-flight read is not a cancellation receipt"
    );
    assert!(controller.fan_session_held && controller.thermal_sampling());
    assert_eq!(controller.refresh_thermal(), Err(SubmitError::Busy));
    assert_eq!(controller.refresh(), Err(SubmitError::Busy));
    assert_eq!(count(&audit, Call::Shutdown), 0);
    assert!(!calls(&audit)
        .iter()
        .any(|call| matches!(call, Call::Drop { .. })));

    gate.release();
    until(&mut controller, Controller::close_ready);
    assert_eq!(controller.refresh_thermal(), Err(SubmitError::Busy));
    let observed = calls(&audit);
    let thermal_index = observed
        .iter()
        .position(|call| *call == Call::Thermal)
        .unwrap();
    assert_eq!(
        &observed[thermal_index..],
        [Call::Thermal, Call::Shutdown, Call::Drop { held: false }]
    );
    assert_eq!(count(&audit, Call::Thermal), 1);
    assert_eq!(count(&audit, Call::Shutdown), 1);
    assert!(controller.fan_helper_reaped && controller.fan_cleanup_complete);
    assert!(controller.fan_restoration_confirmed && !controller.fan_session_held);
    assert_no_ordinary_control(&audit);
}
