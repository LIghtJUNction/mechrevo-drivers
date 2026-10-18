#![cfg(feature = "gui")]
//! Actual-renderer brightness regressions with typed local backends only.
//! A delegated session route never implies direct sysfs write permission.
use eframe::egui::{self, pos2, vec2, Context, FullOutput, RawInput, Rect};
use egui_system_font_provider::SystemFontProvider;
use mechrevo_console_core::{
    brightness::{
        BrightnessBackend, BrightnessOutcome, BrightnessRecovery, BrightnessTransaction,
        LogindSessionBinding, BRIGHTNESS_TRANSACTION_MARKER,
    },
    cpu_power::{CpuPowerUpdate, Outcome as CpuOutcome, Plan as CpuPlan},
    fan_curve::{client::FanClientUpdate, ValidatedFanCurve},
    ApplyResult, BrightnessApplyResult, BrightnessDevice, BrightnessPlan, CapabilityStatus,
    CatalogModel, ControlError, EvidenceSource, OemApplyResult, OemPlan, OemValue, PowerProfile,
    ProfilePlan, Snapshot, ThermalSample,
};
use mechrevo_console_gui::{
    desktop::{DesktopApp, RenderMetrics},
    Backend, Controller, Preferences,
};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Calls {
    preview: AtomicUsize,
    apply: AtomicUsize,
    snapshot: AtomicUsize,
    catalog: AtomicUsize,
    other: AtomicUsize,
    preview_requests: Mutex<Vec<(u8, String)>>,
}
struct FixtureBackend {
    snapshot: Snapshot,
    after_apply: Option<Snapshot>,
    previews: VecDeque<BrightnessPlan>,
    apply_result: Option<Result<BrightnessApplyResult, ControlError>>,
    calls: Arc<Calls>,
}
impl FixtureBackend {
    fn denied<T>(&self) -> Result<T, ControlError> {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn no_fan(&self) -> FanClientUpdate {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate {
            error: Some(ControlError::new("fixture_only", "No hardware backend")),
            ..Default::default()
        }
    }
    fn no_cpu(&self) -> CpuPowerUpdate {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        CpuPowerUpdate {
            status: None,
            outcome: CpuOutcome::RejectedBeforeWrite,
            error: Some(ControlError::new("fixture_only", "No hardware backend")),
            receipt: None,
            helper_reaped: true,
        }
    }
}
impl Backend for FixtureBackend {
    fn snapshot(&mut self) -> Snapshot {
        self.calls.snapshot.fetch_add(1, Ordering::SeqCst);
        self.snapshot.clone()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        self.calls.catalog.fetch_add(1, Ordering::SeqCst);
        vec![]
    }
    fn plan_brightness(
        &mut self,
        percent: u8,
        device: &str,
    ) -> Result<BrightnessPlan, ControlError> {
        self.calls.preview.fetch_add(1, Ordering::SeqCst);
        self.calls
            .preview_requests
            .lock()
            .unwrap()
            .push((percent, device.into()));
        Ok(self
            .previews
            .pop_front()
            .expect("unexpected brightness preview"))
    }
    fn apply_brightness(
        &mut self,
        _: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        self.calls.apply.fetch_add(1, Ordering::SeqCst);
        if let Some(snapshot) = self.after_apply.take() {
            self.snapshot = snapshot;
        }
        self.apply_result
            .take()
            .expect("unexpected brightness apply")
    }
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
        self.denied()
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        self.denied()
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        self.denied()
    }
    fn plan_oem(&mut self, _: &str, _: OemValue) -> Result<OemPlan, ControlError> {
        self.denied()
    }
    fn apply_oem(&mut self, _: &OemPlan) -> Result<OemApplyResult, ControlError> {
        self.denied()
    }
    fn cpu_power_probe(&mut self) -> CpuPowerUpdate {
        self.no_cpu()
    }
    fn cpu_power_execute(&mut self, _: &CpuPlan) -> CpuPowerUpdate {
        self.no_cpu()
    }
    fn fan_probe(&mut self) -> FanClientUpdate {
        self.no_fan()
    }
    fn fan_refresh(&mut self) -> FanClientUpdate {
        self.no_fan()
    }
    fn fan_apply(&mut self, _: &ValidatedFanCurve) -> FanClientUpdate {
        self.no_fan()
    }
    fn fan_restore(&mut self) -> FanClientUpdate {
        self.no_fan()
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        self.no_fan()
    }
    fn fan_poll(&mut self) -> Option<FanClientUpdate> {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        None
    }
}

fn device() -> BrightnessDevice {
    BrightnessDevice {
        id: "fixture-display".into(),
        label: "本地背光测试设备".into(),
        current_percent: None,
        control_value: Some(400),
        actual_value: None,
        maximum: Some(1000),
        source: EvidenceSource {
            backend: "fixture-only".into(),
            path: "fixture://backlight/brightness".into(),
            device_id: Some("fixture-display".into()),
        },
        status: CapabilityStatus::Available,
        readable: true,
        writable: false,
        write_backend: Some(BrightnessBackend::LinuxLogind),
        logind_session: Some(LogindSessionBinding {
            session_id: "fixture-session".into(),
            object_path: "/org/freedesktop/login1/session/fixture".into(),
            uid: 1000,
            seat_id: "seat0".into(),
            seat_path: "/org/freedesktop/login1/seat/seat0".into(),
        }),
        supported_percentages: vec![],
        reason: None,
    }
}
fn plan() -> BrightnessPlan {
    let device = device();
    BrightnessPlan {
        device_id: device.id,
        requested_percent: 51,
        target: 510,
        previous: 400,
        maximum: 1000,
        source: device.source,
        backend: BrightnessBackend::LinuxLogind,
        logind_session: device.logind_session,
    }
}
fn snapshot(device: BrightnessDevice) -> Snapshot {
    serde_json::from_value(json!({
        "platform":"fixture-only / 非真实硬件",
        "identity":{"manufacturer":"TEST FIXTURE","product_name":"亮度界面测试设备","product_version":null,
            "board_name":"NO-HARDWARE-BINDING","bios_version":"TEST.0","mechrevo_brand_detected":false,"support_note":"仅本地UI测试，不访问桌面会话或设备。"},
        "telemetry":[],"brightness":[device],"oem_controls":[],"capabilities":[],"diagnostics":[],
        "power_policy":{"scope":"fixture-only","current":null,"options":[],"source":null}
    })).unwrap()
}
fn fixture(
    device: BrightnessDevice,
    apply_result: Option<Result<BrightnessApplyResult, ControlError>>,
    after_apply: Option<Snapshot>,
) -> (DesktopApp, Arc<Calls>) {
    let snapshot = snapshot(device);
    let calls = Arc::new(Calls::default());
    let mut controller = Controller::new(FixtureBackend {
        snapshot: snapshot.clone(),
        after_apply,
        previews: vec![plan(), plan()].into(),
        apply_result,
        calls: calls.clone(),
    })
    .unwrap();
    controller.snapshot = Some(snapshot);
    controller.select_brightness_device("fixture-display");
    let app = DesktopApp::with_controller(
        controller,
        Preferences {
            auto_refresh: false,
            ..Default::default()
        },
    );
    (app, calls)
}
fn transaction(outcome: BrightnessOutcome, writer_may_continue: bool) -> BrightnessTransaction {
    let confirmed = outcome == BrightnessOutcome::Confirmed;
    BrightnessTransaction {
        outcome,
        write_attempted: true,
        writer_may_continue,
        observed_control: confirmed.then_some(510),
        primary_error: (!confirmed)
            .then(|| ControlError::new("fixture_write_unknown", "调节请求结果未确认。")),
        rollback_error: (outcome == BrightnessOutcome::RecoveryFailed)
            .then(|| ControlError::new("fixture_recovery_failed", "保留独立的恢复失败记录。")),
        readback_error: (!confirmed)
            .then(|| ControlError::new("fixture_readback_failed", "保留独立的回读错误。")),
        recovery: if confirmed {
            BrightnessRecovery::NotNeeded
        } else if outcome == BrightnessOutcome::RecoveryFailed {
            BrightnessRecovery::Failed
        } else {
            BrightnessRecovery::Skipped
        },
        recovery_reason: (!confirmed).then(|| "本地未知结果测试，不以重复提交模拟恢复。".into()),
    }
}
fn failure(transaction: &BrightnessTransaction) -> ControlError {
    let mut error = ControlError::new(
        "fixture_brightness_unknown",
        "调节结果未知，原始事务证据已保留。",
    );
    error.details = vec![format!(
        "{BRIGHTNESS_TRANSACTION_MARKER}{}",
        serde_json::to_string(transaction).unwrap()
    )];
    error
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
    fn click(&mut self, app: &mut DesktopApp, at: egui::Pos2) -> FullOutput {
        for pressed in [true, false] {
            self.events(
                app,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        self.frame(app).1
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
    fn focus(&mut self, app: &mut DesktopApp, id: egui::accesskit::NodeId) {
        for _ in 0..96 {
            let output = self.key(app, egui::Key::Tab);
            if output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .focus
                == id
            {
                return;
            }
        }
        panic!("keyboard did not reach brightness slider");
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
                "brightness fake worker did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
fn node_bounds(node: &egui::accesskit::Node) -> Rect {
    let bounds = node.bounds().unwrap();
    Rect::from_min_max(
        pos2(bounds.x0 as f32, bounds.y0 as f32),
        pos2(bounds.x1 as f32, bounds.y1 as f32),
    )
}
fn control(
    output: &FullOutput,
    label: &str,
    role: egui::accesskit::Role,
) -> (egui::accesskit::NodeId, Rect) {
    let (id, node) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| {
            node.role() == role && (node.label() == Some(label) || node.value() == Some(label))
        })
        .unwrap_or_else(|| panic!("{role:?} '{label}' is missing"));
    (*id, node_bounds(node))
}
fn reveal(
    harness: &mut Harness,
    app: &mut DesktopApp,
    find: impl Fn(&FullOutput) -> (egui::accesskit::NodeId, Rect),
) -> FullOutput {
    let (mut metrics, mut output) = harness.settle(app);
    for _ in 0..16 {
        let (_, bounds) = find(&output);
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
    panic!("brightness control remained clipped after bounded scrolling");
}
fn button(harness: &mut Harness, app: &mut DesktopApp, label: &str) -> FullOutput {
    let output = reveal(harness, app, |output| {
        control(output, label, egui::accesskit::Role::Button)
    });
    let (_, bounds) = control(&output, label, egui::accesskit::Role::Button);
    harness.click(app, bounds.center())
}
fn slider(output: &FullOutput) -> (egui::accesskit::NodeId, Rect) {
    let (id, node) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.role() == egui::accesskit::Role::Slider)
        .expect("delegated brightness route must expose a real slider");
    assert!(!node.is_disabled());
    (*id, node_bounds(node))
}
fn text_contains(output: &FullOutput, expected: &str) -> bool {
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
fn label_containing(output: &FullOutput, part: &str) -> (egui::accesskit::NodeId, Rect) {
    let (id, node) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| {
            node.role() == egui::accesskit::Role::Label
                && node
                    .value()
                    .or(node.label())
                    .is_some_and(|text| text.contains(part))
        })
        .unwrap_or_else(|| panic!("brightness label containing '{part}' is missing"));
    (*id, node_bounds(node))
}
fn assert_unavailable_reason(
    harness: &mut Harness,
    app: &mut DesktopApp,
    summary: &str,
    reason: &str,
) {
    let output = reveal(harness, app, |output| {
        control(output, summary, egui::accesskit::Role::Label)
    });
    assert!(text_contains(&output, summary));
    let find_details = |output: &FullOutput| {
        let (_, summary_bounds) = control(output, summary, egui::accesskit::Role::Label);
        let (id, node) = output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .filter(|(_, node)| {
                if node.role() != egui::accesskit::Role::DisclosureTriangle
                    || node.label().or(node.value()) != Some("查看原因")
                {
                    return false;
                }
                let bounds = node_bounds(node);
                bounds.top() >= summary_bounds.bottom() - 1.0
                    && (bounds.left() - summary_bounds.left()).abs() < 16.0
            })
            .min_by(|(_, left), (_, right)| {
                node_bounds(left).top().total_cmp(&node_bounds(right).top())
            })
            .expect("the brightness summary must have its own reason disclosure");
        (*id, node_bounds(node))
    };
    let output = reveal(harness, app, find_details);
    let (id, bounds) = find_details(&output);
    let output = harness.click(app, bounds.center());
    let (_, node) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(candidate, _)| *candidate == id)
        .unwrap();
    assert_eq!(node.is_expanded(), Some(true));
    let output = reveal(harness, app, |output| {
        control(output, reason, egui::accesskit::Role::Label)
    });
    assert!(text_contains(&output, reason));
}
fn prepare_preview(harness: &mut Harness, app: &mut DesktopApp, calls: &Calls) {
    let output = reveal(harness, app, slider);
    let (id, bounds) = slider(&output);
    harness.click(app, bounds.center());
    assert_eq!(app.controller.requested_brightness, 50);
    harness.focus(app, id);
    harness.key(app, egui::Key::ArrowRight);
    assert_eq!(app.controller.requested_brightness, 51);
    assert_eq!(calls.preview.load(Ordering::SeqCst), 0);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
    button(harness, app, "预览亮度");
    harness.wait(app, |controller| {
        controller.brightness_preview.is_some() && !controller.busy()
    });
    assert_eq!(calls.preview.load(Ordering::SeqCst), 1);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
    assert_eq!(
        calls.preview_requests.lock().unwrap().as_slice(),
        &[(51, "fixture-display".to_owned())]
    );
}
fn assert_fits(metrics: &RenderMetrics) {
    assert!(metrics.content_size.is_finite());
    assert!(metrics.content_size.x <= metrics.viewport.width() + 1.0);
    assert!(metrics.scroll_offset.x.abs() < 0.001);
}

#[test]
fn delegated_route_is_editable_without_direct_permission_and_confirmed_apply_keeps_actual_unknown()
{
    let device = device();
    assert!(!device.writable);
    assert!(device.can_control());
    let mut updated = device.clone();
    updated.control_value = Some(510);
    let result = BrightnessApplyResult {
        device_id: device.id.clone(),
        requested_percent: 51,
        confirmed: 510,
        previous: 400,
        actual_value: None,
        source: device.source.clone(),
        transaction: transaction(BrightnessOutcome::Confirmed, false),
    };
    let (mut app, calls) = fixture(device, Some(Ok(result)), Some(snapshot(updated)));
    let mut harness = Harness::new([1120, 760], 1.0);
    let output = reveal(&mut harness, &mut app, |output| {
        label_containing(output, "通过当前桌面会话调节")
    });
    assert!(text_contains(&output, "通过当前桌面会话调节"));
    assert!(
        !text_contains(&output, "SetBrightness")
            && !text_contains(&output, "/org/freedesktop/login1/")
    );
    prepare_preview(&mut harness, &mut app, &calls);
    button(&mut harness, &mut app, "应用并回读");
    harness.wait(&mut app, |controller| {
        controller.brightness_applied.is_some() && !controller.busy()
    });
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    let applied = app.controller.brightness_applied.as_ref().unwrap();
    assert_eq!(applied.confirmed, 510);
    assert_eq!(applied.actual_value, None);
    assert!(app.controller.brightness_preview.is_none());
    let readout = "请求 51%  ·  驱动报告 未采集";
    let output = reveal(&mut harness, &mut app, |output| {
        control(output, readout, egui::accesskit::Role::Label)
    });
    assert!(
        text_contains(&output, readout),
        "the brightness readout itself must show its unknown actual value"
    );
    assert!(!text_contains(&output, "驱动报告 0%") && !text_contains(&output, "实际 0%"));
    assert!(
        !text_contains(&output, "SetBrightness")
            && !text_contains(&output, "/org/freedesktop/login1/")
    );
}

#[test]
fn unknown_completed_write_needs_explicit_brightness_refresh_and_keeps_its_evidence() {
    let txn = transaction(BrightnessOutcome::Unknown, false);
    let (mut app, calls) = fixture(device(), Some(Err(failure(&txn))), None);
    let mut harness = Harness::new([640, 480], 1.5);
    prepare_preview(&mut harness, &mut app, &calls);
    button(&mut harness, &mut app, "应用并回读");
    harness.wait(&mut app, |controller| {
        controller.brightness_needs_refresh && controller.brightness_error.is_some()
    });
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert!(app.controller.brightness_preview.is_none());
    let evidence =
        serde_json::to_value(app.controller.brightness_transaction.as_ref().unwrap()).unwrap();
    let last_plan =
        serde_json::to_value(app.controller.brightness_last_plan.as_ref().unwrap()).unwrap();
    let error = serde_json::to_value(app.controller.brightness_error.as_ref().unwrap()).unwrap();
    assert_unavailable_reason(
        &mut harness,
        &mut app,
        "亮度状态未确认，请先刷新亮度状态，再重新预览",
        "亮度状态未确认，请先刷新亮度状态，再重新预览。",
    );
    assert!(app.controller.preview_brightness().is_err());
    assert_eq!(
        calls.apply.load(Ordering::SeqCst),
        1,
        "unknown outcomes must not replay the last write"
    );
    app.controller.refresh().unwrap();
    harness.wait(&mut app, |controller| !controller.busy());
    assert!(
        app.controller.brightness_needs_refresh,
        "an unrelated full refresh must not unlock brightness writes"
    );
    assert_eq!(
        serde_json::to_value(app.controller.brightness_transaction.as_ref().unwrap()).unwrap(),
        evidence
    );
    assert_eq!(
        serde_json::to_value(app.controller.brightness_error.as_ref().unwrap()).unwrap(),
        error
    );
    button(&mut harness, &mut app, "刷新亮度状态");
    harness.wait(&mut app, |controller| {
        !controller.brightness_needs_refresh && !controller.busy()
    });
    assert_eq!(
        serde_json::to_value(app.controller.brightness_transaction.as_ref().unwrap()).unwrap(),
        evidence
    );
    assert_eq!(
        serde_json::to_value(app.controller.brightness_last_plan.as_ref().unwrap()).unwrap(),
        last_plan
    );
    assert_eq!(
        serde_json::to_value(app.controller.brightness_error.as_ref().unwrap()).unwrap(),
        error
    );
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    app.controller.set_brightness_target(51);
    button(&mut harness, &mut app, "预览亮度");
    harness.wait(&mut app, |controller| {
        controller.brightness_preview.is_some() && !controller.busy()
    });
    assert_eq!(calls.preview.load(Ordering::SeqCst), 2);
    assert_eq!(
        calls.apply.load(Ordering::SeqCst),
        1,
        "a successful refresh only permits a new preview, never automatic apply"
    );
}

#[test]
fn an_unfinished_writer_remains_blocked_after_both_refresh_paths() {
    let txn = transaction(BrightnessOutcome::Unknown, true);
    let (mut app, calls) = fixture(device(), Some(Err(failure(&txn))), None);
    let mut harness = Harness::new([640, 480], 2.0);
    prepare_preview(&mut harness, &mut app, &calls);
    button(&mut harness, &mut app, "应用并回读");
    harness.wait(&mut app, |controller| {
        controller.brightness_writer_may_continue && controller.brightness_error.is_some()
    });
    let evidence =
        serde_json::to_value(app.controller.brightness_transaction.as_ref().unwrap()).unwrap();
    assert_unavailable_reason(
        &mut harness,
        &mut app,
        "尚未确认上次调节结束，不能再次提交",
        "尚未确认上次调节结束，不能再次提交。刷新只能核对当前读数。",
    );
    let ordinary = app.controller.refresh();
    if ordinary.is_ok() {
        harness.wait(&mut app, |controller| controller.activity().is_none());
    }
    let explicit = app.controller.refresh_brightness_state();
    if explicit.is_ok() {
        harness.wait(&mut app, |controller| controller.activity().is_none());
    }
    assert!(app.controller.brightness_writer_may_continue);
    assert!(app.controller.brightness_needs_refresh);
    assert!(app.controller.preview_brightness().is_err());
    assert_eq!(
        serde_json::to_value(app.controller.brightness_transaction.as_ref().unwrap()).unwrap(),
        evidence
    );
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(calls.preview.load(Ordering::SeqCst), 1);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

#[test]
fn missing_control_route_cannot_offer_a_preview_or_a_slider() {
    let mut device = device();
    device.write_backend = None;
    device.logind_session = None;
    device.reason = Some("没有经过确认的亮度写入接口。".into());
    assert!(!device.can_control());
    let (mut app, calls) = fixture(device, None, None);
    let mut harness = Harness::new([640, 480], 1.0);
    let (_, output) = harness.settle(&mut app);
    assert!(!output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .any(|(_, node)| node.role() == egui::accesskit::Role::Slider && !node.is_disabled()));
    assert!(app.controller.preview_brightness().is_err());
    assert_eq!(calls.preview.load(Ordering::SeqCst), 0);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

#[test]
fn brightness_transaction_details_wrap_and_remain_accessible_at_small_window_sizes() {
    for dpi in [1.0, 2.0] {
        for outcome in [
            None,
            Some(BrightnessOutcome::Unknown),
            Some(BrightnessOutcome::RecoveryFailed),
        ] {
            let (mut app, calls) = fixture(device(), None, None);
            if let Some(outcome) = outcome {
                let mut txn = transaction(outcome, false);
                let long = "UnbrokenBrightnessReadbackDiagnostic0123456789".repeat(7);
                txn.readback_error.as_mut().unwrap().details =
                    vec![format!("fixture://{long}"), "亮度回读详情结束。".into()];
                app.controller.brightness_error = Some(failure(&txn));
                app.controller.brightness_transaction = Some(txn);
                app.controller.brightness_last_plan = Some(plan());
                app.controller.brightness_needs_refresh = true;
            }
            let mut harness = Harness::new([360, 280], dpi);
            let output = reveal(&mut harness, &mut app, |output| {
                control(
                    output,
                    "背光设备详情",
                    egui::accesskit::Role::DisclosureTriangle,
                )
            });
            let (details, bounds) = control(
                &output,
                "背光设备详情",
                egui::accesskit::Role::DisclosureTriangle,
            );
            let output = harness.click(&mut app, bounds.center());
            let (_, node) = output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|(id, _)| *id == details)
                .unwrap();
            assert_eq!(node.is_expanded(), Some(true));
            let (metrics, _) = harness.settle(&mut app);
            assert_fits(&metrics);
            if let Some(outcome) = outcome {
                let mut evidence = vec![
                    "调节错误 · fixture_write_unknown · 调节请求结果未确认。",
                    "回读错误 · fixture_readback_failed · 保留独立的回读错误。",
                    "亮度回读详情结束。",
                ];
                if outcome == BrightnessOutcome::RecoveryFailed {
                    evidence.push("恢复错误 · fixture_recovery_failed · 保留独立的恢复失败记录。");
                }
                for expected in evidence {
                    let output = reveal(&mut harness, &mut app, |output| {
                        control(output, expected, egui::accesskit::Role::Label)
                    });
                    assert!(
                        text_contains(&output, expected),
                        "expanded transaction evidence was not drawn: {expected}"
                    );
                    let (metrics, _) = harness.frame(&mut app);
                    assert_fits(&metrics);
                }
            }
            app.scroll_to_bottom();
            let (metrics, _) = harness.settle(&mut app);
            assert_fits(&metrics);
            assert!(
                metrics.bottom_marker.top() >= metrics.viewport.top() - 1.0
                    && metrics.bottom_marker.bottom() <= metrics.viewport.bottom() + 1.0
            );
            assert_eq!(calls.preview.load(Ordering::SeqCst), 0);
            assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
            assert_eq!(calls.other.load(Ordering::SeqCst), 0);
        }
    }
}
