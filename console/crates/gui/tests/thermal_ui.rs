#![cfg(feature = "gui")]
//! Production-renderer regressions for read-only thermal sampling during a fan
//! lease. All telemetry and control responses are explicitly local fixtures.
use eframe::egui::{self, pos2, vec2, Context, FullOutput, RawInput, Rect};
use egui_system_font_provider::SystemFontProvider;
use mechrevo_console_core::{
    fan_curve::{
        broker::{BrokerPhase, BrokerState, BACKEND, DEVICE_PATH},
        client::FanClientUpdate,
        FanCurve, FanCurveChannel, FanCurveLayout, RawFanTable, ValidatedFanCurve,
    },
    ApplyResult, BrightnessApplyResult, BrightnessPlan, CapabilityStatus, CatalogModel,
    ControlError, EvidenceSource, OemApplyResult, OemControl, OemControlKind, OemDomain, OemPlan,
    OemValue, PowerProfile, ProfilePlan, SensorReading, Snapshot, ThermalSample,
};
use mechrevo_console_gui::{
    desktop::{DesktopApp, RenderMetrics, Section},
    Backend, Controller, Preferences,
};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Calls {
    thermal: AtomicUsize,
    snapshots: AtomicUsize,
    catalogs: AtomicUsize,
    lifecycle: AtomicUsize,
    cached_polls: AtomicUsize,
    allow_cached_events: AtomicBool,
    other: AtomicUsize,
}
struct FixtureBackend {
    snapshot: Snapshot,
    responses: VecDeque<Result<ThermalSample, ControlError>>,
    calls: Arc<Calls>,
}
impl FixtureBackend {
    fn denied<T>(&self) -> Result<T, ControlError> {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn fan_reply(&self) -> FanClientUpdate {
        self.calls.lifecycle.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate {
            error: Some(ControlError::new(
                "fixture_fan_polled",
                "本地会话回包已处理",
            )),
            session_held: true,
            helper_reaped: false,
            cleanup_complete: false,
            ..Default::default()
        }
    }
}
impl Backend for FixtureBackend {
    fn snapshot(&mut self) -> Snapshot {
        self.calls.snapshots.fetch_add(1, Ordering::SeqCst);
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        self.snapshot.clone()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        self.calls.catalogs.fetch_add(1, Ordering::SeqCst);
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        vec![]
    }
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
        self.calls.thermal.fetch_add(1, Ordering::SeqCst);
        self.responses
            .pop_front()
            .unwrap_or_else(|| Err(ControlError::new("unexpected_sampling", "采样超出测试脚本")))
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        self.denied()
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        self.denied()
    }
    fn plan_brightness(&mut self, _: u8, _: &str) -> Result<BrightnessPlan, ControlError> {
        self.denied()
    }
    fn apply_brightness(
        &mut self,
        _: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        self.denied()
    }
    fn plan_oem(&mut self, _: &str, _: OemValue) -> Result<OemPlan, ControlError> {
        self.denied()
    }
    fn apply_oem(&mut self, _: &OemPlan) -> Result<OemApplyResult, ControlError> {
        self.denied()
    }
    fn fan_probe(&mut self) -> FanClientUpdate {
        self.fan_reply()
    }
    fn fan_refresh(&mut self) -> FanClientUpdate {
        self.fan_reply()
    }
    fn fan_apply(&mut self, _: &ValidatedFanCurve) -> FanClientUpdate {
        self.fan_reply()
    }
    fn fan_restore(&mut self) -> FanClientUpdate {
        self.fan_reply()
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        self.calls.lifecycle.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate::default()
    }
    fn fan_poll(&mut self) -> Option<FanClientUpdate> {
        self.calls.cached_polls.fetch_add(1, Ordering::SeqCst);
        if !self.calls.allow_cached_events.load(Ordering::Acquire) {
            return None;
        }
        Some(FanClientUpdate {
            error: Some(ControlError::new(
                "fixture_cached_poll",
                "仅缓存的辅助进程状态已处理",
            )),
            session_held: true,
            helper_reaped: false,
            cleanup_complete: false,
            ..Default::default()
        })
    }
}

fn reading(id: &str, label: &str, kind: &str, unit: &str, value: f64) -> SensorReading {
    serde_json::from_value(json!({
        "id":id, "label":label, "device_id":"fixture-thermal",
        "kind":kind, "value":value, "unit":unit, "status":"available", "reason":null,
        "source":{"backend":"fixture-only", "path":format!("fixture://thermal/{id}"), "device_id":"fixture-thermal"}
    })).unwrap()
}
fn sample(started_at: Instant, finished_at: Instant, cpu: f64) -> ThermalSample {
    ThermalSample {
        readings: vec![
            reading("cpu_temp", "CPU temperature", "temperature", "°C", cpu),
            reading("gpu_temp", "GPU temperature", "temperature", "°C", 49.0),
            reading("cpu_fan", "CPU Fan", "fan_speed", "RPM", 2600.0),
        ],
        diagnostics: vec![],
        started_at,
        finished_at,
    }
}
fn snapshot_fixture() -> Snapshot {
    serde_json::from_value(json!({
        "platform":"fixture-only / 非真实硬件",
        "identity":{
            "manufacturer":"TEST FIXTURE", "product_name":"采样回归测试设备",
            "product_version":null, "board_name":"NO-HARDWARE-BINDING", "bios_version":"TEST.0",
            "mechrevo_brand_detected":false, "support_note":"仅测试界面，不建立硬件资格。"
        },
        "telemetry":[reading("cpu_temp", "CPU temperature", "temperature", "°C", 41.0)],
        "brightness":[], "oem_controls":[], "capabilities":[], "diagnostics":[],
        "power_policy":{"scope":"fixture-only", "current":null, "options":[], "source":null}
    }))
    .unwrap()
}
fn fixture(responses: Vec<Result<ThermalSample, ControlError>>) -> (DesktopApp, Arc<Calls>) {
    fixture_on_platform(responses, "fixture-only / 非真实硬件")
}

fn fixture_on_platform(
    responses: Vec<Result<ThermalSample, ControlError>>,
    platform: &str,
) -> (DesktopApp, Arc<Calls>) {
    let mut snapshot = snapshot_fixture();
    snapshot.platform = platform.into();
    fixture_from_snapshots(responses, snapshot.clone(), snapshot)
}

fn fixture_from_snapshots(
    responses: Vec<Result<ThermalSample, ControlError>>,
    observed: Snapshot,
    next: Snapshot,
) -> (DesktopApp, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let mut controller = Controller::new(FixtureBackend {
        snapshot: next,
        responses: responses.into(),
        calls: calls.clone(),
    })
    .unwrap();
    controller.snapshot = Some(observed);
    controller.last_refresh = Some(Instant::now() - Duration::from_secs(60));
    let app = DesktopApp::with_controller(
        controller,
        Preferences {
            auto_refresh: false,
            ..Default::default()
        },
    );
    (app, calls)
}

/// A renderer fixture at the point after the controller accepted this sample.
/// Worker tests above still exercise the real acceptance/merge path.
fn install_sample(app: &mut DesktopApp, sample: ThermalSample) {
    app.controller.snapshot.as_mut().unwrap().telemetry = sample.readings.clone();
    app.controller.thermal_last_attempt = Some(sample.started_at);
    app.controller.thermal_last_finished = Some(sample.finished_at);
    app.controller.thermal_sample = Some(sample);
}

struct Harness {
    ctx: Context,
    physical: [u32; 2],
    dpi: f32,
    frame: u32,
}
impl Harness {
    fn new(logical: [u32; 2], dpi: f32) -> Self {
        let ctx = Context::default();
        ctx.add_font_provider(Arc::new(
            SystemFontProvider::new().with_locale(Some("zh-Hans")),
        ));
        ctx.enable_accesskit();
        ctx.set_missing_glyph_policy(egui::epaint::text::MissingGlyphPolicy::Panic);
        Self {
            ctx,
            physical: logical.map(|n| (n as f32 * dpi).round() as u32),
            dpi,
            frame: 0,
        }
    }
    fn events(
        &mut self,
        app: &mut DesktopApp,
        events: Vec<egui::Event>,
    ) -> (RenderMetrics, FullOutput) {
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(
                    self.physical[0] as f32 / (self.dpi * self.ctx.zoom_factor()),
                    self.physical[1] as f32 / (self.dpi * self.ctx.zoom_factor()),
                ),
            )),
            time: Some(self.frame as f64 * 0.1),
            events,
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(self.dpi);
        let mut metrics = None;
        let mut output = self.ctx.run_ui(input, |ui| metrics = Some(app.render(ui)));
        self.frame += 1;
        output.textures_delta.set.clear();
        output.textures_delta.free.clear();
        (metrics.unwrap(), output)
    }
    fn frame(&mut self, app: &mut DesktopApp) -> (RenderMetrics, FullOutput) {
        self.events(app, vec![])
    }
    fn settle(&mut self, app: &mut DesktopApp) -> (RenderMetrics, FullOutput) {
        for _ in 0..3 {
            self.frame(app);
        }
        self.frame(app)
    }
    fn wait(&mut self, app: &mut DesktopApp, done: impl Fn(&Controller) -> bool) -> FullOutput {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let output = self.frame(app).1;
            if done(&app.controller) {
                return output;
            }
            assert!(
                Instant::now() < deadline,
                "fixture worker did not finish within two seconds"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn key(&mut self, app: &mut DesktopApp, key: egui::Key) -> FullOutput {
        for pressed in [true, false] {
            self.events(
                app,
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        self.frame(app).1
    }
}

fn has_text(output: &FullOutput, expected: &str) -> bool {
    fn visit(shape: &egui::Shape, expected: &str) -> bool {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| visit(shape, expected)),
            egui::Shape::Text(text) => text.galley.job.text.contains(expected),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|shape| visit(&shape.shape, expected))
}
fn assert_fits(metrics: &RenderMetrics) {
    assert!(metrics.content_size.is_finite());
    assert!(
        metrics.content_size.x <= metrics.viewport.width() + 1.0,
        "thermal status must wrap inside the viewport"
    );
    assert!(metrics.scroll_offset.x.abs() < 0.001);
}

#[test]
fn automatic_sampling_uses_only_the_dedicated_thermal_job_during_a_lease_or_unclean_session() {
    for held in [true, false] {
        let now = Instant::now();
        let expected = sample(
            now - Duration::from_millis(30),
            now - Duration::from_millis(10),
            64.0,
        );
        let (mut app, calls) = fixture(vec![Ok(expected.clone())]);
        app.preferences.auto_refresh = true;
        app.controller.fan_session_held = held;
        app.controller.fan_helper_reaped = !held;
        app.controller.fan_cleanup_complete = false;
        app.controller.thermal_last_attempt = Some(now - Duration::from_secs(4));
        let mut harness = Harness::new([640, 480], 1.5);
        let output = harness.wait(&mut app, |controller| {
            controller.thermal_sample.is_some() && !controller.busy()
        });
        assert_eq!(calls.thermal.load(Ordering::SeqCst), 1);
        assert_eq!(
            calls.other.load(Ordering::SeqCst),
            0,
            "a thermal refresh must not enumerate OEM, battery, backlight or catalog state"
        );
        assert_eq!(calls.lifecycle.load(Ordering::SeqCst), 0);
        assert_eq!(
            app.controller.thermal_sample.as_ref().unwrap().started_at,
            expected.started_at
        );
        assert!(
            has_text(&output, "64"),
            "the card must display the actual new sample"
        );
        for _ in 0..4 {
            assert_fits(&harness.frame(&mut app).0);
        }
        assert_eq!(
            calls.thermal.load(Ordering::SeqCst),
            1,
            "sampling must retain its bounded cadence"
        );
    }
}

#[test]
fn clean_linux_without_a_preview_refreshes_battery_and_ac_through_the_full_snapshot() {
    let now = Instant::now();
    let mut observed = snapshot_fixture();
    observed.platform = "linux".into();
    observed.telemetry.extend([
        reading("battery", "Battery", "battery_capacity", "%", 20.0),
        reading("ac", "AC", "external_power_online", "boolean", 0.0),
    ]);
    let mut next = observed.clone();
    next.telemetry
        .iter_mut()
        .for_each(|reading| match reading.id.as_str() {
            "battery" => reading.value = Some(63.0),
            "ac" => reading.value = Some(1.0),
            _ => {}
        });
    let (mut app, calls) = fixture_from_snapshots(vec![], observed, next);
    let old_refresh = app.controller.last_refresh.unwrap();
    app.preferences.auto_refresh = true;
    app.controller.thermal_last_attempt = Some(now - Duration::from_secs(30));
    app.controller.thermal_last_finished = Some(now - Duration::from_secs(30));
    assert!(!app.controller.fan_session_held);
    assert!(app.controller.fan_helper_reaped && app.controller.fan_cleanup_complete);
    let mut harness = Harness::new([1120, 760], 1.0);
    harness.wait(&mut app, |controller| {
        controller
            .last_refresh
            .is_some_and(|time| time > old_refresh)
            && !controller.busy()
    });
    let telemetry = &app.controller.snapshot.as_ref().unwrap().telemetry;
    for (id, value) in [("battery", 63.0), ("ac", 1.0)] {
        assert_eq!(
            telemetry
                .iter()
                .find(|reading| reading.id == id)
                .unwrap()
                .value,
            Some(value)
        );
    }
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 0);
    assert_eq!(calls.snapshots.load(Ordering::SeqCst), 1);
    assert_eq!(calls.catalogs.load(Ordering::SeqCst), 1);
    assert_eq!(calls.other.load(Ordering::SeqCst), 2);
    assert_eq!(calls.lifecycle.load(Ordering::SeqCst), 0);
}

#[test]
fn clean_windows_keeps_full_snapshot_and_catalog_refresh_without_calling_the_narrow_sampler() {
    let (mut app, calls) = fixture_on_platform(vec![], "windows");
    let old_refresh = app.controller.last_refresh.unwrap();
    app.preferences.auto_refresh = true;
    let old = Instant::now() - Duration::from_secs(30);
    app.controller.thermal_last_attempt = Some(old);
    app.controller.thermal_last_finished = Some(old);
    assert!(!app.controller.fan_session_held);
    assert!(app.controller.fan_helper_reaped && app.controller.fan_cleanup_complete);
    let mut harness = Harness::new([1120, 760], 1.0);
    let output = harness.wait(&mut app, |controller| {
        controller
            .last_refresh
            .is_some_and(|time| time > old_refresh)
            && !controller.busy()
    });
    assert!(has_text(&output, "41"));
    assert_eq!(
        app.controller.snapshot.as_ref().unwrap().platform,
        "windows"
    );
    assert_eq!(calls.snapshots.load(Ordering::SeqCst), 1);
    assert_eq!(calls.catalogs.load(Ordering::SeqCst), 1);
    assert_eq!(calls.other.load(Ordering::SeqCst), 2);
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 0);
    assert_eq!(calls.lifecycle.load(Ordering::SeqCst), 0);
}

fn prepare_fan_preview(app: &mut DesktopApp) -> ValidatedFanCurve {
    // Protocol-valid fixture input only; no model default or hardware write.
    let curve = FanCurve {
        schema_version: 1,
        layout: FanCurveLayout::UniwillRamFan1p5,
        respective: true,
        cpu: FanCurveChannel {
            rise_celsius: [45; 15],
            fall_celsius: [40; 15],
            duty_raw: [100; 16],
        },
        gpu: FanCurveChannel {
            rise_celsius: [48; 15],
            fall_celsius: [42; 15],
            duty_raw: [120; 16],
        },
    }
    .validate()
    .unwrap();
    app.controller.set_fan_draft(curve.curve().clone()).unwrap();
    app.controller.preview_fan().unwrap();
    assert_eq!(app.controller.fan_preview.as_ref(), Some(&curve));
    curve
}

fn install_active_session(app: &mut DesktopApp, curve: &ValidatedFanCurve) {
    let original = RawFanTable::from_bytes([129; 96]);
    app.controller.fan_state = Some(BrokerState {
        backend: BACKEND.into(),
        device_path: DEVICE_PATH.into(),
        epoch: 41,
        generation: 3,
        lease_id: 17,
        last_request_id: 2,
        phase: BrokerPhase::Active,
        capabilities: 7,
        owner: true,
        revoked: false,
        modified_or_uncertain: true,
        state_is_current: true,
        restored_verified: false,
        live_error: 0,
        last_error: 0,
        restore_error: 0,
        live_table: Some(original.with_curve(curve).as_bytes().to_vec()),
        original_table: Some(original.as_bytes().to_vec()),
        live_controls: Some([0x80, 0x04, 0, 0, 0, 0]),
        original_controls: Some([0; 6]),
        cached_original_targets: Some([0; 4]),
        applied_curve: Some(curve.curve().clone()),
    });
    app.controller.fan_session_held = true;
    app.controller.fan_helper_reaped = false;
    app.controller.fan_cleanup_complete = false;
    app.controller.fan_applied = Some(curve.clone());
}

fn assert_latest_thermal_values(app: &DesktopApp, cpu: f64, rpm: f64) {
    let readings = &app.controller.snapshot.as_ref().unwrap().telemetry;
    for (id, value) in [("cpu_temp", cpu), ("cpu_fan", rpm)] {
        assert_eq!(
            readings
                .iter()
                .find(|reading| reading.id == id)
                .unwrap()
                .value,
            Some(value)
        );
    }
}

#[test]
fn active_fan_preview_keeps_automatic_thermal_sampling_and_preserves_confirmation() {
    let now = Instant::now();
    let mut expected = sample(
        now - Duration::from_millis(30),
        now - Duration::from_millis(10),
        77.0,
    );
    expected
        .readings
        .iter_mut()
        .find(|reading| reading.id == "cpu_fan")
        .unwrap()
        .value = Some(3200.0);
    let (mut app, calls) = fixture_on_platform(vec![Ok(expected)], "linux");
    app.controller
        .snapshot
        .as_mut()
        .unwrap()
        .telemetry
        .push(reading("cpu_fan", "CPU Fan", "fan_speed", "RPM", 1200.0));
    let preview = prepare_fan_preview(&mut app);
    install_active_session(&mut app, &preview);
    app.preferences.auto_refresh = true;
    app.controller.thermal_last_attempt = Some(now - Duration::from_secs(30));
    app.controller.thermal_last_finished = Some(now - Duration::from_secs(30));
    let mut harness = Harness::new([1120, 760], 1.0);
    let output = harness.wait(&mut app, |controller| {
        controller.thermal_sample.is_some() && !controller.busy()
    });
    assert_latest_thermal_values(&app, 77.0, 3200.0);
    assert!(has_text(&output, "77") && has_text(&output, "3200"));
    assert!(!has_text(&output, "自动采样已暂停"));
    assert_eq!(app.controller.fan_preview.as_ref(), Some(&preview));
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 1);
    assert_eq!(calls.snapshots.load(Ordering::SeqCst), 0);
    assert_eq!(calls.catalogs.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    assert_eq!(calls.lifecycle.load(Ordering::SeqCst), 0);
}

#[test]
fn a_clean_pending_preview_samples_narrowly_on_linux_and_pauses_both_paths_on_windows() {
    for platform in ["linux", "windows"] {
        let now = Instant::now();
        let expected = sample(
            now - Duration::from_millis(30),
            now - Duration::from_millis(10),
            76.0,
        );
        let (mut app, calls) = fixture_on_platform(vec![Ok(expected)], platform);
        let preview = prepare_fan_preview(&mut app);
        app.preferences.auto_refresh = true;
        app.controller.thermal_last_attempt = Some(now - Duration::from_secs(30));
        app.controller.thermal_last_finished = Some(now - Duration::from_secs(30));
        assert!(!app.controller.fan_session_held);
        assert!(app.controller.fan_helper_reaped && app.controller.fan_cleanup_complete);
        let mut harness = Harness::new([1120, 760], 1.0);
        let output = if platform == "linux" {
            harness.wait(&mut app, |controller| {
                controller.thermal_sample.is_some() && !controller.busy()
            })
        } else {
            harness.settle(&mut app).1
        };
        if platform == "linux" {
            assert_latest_thermal_values(&app, 76.0, 2600.0);
            assert!(!has_text(&output, "自动采样已暂停"));
            assert_eq!(calls.thermal.load(Ordering::SeqCst), 1);
        } else {
            assert!(has_text(&output, "自动采样已暂停"));
            assert!(app.controller.thermal_sample.is_none());
            assert_eq!(calls.thermal.load(Ordering::SeqCst), 0);
        }
        assert_eq!(app.controller.fan_preview.as_ref(), Some(&preview));
        assert_eq!(calls.snapshots.load(Ordering::SeqCst), 0);
        assert_eq!(calls.catalogs.load(Ordering::SeqCst), 0);
        assert_eq!(calls.other.load(Ordering::SeqCst), 0);
        assert_eq!(calls.lifecycle.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn manual_sampling_disables_hidden_reads_but_does_not_stop_fan_response_polling() {
    let (mut app, calls) = fixture(vec![]);
    app.controller.fan_session_held = true;
    app.controller.fan_helper_reaped = false;
    app.controller.fan_cleanup_complete = false;
    app.controller.thermal_last_attempt = Some(Instant::now() - Duration::from_secs(30));
    app.controller.refresh_fan().unwrap();
    let mut harness = Harness::new([640, 480], 2.0);
    harness.wait(&mut app, |controller| {
        controller
            .fan_error
            .as_ref()
            .is_some_and(|error| error.code == "fixture_fan_polled")
            && !controller.busy()
    });
    calls.allow_cached_events.store(true, Ordering::Release);
    harness.wait(&mut app, |controller| {
        controller
            .fan_error
            .as_ref()
            .is_some_and(|error| error.code == "fixture_cached_poll")
            && !controller.busy()
    });
    for _ in 0..8 {
        assert_fits(&harness.frame(&mut app).0);
    }
    assert_eq!(calls.lifecycle.load(Ordering::SeqCst), 1);
    assert!(
        calls.cached_polls.load(Ordering::SeqCst) >= 1,
        "manual telemetry mode must still consume cached fan lifecycle updates"
    );
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

/// Deliberately omits sample_thermal to exercise the production trait default.
struct UnsupportedThermalBackend(Arc<AtomicUsize>);
impl Backend for UnsupportedThermalBackend {
    fn snapshot(&mut self) -> Snapshot {
        self.0.fetch_add(1, Ordering::SeqCst);
        snapshot_fixture()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        self.0.fetch_add(1, Ordering::SeqCst);
        vec![]
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
}

#[test]
fn a_backend_without_dedicated_sampling_never_falls_back_to_a_full_snapshot() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut controller = Controller::new(UnsupportedThermalBackend(calls.clone())).unwrap();
    controller.snapshot = Some(snapshot_fixture());
    let mut app = DesktopApp::with_controller(
        controller,
        Preferences {
            auto_refresh: false,
            ..Default::default()
        },
    );
    app.controller.refresh_thermal().unwrap();
    let mut harness = Harness::new([1120, 760], 1.0);
    harness.wait(&mut app, |controller| {
        controller.thermal_error.is_some() && !controller.busy()
    });
    assert!(app.controller.thermal_sample.is_none());
    assert_ne!(
        app.controller.thermal_error.as_ref().unwrap().code,
        "worker_panicked"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the trait default must report unavailable without invoking full snapshot or catalog"
    );
}

#[test]
fn freshness_uses_the_collection_start_and_discloses_a_slow_collection() {
    let now = Instant::now();
    let (mut app, calls) = fixture(vec![]);
    app.controller.last_refresh = Some(now);
    install_sample(
        &mut app,
        sample(
            now - Duration::from_secs(20),
            now - Duration::from_secs(1),
            89.0,
        ),
    );
    let mut harness = Harness::new([640, 480], 2.0);
    let (metrics, output) = harness.settle(&mut app);
    assert_fits(&metrics);
    assert!(
        has_text(&output, "读数已陈旧"),
        "finishing one second ago must not make a twenty-second-old observation fresh"
    );
    assert!(has_text(&output, "采集耗时"));
    assert!(has_text(&output, "手动采样"));
    assert!(
        has_text(&output, "89"),
        "the stale status must describe the displayed thermal sample"
    );
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

#[test]
fn a_failed_sample_is_visible_pauses_automatic_reads_and_can_be_explicitly_retried() {
    let now = Instant::now();
    let fresh = sample(
        now - Duration::from_millis(20),
        now - Duration::from_millis(5),
        91.0,
    );
    let (mut app, calls) = fixture(vec![
        Err(ControlError::new(
            "fixture_thermal_failed",
            "测试温度采样失败，保留旧读数",
        )),
        Ok(fresh.clone()),
    ]);
    let previous = sample(
        now - Duration::from_secs(8),
        now - Duration::from_secs(7),
        83.0,
    );
    install_sample(&mut app, previous.clone());
    app.preferences.auto_refresh = true;
    app.controller.fan_session_held = true;
    app.controller.fan_helper_reaped = false;
    app.controller.fan_cleanup_complete = false;
    app.controller.refresh_thermal().unwrap();
    let mut harness = Harness::new([1120, 760], 1.0);
    let output = harness.wait(&mut app, |controller| {
        controller.thermal_error.is_some() && !controller.busy()
    });
    assert!(has_text(&output, "采样失败"));
    assert!(has_text(&output, "自动采样已暂停"));
    assert!(has_text(&output, "83"));
    assert_eq!(
        app.controller.thermal_sample.as_ref().unwrap().started_at,
        previous.started_at
    );
    // Force an already-due interval: a failure pause cannot be only the normal
    // three-second cadence delay dressed up as an explicit paused state.
    app.controller.thermal_last_attempt = Some(now - Duration::from_secs(30));
    app.controller.thermal_last_finished = Some(now - Duration::from_secs(30));
    for _ in 0..4 {
        harness.frame(&mut app);
    }
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 1);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    app.controller.refresh_thermal().unwrap();
    let output = harness.wait(&mut app, |controller| {
        controller.thermal_error.is_none() && !controller.busy()
    });
    assert!(has_text(&output, "91"));
    assert!(!has_text(&output, "采样失败"));
    assert_eq!(
        app.controller.thermal_sample.as_ref().unwrap().started_at,
        fresh.started_at
    );
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 2);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    assert_eq!(calls.lifecycle.load(Ordering::SeqCst), 0);
}

fn history_segments(output: &FullOutput, card: Rect) -> usize {
    fn count(shape: &egui::Shape, card: Rect) -> usize {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().map(|shape| count(shape, card)).sum(),
            egui::Shape::LineSegment { points, stroke }
                if (stroke.width - 2.0).abs() < 0.01
                    && points.iter().all(|point| card.contains(*point)) =>
            {
                1
            }
            _ => 0,
        }
    }
    output
        .shapes
        .iter()
        .map(|shape| count(&shape.shape, card))
        .sum()
}

#[test]
fn a_long_sample_gap_is_disclosed_and_never_painted_as_a_continuous_trend() {
    let (mut app, calls) = fixture(vec![]);
    let mut harness = Harness::new([1120, 760], 1.0);
    let start = Instant::now() - Duration::from_secs(25);
    for (offset, cpu) in [(0, 61.0), (3, 64.0), (6, 68.0)] {
        let time = start + Duration::from_secs(offset);
        install_sample(
            &mut app,
            sample(time, time + Duration::from_millis(10), cpu),
        );
        harness.settle(&mut app);
    }
    let (metrics, output) = harness.settle(&mut app);
    assert!(!has_text(&output, "存在采样间隔，空窗不连线"));
    let continuous = history_segments(&output, metrics.thermal_panel_rect);
    assert!(
        continuous >= 4,
        "both actual sensor histories must be drawn before checking a gap"
    );
    let time = start + Duration::from_secs(20);
    install_sample(
        &mut app,
        sample(time, time + Duration::from_millis(10), 73.0),
    );
    let (metrics, output) = harness.settle(&mut app);
    assert!(has_text(&output, "存在采样间隔，空窗不连线"));
    assert_eq!(
        history_segments(&output, metrics.thermal_panel_rect),
        continuous,
        "neither CPU nor GPU may connect across the fourteen-second gap"
    );
    let time = start + Duration::from_secs(23);
    install_sample(
        &mut app,
        sample(time, time + Duration::from_millis(10), 71.0),
    );
    let (metrics, output) = harness.settle(&mut app);
    assert_eq!(
        history_segments(&output, metrics.thermal_panel_rect),
        continuous + 2,
        "the next consecutive sample must resume both histories"
    );
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

fn named_control(
    output: &FullOutput,
    name: &str,
    role: egui::accesskit::Role,
) -> (egui::accesskit::NodeId, Rect) {
    let nodes = &output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes;
    let (id, node) = nodes
        .iter()
        .find(|(_, node)| {
            node.role() == role
                && (node.label() == Some(name)
                    || node.labelled_by().iter().any(|label| {
                        nodes.iter().any(|(id, node)| {
                            id == label
                                && (node.value() == Some(name) || node.label() == Some(name))
                        })
                    }))
        })
        .unwrap_or_else(|| panic!("accessible {role:?} '{name}' is missing"));
    let bounds = node.bounds().unwrap();
    (
        *id,
        Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        ),
    )
}
fn reveal(harness: &mut Harness, app: &mut DesktopApp, name: &str) -> FullOutput {
    let (mut metrics, mut output) = harness.settle(app);
    for _ in 0..12 {
        let (_, bounds) = named_control(&output, name, egui::accesskit::Role::SpinButton);
        if metrics.viewport.expand(1.0).contains_rect(bounds) {
            return output;
        }
        harness.events(
            app,
            vec![
                egui::Event::PointerMoved(metrics.viewport.center()),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(
                        0.0,
                        (metrics.viewport.center().y - bounds.center().y).clamp(-200.0, 200.0),
                    ),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        (metrics, output) = harness.settle(app);
    }
    panic!("RGB control '{name}' remained clipped after bounded scrolling");
}

#[test]
fn keyboard_rgb_increments_each_selected_byte_by_exactly_one_without_a_preview() {
    let (mut app, calls) = fixture(vec![]);
    let initial = [12, 80, 230];
    app.controller.snapshot.as_mut().unwrap().oem_controls = vec![OemControl {
        id: "keyboard_color".into(),
        kind: OemControlKind::KeyboardColor,
        label: "键盘颜色".into(),
        current: Some(OemValue::Rgb(initial)),
        domain: OemDomain::Rgb,
        status: CapabilityStatus::Available,
        writable: true,
        source: EvidenceSource {
            backend: "fixture-only".into(),
            path: "fixture://rgb".into(),
            device_id: Some("fixture-keyboard".into()),
        },
        binding: "fixture-only:keyboard-color".into(),
        readback_scope: "仅本地请求值，不测量灯光。".into(),
        reason: None,
    }];
    app.controller.select_oem_control("keyboard_color");
    app.controller.set_oem_target(OemValue::Rgb(initial));
    app.select_section(Section::Controls);
    let mut harness = Harness::new([640, 480], 2.0);
    let mut expected = initial;
    for (index, name) in ["红", "绿", "蓝"].into_iter().enumerate() {
        let output = reveal(&mut harness, &mut app, name);
        let (target, _) = named_control(&output, name, egui::accesskit::Role::SpinButton);
        let mut focused = false;
        for _ in 0..96 {
            let output = harness.key(&mut app, egui::Key::Tab);
            if output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .focus
                == target
            {
                focused = true;
                break;
            }
        }
        assert!(focused, "keyboard focus did not reach RGB channel '{name}'");
        harness.key(&mut app, egui::Key::ArrowUp);
        harness.key(&mut app, egui::Key::Tab);
        expected[index] += 1;
        assert_eq!(
            app.controller.requested_oem_value,
            Some(OemValue::Rgb(expected)),
            "one keypress must change only one byte by one"
        );
        assert!(app.controller.oem_preview.is_none());
        assert!(!app.controller.busy());
    }
    assert_eq!(calls.thermal.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    assert_eq!(calls.lifecycle.load(Ordering::SeqCst), 0);
}
