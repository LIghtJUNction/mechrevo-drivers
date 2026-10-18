#![cfg(feature = "gui")]
//! Exercise the real power-profile renderer with local typed PPD fixtures.
//! No D-Bus service, authorization helper, power write or hardware is accessed.
use eframe::egui::{self, pos2, vec2, Context, FullOutput, RawInput, Rect};
use egui_system_font_provider::SystemFontProvider;
use mechrevo_console_core::{
    cpu_power::{CpuPowerUpdate, Outcome as CpuOutcome, Plan as CpuPlan},
    fan_curve::{client::FanClientUpdate, ValidatedFanCurve},
    power_profiles_daemon::{
        self, AdvertisedProfile, ProfileApplyOutcome, ProfileApplyReceipt, ProfileApplyStage,
        ProfileDaemonIdentity, ProfileDetails, ProfileHold, ProfileHoldEffect,
        PROFILE_RECEIPT_MARKER,
    },
    windows_oem::{
        ClientHelperLifetime, Outcome as FnOutcome, Receipt as FnReceipt, Update as FnUpdate,
    },
    ApplyResult, BrightnessApplyResult, BrightnessPlan, CatalogModel, ControlError, EvidenceSource,
    OemApplyResult, OemPlan, OemValue, PowerProfile, ProfilePlan, Snapshot, ThermalSample,
};
use mechrevo_console_gui::{
    desktop::{DesktopApp, RenderMetrics},
    Backend, Controller, Preferences,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Calls {
    snapshot: AtomicUsize,
    catalog: AtomicUsize,
    preview: AtomicUsize,
    apply: AtomicUsize,
    cleanup: AtomicUsize,
    other: AtomicUsize,
    requested: Mutex<Vec<PowerProfile>>,
    plans: Mutex<Vec<ProfilePlan>>,
}
struct Gate {
    entered: SyncSender<()>,
    release: Receiver<()>,
}
struct FixtureBackend {
    snapshot: Snapshot,
    after_apply: Option<Snapshot>,
    plan: Option<Result<ProfilePlan, ControlError>>,
    result: Option<Result<ApplyResult, ControlError>>,
    gate: Option<Gate>,
    calls: Arc<Calls>,
}
impl FixtureBackend {
    fn denied<T>(&self) -> Result<T, ControlError> {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No native backend"))
    }
    fn no_fan(&self) -> FanClientUpdate {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate {
            error: Some(ControlError::new("fixture_only", "No native backend")),
            ..Default::default()
        }
    }
    fn no_cpu(&self) -> CpuPowerUpdate {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        CpuPowerUpdate {
            status: None,
            outcome: CpuOutcome::RejectedBeforeWrite,
            error: Some(ControlError::new("fixture_only", "No native backend")),
            receipt: None,
            helper_reaped: true,
        }
    }
    fn no_fn(&self) -> FnUpdate {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        FnUpdate {
            status: None,
            outcome: FnOutcome::RejectedBeforeWrite,
            result: None,
            receipt: FnReceipt::default(),
            error: Some(ControlError::new("fixture_only", "No native backend")),
            helper_reaped: true,
            helper_lifetime: ClientHelperLifetime::NotStarted,
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
    fn plan(&mut self, requested: PowerProfile) -> Result<ProfilePlan, ControlError> {
        self.calls.preview.fetch_add(1, Ordering::SeqCst);
        self.calls.requested.lock().unwrap().push(requested);
        self.plan.take().expect("unexpected power-profile preview")
    }
    fn apply(&mut self, plan: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        self.calls.apply.fetch_add(1, Ordering::SeqCst);
        self.calls.plans.lock().unwrap().push(plan.clone());
        if let Some(gate) = self.gate.take() {
            gate.entered.send(()).unwrap();
            gate.release
                .recv_timeout(Duration::from_secs(2))
                .expect("renderer froze before releasing local transaction");
        }
        if let Some(snapshot) = self.after_apply.take() {
            self.snapshot = snapshot;
        }
        self.result.take().expect("unexpected power-profile apply")
    }
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
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
    fn cpu_power_probe(&mut self) -> CpuPowerUpdate {
        self.no_cpu()
    }
    fn cpu_power_execute(&mut self, _: &CpuPlan) -> CpuPowerUpdate {
        self.no_cpu()
    }
    fn windows_fn_probe(&mut self) -> FnUpdate {
        self.no_fn()
    }
    fn windows_fn_apply(&mut self, _: &OemPlan) -> FnUpdate {
        self.no_fn()
    }
    fn windows_fn_finish_pending(&mut self) -> FnUpdate {
        self.no_fn()
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
        self.calls.cleanup.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate::default()
    }
    fn fan_poll(&mut self) -> Option<FanClientUpdate> {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        None
    }
}

struct Harness {
    ctx: Context,
    physical: [u32; 2],
    dpi: f32,
    frame: u32,
    close_requested: bool,
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
            close_requested: false,
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
        if std::mem::take(&mut self.close_requested) {
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
        }
        let mut metrics = None;
        let mut output = self.ctx.run_ui(input, |ui| metrics = Some(app.render(ui)));
        self.frame += 1;
        // Acknowledge updates and releases even without a GPU renderer.
        output.textures_delta.set.clear();
        output.textures_delta.free.clear();
        (metrics.unwrap(), output)
    }
    fn frame(&mut self, app: &mut DesktopApp) -> (RenderMetrics, FullOutput) {
        self.events(app, vec![])
    }
    fn close(&mut self, app: &mut DesktopApp) -> FullOutput {
        self.close_requested = true;
        self.frame(app).1
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
    fn focus(&mut self, app: &mut DesktopApp, target: egui::accesskit::NodeId) {
        for _ in 0..96 {
            let output = self.key(app, egui::Key::Tab);
            if output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .focus
                == target
            {
                return;
            }
        }
        panic!("keyboard traversal did not reach the requested power-profile control");
    }
    fn wait(&mut self, app: &mut DesktopApp, done: impl Fn(&DesktopApp) -> bool) -> FullOutput {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let output = self.frame(app).1;
            if done(app) {
                return output;
            }
            assert!(
                Instant::now() < deadline,
                "fixture power-profile job did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn node_named(output: &FullOutput, node: &egui::accesskit::Node, label: &str) -> bool {
    if node.label() == Some(label) || node.value() == Some(label) {
        return true;
    }
    let nodes = &output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes;
    node.labelled_by().iter().any(|name| {
        nodes.iter().any(|(id, node)| {
            id == name && (node.value() == Some(label) || node.label() == Some(label))
        })
    })
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
        .find(|(_, node)| node.role() == role && node_named(output, node, label))
        .unwrap_or_else(|| panic!("accessible {role:?} '{label}' is missing"));
    let bounds = node.bounds().expect("interactive control needs bounds");
    (
        *id,
        Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        ),
    )
}
fn reveal(
    harness: &mut Harness,
    app: &mut DesktopApp,
    label: &str,
    role: egui::accesskit::Role,
) -> FullOutput {
    let (mut metrics, mut output) = harness.settle(app);
    for _ in 0..16 {
        let (_, bounds) = control(&output, label, role);
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
    panic!("power-profile control '{label}' remained clipped after bounded scrolling");
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
fn text_equals(output: &FullOutput, expected: &str) -> bool {
    fn visit(shape: &egui::Shape, expected: &str) -> bool {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| visit(shape, expected)),
            egui::Shape::Text(text) => text.galley.job.text == expected,
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|shape| visit(&shape.shape, expected))
}
fn assert_fits(label: &str, metrics: &RenderMetrics, harness: &Harness, output: &FullOutput) {
    assert!(metrics.content_size.is_finite());
    assert!(
        metrics.content_size.x <= metrics.viewport.width() + 1.0,
        "{label}: horizontal overflow: content={:?}, viewport={:?}",
        metrics.content_size,
        metrics.viewport
    );
    assert!(metrics.scroll_offset.x.abs() < 0.001);
    let client = Rect::from_min_size(
        pos2(0.0, 0.0),
        vec2(
            harness.physical[0] as f32 / output.pixels_per_point,
            harness.physical[1] as f32 / output.pixels_per_point,
        ),
    );
    assert!(client.expand(1.0).contains_rect(metrics.viewport));
}
fn assert_complete_layout(label: &str, harness: &mut Harness, app: &mut DesktopApp) {
    let (metrics, output) = harness.settle(app);
    assert_fits(label, &metrics, harness, &output);
    assert!((output.pixels_per_point - harness.dpi * app.preferences.text_scale).abs() < 0.001);
    app.scroll_to_bottom();
    let (metrics, output) = harness.settle(app);
    assert_fits(label, &metrics, harness, &output);
    assert!(
        metrics.bottom_marker.top() >= metrics.viewport.top() - 1.0
            && metrics.bottom_marker.bottom() <= metrics.viewport.bottom() + 1.0,
        "{label}: power-profile page bottom cannot be reached"
    );
}

fn hold() -> ProfileHold {
    ProfileHold {
        application_id: "fixture.builder".into(),
        profile: PowerProfile::Performance,
        reason: "本地构建任务".into(),
        attributes: BTreeMap::new(),
    }
}
fn details(release_holds: bool) -> ProfileDetails {
    ProfileDetails {
        daemon: ProfileDaemonIdentity {
            bus_id: "a".repeat(32),
            unique_owner: ":1.99".into(),
            bus_name: "org.freedesktop.UPower.PowerProfiles".into(),
            object_path: "/org/freedesktop/UPower/PowerProfiles".into(),
            interface: "org.freedesktop.UPower.PowerProfiles".into(),
            unix_user: 0,
        },
        performance_degraded: "lap-detected".into(),
        holds: vec![hold()],
        advertised_profiles: [
            PowerProfile::PowerSaver,
            PowerProfile::Balanced,
            PowerProfile::Performance,
        ]
        .into_iter()
        .map(|profile| AdvertisedProfile {
            profile,
            attributes: BTreeMap::from([("Driver".into(), "fixture-only-not-a-service".into())]),
        })
        .collect(),
        holds_will_be_released: release_holds,
    }
}
fn source() -> EvidenceSource {
    EvidenceSource {
        backend: power_profiles_daemon::BACKEND.into(),
        path: details(false).daemon.object_path,
        device_id: Some("b".repeat(64)),
    }
}
fn snapshot_fixture(current: &str, details: Option<ProfileDetails>) -> Snapshot {
    serde_json::from_value(json!({
        "platform":"linux", "identity":{"manufacturer":"TEST FIXTURE","product_name":"系统电源策略界面测试设备","board_name":"NO-HARDWARE-BINDING","bios_version":"TEST.0","mechrevo_brand_detected":false,"support_note":"纯本地typed PPD测试，不连接系统总线。"},
        "telemetry":[],"brightness":[],"oem_controls":[],"diagnostics":[],
        "capabilities":[{"id":"power_profile","label":"系统电源策略","status":"available","reason":"本地PPD fixture","source":source(),"options":["power-saver","balanced","performance"]}],
        "power_policy":{"scope":power_profiles_daemon::SCOPE,"current":current,"options":["power-saver","balanced","performance"],"source":source(),"details":details}
    })).unwrap()
}
fn plan() -> ProfilePlan {
    ProfilePlan {
        requested: PowerProfile::PowerSaver,
        target: "power-saver".into(),
        previous: "balanced".into(),
        scope: power_profiles_daemon::SCOPE.into(),
        source: source(),
        details: Some(details(true)),
    }
}
fn receipt(outcome: ProfileApplyOutcome, effect: ProfileHoldEffect) -> ProfileApplyReceipt {
    let confirmed = outcome == ProfileApplyOutcome::Confirmed;
    ProfileApplyReceipt {
        outcome,
        stage: if confirmed {
            ProfileApplyStage::Complete
        } else {
            ProfileApplyStage::Setter
        },
        submitted: true,
        acknowledged: confirmed.then_some(true),
        readback: if confirmed {
            Some("power-saver".into())
        } else if effect == ProfileHoldEffect::ReleasedObserved {
            Some("balanced".into())
        } else {
            None
        },
        authority_unchanged: Some(true),
        holds_before: vec![hold()],
        holds_after: (effect != ProfileHoldEffect::Unknown).then(Vec::new),
        hold_effect: effect,
        primary_error: (!confirmed).then(|| {
            ControlError::new(
                "fixture_ppd_set_failed",
                "服务更改返回失败，结果以独立回读为准。",
            )
        }),
        readback_error: (!confirmed && effect == ProfileHoldEffect::Unknown).then(|| {
            ControlError::new("fixture_ppd_readback_failed", "独立回读未能确认当前档位。")
        }),
        writer_may_continue: !confirmed,
    }
}
fn failure(receipt: &ProfileApplyReceipt) -> ControlError {
    let mut error = ControlError::new("fixture_ppd_result_unknown", "电源服务未能确认这次请求。");
    error.details = vec![format!(
        "{PROFILE_RECEIPT_MARKER}{}",
        serde_json::to_string(receipt).unwrap()
    )];
    error
}
fn success() -> ApplyResult {
    ApplyResult {
        requested: PowerProfile::PowerSaver,
        confirmed: "power-saver".into(),
        previous: "balanced".into(),
        scope: power_profiles_daemon::SCOPE.into(),
        receipt: Some(receipt(
            ProfileApplyOutcome::Confirmed,
            ProfileHoldEffect::ReleasedObserved,
        )),
    }
}
fn fixture(
    result: Option<Result<ApplyResult, ControlError>>,
    gate: Option<Gate>,
) -> (DesktopApp, Arc<Calls>) {
    let snapshot = snapshot_fixture("balanced", Some(details(false)));
    assert!(
        mechrevo_console_core::profile_available(&snapshot, PowerProfile::PowerSaver),
        "PPD must advertise power-saver rather than the sysfs low-power spelling"
    );
    let calls = Arc::new(Calls::default());
    let mut after = details(false);
    after.holds.clear();
    let mut controller = Controller::new(FixtureBackend {
        snapshot: snapshot.clone(),
        after_apply: Some(snapshot_fixture("power-saver", Some(after))),
        plan: Some(Ok(plan())),
        result,
        gate,
        calls: calls.clone(),
    })
    .unwrap();
    controller.snapshot = Some(snapshot);
    (
        DesktopApp::with_controller(
            controller,
            Preferences {
                auto_refresh: false,
                ..Default::default()
            },
        ),
        calls,
    )
}
fn counts(calls: &Calls) -> [usize; 6] {
    [
        &calls.snapshot,
        &calls.catalog,
        &calls.preview,
        &calls.apply,
        &calls.cleanup,
        &calls.other,
    ]
    .map(|counter| counter.load(Ordering::SeqCst))
}
fn bounds(node: &egui::accesskit::Node) -> Rect {
    let b = node.bounds().unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}
fn system_control(
    output: &FullOutput,
    label: &str,
    role: egui::accesskit::Role,
) -> (egui::accesskit::NodeId, Rect) {
    let (_, heading) = control(output, "系统电源策略", egui::accesskit::Role::Label);
    let (_, brightness) = control(output, "屏幕亮度", egui::accesskit::Role::Label);
    let (id, node) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| {
            if node.role() != role || !node_named(output, node, label) {
                return false;
            }
            let rect = bounds(node);
            if brightness.left() > heading.right() + 10.0 {
                rect.right() < brightness.left() && rect.top() >= heading.bottom()
            } else {
                rect.top() >= heading.bottom() && rect.bottom() < brightness.top()
            }
        })
        .unwrap_or_else(|| panic!("{role:?} '{label}' missing from the system power card"));
    (*id, bounds(node))
}
fn reveal_system(
    harness: &mut Harness,
    app: &mut DesktopApp,
    label: &str,
    role: egui::accesskit::Role,
) -> FullOutput {
    let (mut metrics, mut output) = harness.settle(app);
    for _ in 0..20 {
        let (_, rect) = system_control(&output, label, role);
        if metrics.viewport.expand(1.0).contains_rect(rect) {
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
                        (metrics.viewport.center().y - rect.center().y).clamp(-200.0, 200.0),
                    ),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        (metrics, output) = harness.settle(app);
    }
    panic!("system power control '{label}' remained clipped");
}
fn click_system(harness: &mut Harness, app: &mut DesktopApp, label: &str) -> FullOutput {
    let output = reveal_system(harness, app, label, egui::accesskit::Role::Button);
    let (_, rect) = system_control(&output, label, egui::accesskit::Role::Button);
    harness.click(app, rect.center())
}
fn show_system(harness: &mut Harness, app: &mut DesktopApp, label: &str) -> FullOutput {
    let output = reveal_system(harness, app, label, egui::accesskit::Role::Label);
    assert!(
        text_equals(&output, label),
        "system power label was not actually drawn: {label}"
    );
    output
}
fn refresh(harness: &mut Harness, app: &mut DesktopApp) {
    let output = reveal(harness, app, "刷新", egui::accesskit::Role::Button);
    let (_, rect) = control(&output, "刷新", egui::accesskit::Role::Button);
    harness.click(app, rect.center());
    harness.wait(app, |app| !app.controller.busy());
}
fn prepare_preview(harness: &mut Harness, app: &mut DesktopApp, calls: &Calls) {
    click_system(harness, app, "节能");
    assert_eq!(app.controller.selected_profile, PowerProfile::PowerSaver);
    assert_eq!(calls.preview.load(Ordering::SeqCst), 0);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
    click_system(harness, app, "预览更改");
    harness.wait(app, |app| {
        app.controller.preview.is_some() && !app.controller.busy()
    });
    assert_eq!(
        calls.requested.lock().unwrap().as_slice(),
        &[PowerProfile::PowerSaver]
    );
    let preview = app.controller.preview.as_ref().unwrap();
    assert_eq!(preview.target, "power-saver");
    assert_eq!(preview.source.backend, power_profiles_daemon::BACKEND);
    assert!(preview.details.as_ref().unwrap().holds_will_be_released);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
}
fn open_details(harness: &mut Harness, app: &mut DesktopApp) {
    let output = reveal_system(
        harness,
        app,
        "电源策略详情",
        egui::accesskit::Role::DisclosureTriangle,
    );
    let (id, rect) = system_control(
        &output,
        "电源策略详情",
        egui::accesskit::Role::DisclosureTriangle,
    );
    let output = harness.click(app, rect.center());
    let (_, node) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == id)
        .unwrap();
    assert_eq!(node.is_expanded(), Some(true));
    harness.settle(app);
}
fn assert_no_false_success(output: &FullOutput) {
    for phrase in ["已应用并回读：", "已恢复", "恢复成功"] {
        assert!(!text_contains(output, phrase));
        assert!(
            !output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|(_, node)| node
                    .label()
                    .or(node.value())
                    .is_some_and(|text| text.contains(phrase))),
            "false profile result in semantic tree: {phrase}"
        );
    }
}
fn commands(output: &FullOutput) -> &[egui::ViewportCommand] {
    &output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .unwrap()
        .commands
}

#[test]
fn refreshing_and_real_pointer_keyboard_selection_never_apply_a_ppd_profile() {
    let (mut app, calls) = fixture(None, None);
    let mut harness = Harness::new([1120, 760], 1.0);
    harness.settle(&mut app);
    assert_eq!(counts(&calls), [0; 6]);
    refresh(&mut harness, &mut app);
    assert_eq!(counts(&calls), [1, 1, 0, 0, 0, 0]);
    click_system(&mut harness, &mut app, "节能");
    assert_eq!(app.controller.selected_profile, PowerProfile::PowerSaver);
    let output = reveal_system(
        &mut harness,
        &mut app,
        "平衡",
        egui::accesskit::Role::Button,
    );
    let (id, _) = system_control(&output, "平衡", egui::accesskit::Role::Button);
    harness.focus(&mut app, id);
    harness.key(&mut app, egui::Key::Space);
    assert_eq!(app.controller.selected_profile, PowerProfile::Balanced);
    assert!(app.controller.preview.is_none());
    assert_eq!(counts(&calls), [1, 1, 0, 0, 0, 0]);
    prepare_preview(&mut harness, &mut app, &calls);
    show_system(&mut harness, &mut app, "系统电源服务");
    show_system(
        &mut harness,
        &mut app,
        "系统报告放腿使用限制，性能暂时受限。",
    );
    show_system(&mut harness, &mut app, "1 项应用临时请求正在生效。");
    let output = reveal_system(
        &mut harness,
        &mut app,
        "应用并回读",
        egui::accesskit::Role::Button,
    );
    let (_, warning) = system_control(
        &output,
        "应用此档位会释放 1 项应用临时请求。",
        egui::accesskit::Role::Label,
    );
    let (_, apply) = system_control(&output, "应用并回读", egui::accesskit::Role::Button);
    assert!(warning.bottom() <= apply.top());
    assert!(
        text_equals(&output, "应用此档位会释放 1 项应用临时请求。"),
        "hold-release warning must be visibly beside the apply flow"
    );
    let (id, _) = system_control(
        &output,
        "电源策略详情",
        egui::accesskit::Role::DisclosureTriangle,
    );
    let (_, node) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == id)
        .unwrap();
    assert_eq!(node.is_expanded(), Some(false));
    assert_eq!(counts(&calls), [1, 1, 1, 0, 0, 0]);
}

#[test]
fn applying_the_ppd_preview_uses_the_actual_power_saver_target_and_preserves_its_receipt() {
    let (mut app, calls) = fixture(Some(Ok(success())), None);
    let mut harness = Harness::new([640, 480], 1.5);
    prepare_preview(&mut harness, &mut app, &calls);
    let expected = serde_json::to_value(app.controller.preview.as_ref().unwrap()).unwrap();
    click_system(&mut harness, &mut app, "应用并回读");
    harness.wait(&mut app, |app| {
        app.controller.applied.is_some() && !app.controller.busy()
    });
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(calls.plans.lock().unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(&calls.plans.lock().unwrap()[0]).unwrap(),
        expected
    );
    assert!(app.controller.preview.is_none());
    assert!(app.controller.profile_error.is_none());
    assert_eq!(
        app.controller.profile_receipt.as_ref().unwrap().outcome,
        ProfileApplyOutcome::Confirmed
    );
    show_system(&mut harness, &mut app, "当前  节能");
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

#[test]
fn failed_ppd_receipts_keep_unknown_and_observed_releases_distinct_through_refresh_and_selection() {
    for effect in [
        ProfileHoldEffect::Unknown,
        ProfileHoldEffect::ReleasedObserved,
    ] {
        let receipt = receipt(ProfileApplyOutcome::Unknown, effect);
        let evidence = serde_json::to_value(&receipt).unwrap();
        let (mut app, calls) = fixture(Some(Err(failure(&receipt))), None);
        let mut harness = Harness::new([640, 480], 1.0);
        prepare_preview(&mut harness, &mut app, &calls);
        click_system(&mut harness, &mut app, "应用并回读");
        harness.wait(&mut app, |app| {
            app.controller.profile_error.is_some() && !app.controller.busy()
        });
        assert!(app.controller.applied.is_none());
        assert!(app.controller.preview.is_none());
        assert_eq!(
            serde_json::to_value(app.controller.profile_receipt.as_ref().unwrap()).unwrap(),
            evidence
        );
        let output = show_system(
            &mut harness,
            &mut app,
            "本次更改结果未确认；未自动重试或恢复。",
        );
        assert_no_false_success(&output);
        show_system(
            &mut harness,
            &mut app,
            "先前请求仍可能生效；刷新不能证明该请求已结束。",
        );
        if effect == ProfileHoldEffect::ReleasedObserved {
            show_system(
                &mut harness,
                &mut app,
                "回读时原有的应用临时请求已不在列表中。",
            );
        } else {
            let output = show_system(&mut harness, &mut app, "应用临时请求的保留状态未确认。");
            assert!(!text_contains(
                &output,
                "回读时原有的应用临时请求已不在列表中。"
            ));
        }
        refresh(&mut harness, &mut app);
        click_system(&mut harness, &mut app, "性能");
        assert_eq!(
            serde_json::to_value(app.controller.profile_receipt.as_ref().unwrap()).unwrap(),
            evidence
        );
        assert!(app.controller.profile_error.is_some());
        assert!(app.controller.applied.is_none());
        assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
        assert_eq!(calls.preview.load(Ordering::SeqCst), 1);
        assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn expanded_ppd_evidence_and_main_warnings_fit_small_windows_and_dpi() {
    for dpi in [1.0, 2.0] {
        for case in ["preview", "unknown", "long_error"] {
            let (mut app, calls) = fixture(None, None);
            if case == "preview" {
                app.controller.selected_profile = PowerProfile::PowerSaver;
                app.controller.preview = Some(plan());
            } else {
                let mut receipt = receipt(ProfileApplyOutcome::Unknown, ProfileHoldEffect::Unknown);
                if case == "long_error" {
                    receipt.readback_error.as_mut().unwrap().details = vec![
                        format!(
                            "fixture://{}",
                            "UnbrokenPPDReadbackDiagnostic0123456789".repeat(9)
                        ),
                        "电源回读详情结束。".into(),
                    ];
                }
                app.controller.profile_error = Some(failure(&receipt));
                app.controller.profile_receipt = Some(receipt);
            }
            let mut harness = Harness::new(
                if case == "long_error" {
                    [360, 280]
                } else {
                    [640, 480]
                },
                dpi,
            );
            open_details(&mut harness, &mut app);
            if case == "long_error" {
                show_system(&mut harness, &mut app, "电源回读详情结束。");
            }
            assert_complete_layout(case, &mut harness, &mut app);
            let output = reveal_system(
                &mut harness,
                &mut app,
                "预览更改",
                egui::accesskit::Role::Button,
            );
            assert!(text_equals(&output, "预览更改"));
            if case == "preview" {
                let output = reveal_system(
                    &mut harness,
                    &mut app,
                    "应用并回读",
                    egui::accesskit::Role::Button,
                );
                assert!(text_equals(&output, "应用并回读"));
            }
            assert_eq!(counts(&calls), [0; 6]);
        }
    }
}

#[test]
fn closing_during_a_ppd_transaction_waits_for_the_worker_and_keeps_the_typed_result() {
    for failed in [false, true] {
        let result_receipt = receipt(
            if failed {
                ProfileApplyOutcome::Unknown
            } else {
                ProfileApplyOutcome::Confirmed
            },
            if failed {
                ProfileHoldEffect::Unknown
            } else {
                ProfileHoldEffect::ReleasedObserved
            },
        );
        let evidence = serde_json::to_value(&result_receipt).unwrap();
        let result = if failed {
            Err(failure(&result_receipt))
        } else {
            Ok(success())
        };
        let (entered_send, entered_recv) = mpsc::sync_channel(1);
        let (release_send, release_recv) = mpsc::sync_channel(1);
        let (mut app, calls) = fixture(
            Some(result),
            Some(Gate {
                entered: entered_send,
                release: release_recv,
            }),
        );
        let mut harness = Harness::new([800, 600], 1.0);
        prepare_preview(&mut harness, &mut app, &calls);
        click_system(&mut harness, &mut app, "应用并回读");
        entered_recv
            .recv_timeout(Duration::from_secs(2))
            .expect("local PPD worker did not enter apply");
        let started = Instant::now();
        let output = harness.close(&mut app);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "close handling blocked on the setter"
        );
        assert!(commands(&output).contains(&egui::ViewportCommand::CancelClose));
        assert!(!commands(&output).contains(&egui::ViewportCommand::Close));
        assert!(app.controller.write_in_progress());
        assert!(!app.controller.close_ready());
        let output = harness.frame(&mut app).1;
        assert!(!commands(&output).contains(&egui::ViewportCommand::Close));
        release_send.send(()).unwrap();
        let output = harness.wait(&mut app, |app| app.controller.close_ready());
        assert!(commands(&output).contains(&egui::ViewportCommand::Close));
        assert_eq!(
            serde_json::to_value(app.controller.profile_receipt.as_ref().unwrap()).unwrap(),
            evidence
        );
        if failed {
            assert!(app.controller.applied.is_none());
            assert!(app.controller.profile_error.is_some());
            assert_no_false_success(&output);
        } else {
            assert_eq!(
                app.controller.applied.as_ref().unwrap().confirmed,
                "power-saver"
            );
        }
        assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
        assert_eq!(calls.cleanup.load(Ordering::SeqCst), 1);
        assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    }
}
