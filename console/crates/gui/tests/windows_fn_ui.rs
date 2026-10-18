#![cfg(feature = "gui")]
//! Real-renderer Windows Fn-lock regressions with local typed backends only.
//! Candidate DTO fixtures never establish Windows or physical-key acceptance.
use eframe::egui::{self, pos2, vec2, Context, FullOutput, RawInput, Rect};
use egui_system_font_provider::SystemFontProvider;
use mechrevo_console_core::{
    cpu_power::{CpuPowerUpdate, Outcome as CpuOutcome, Plan as CpuPlan},
    fan_curve::{client::FanClientUpdate, ValidatedFanCurve},
    windows_oem::{
        self, contract, ClientHelperLifetime, IdentityEvidence, LoadedModuleEvidence, Outcome,
        Receipt, Recovery, Status, SupportStage, Update,
    },
    ApplyResult, BrightnessApplyResult, BrightnessPlan, CapabilityStatus, CatalogModel,
    ControlError, EvidenceSource, OemApplyResult, OemControl, OemControlKind, OemDomain, OemPlan,
    OemValue, PowerProfile, ProfilePlan, Snapshot, ThermalSample,
};
use mechrevo_console_gui::{
    desktop::{ControlSection, DesktopApp, RenderMetrics, Section},
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
    probe: AtomicUsize,
    apply: AtomicUsize,
    finish: AtomicUsize,
    other: AtomicUsize,
    snapshot: AtomicUsize,
    catalog: AtomicUsize,
    plans: Mutex<Vec<OemPlan>>,
}
struct FixtureBackend {
    snapshot: Snapshot,
    probes: VecDeque<Update>,
    applies: VecDeque<Update>,
    finishes: VecDeque<Update>,
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
    fn windows_fn_probe(&mut self) -> Update {
        self.calls.probe.fetch_add(1, Ordering::SeqCst);
        self.probes.pop_front().expect("unexpected Fn-lock probe")
    }
    fn windows_fn_apply(&mut self, plan: &OemPlan) -> Update {
        self.calls.apply.fetch_add(1, Ordering::SeqCst);
        self.calls.plans.lock().unwrap().push(plan.clone());
        self.applies.pop_front().expect("unexpected Fn-lock apply")
    }
    fn windows_fn_finish_pending(&mut self) -> Update {
        self.calls.finish.fetch_add(1, Ordering::SeqCst);
        self.finishes
            .pop_front()
            .expect("unexpected retained-child wait")
    }
    fn snapshot(&mut self) -> Snapshot {
        self.calls.snapshot.fetch_add(1, Ordering::SeqCst);
        self.snapshot.clone()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        self.calls.catalog.fetch_add(1, Ordering::SeqCst);
        vec![]
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
fn snapshot_fixture(platform: &str) -> Snapshot {
    serde_json::from_value(json!({
        "platform": platform,
        "identity": {"manufacturer":"TEST FIXTURE", "product_name":"Fn锁界面测试设备", "product_version":null,
            "board_name":"NO-HARDWARE-BINDING", "bios_version":"TEST.0", "mechrevo_brand_detected":false,
            "support_note":"纯本地界面测试，不访问Windows驱动或物理按键。"},
        "telemetry":[], "brightness":[], "oem_controls":[], "capabilities":[], "diagnostics":[],
        "power_policy":{"scope":"fixture-only", "current":null, "options":[], "source":null}
    })).unwrap()
}
fn status_fixture(current: bool) -> Status {
    let contract_id = "fixture-only-not-hardware";
    let device_instance = r"ACPI\INOU0000\OFFLINE-UI-FIXTURE";
    let status = Status {
        contract_id: Some(contract_id.into()),
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
                device_id: Some(device_instance.into()),
            },
            binding: "a".repeat(64),
            readback_scope: contract::READBACK_SCOPE.into(),
            reason: Some("仅离线整字节回读fixture，不是物理键验证。".into()),
        }),
        evidence: IdentityEvidence {
            disk_file_sha256: Some(
                "6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235".into(),
            ),
            loaded_module_metadata: Some(LoadedModuleEvidence {
                service: "UWACPIDriver".into(),
                device_instance_id: device_instance.into(),
                driver_inf: "oem-offline-ui-fixture.inf".into(),
                driver_version: "10.48.11.331".into(),
                loaded_path: r"C:\Windows\System32\drivers\UWACPIDriver.sys".into(),
                association_level: "Offline synthetic association, not Windows acceptance".into(),
            }),
            ..IdentityEvidence::default()
        },
        blocking_errors: vec![],
        diagnostics: vec![
            "Offline synthetic identity only; no Windows or physical-key acceptance".into(),
        ],
    };
    windows_oem::preview(&status, !current)
        .expect("local typed fixture must pass the real preview validation");
    status
}
fn observed(current: bool) -> Update {
    Update {
        status: Some(status_fixture(current)),
        outcome: Outcome::Observed,
        result: None,
        receipt: Receipt::default(),
        error: None,
        helper_reaped: true,
        helper_lifetime: ClientHelperLifetime::Reaped,
    }
}
fn applied() -> Update {
    let plan = windows_oem::preview(&status_fixture(false), true).unwrap();
    Update {
        status: Some(status_fixture(true)),
        outcome: Outcome::AppliedVerified,
        result: Some(OemApplyResult {
            control_id: plan.control_id,
            kind: plan.kind,
            previous: plan.previous,
            confirmed: plan.target,
            source: plan.source,
            readback_scope: plan.readback_scope,
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
        helper_lifetime: ClientHelperLifetime::Reaped,
    }
}
fn unknown(live_writer: bool) -> Update {
    let error = ControlError::new("fixture_fn_apply_unknown", "Fn锁请求未确认，保留原始错误。");
    let mut status = status_fixture(false);
    status.control = None;
    status.blocking_errors.push(ControlError::new(
        "fixture_fn_probe_required",
        "需要重新显式检查。",
    ));
    Update {
        status: Some(status),
        outcome: Outcome::FailedStateUnknown,
        result: None,
        receipt: Receipt {
            write_attempted: true,
            write_call_succeeded: Some(true),
            full_byte_verified: Some(false),
            writer_may_continue: live_writer,
            recovery: Recovery::Unknown,
            primary_error: Some(error.clone()),
            readback_errors: vec![ControlError::new(
                "fixture_fn_readback_failed",
                "独立回读未能确认完整字节。",
            )],
            recovery_reason: Some("状态不确定，禁止自动重放或盲目恢复。".into()),
            ..Default::default()
        },
        error: Some(error),
        helper_reaped: !live_writer,
        helper_lifetime: if live_writer {
            ClientHelperLifetime::Unknown
        } else {
            ClientHelperLifetime::Reaped
        },
    }
}
fn unsupported() -> Update {
    let error = ControlError::new("fixture_fn_unsupported", "此离线测试契约不具备写入资格。");
    Update {
        status: Some(Status {
            blocking_errors: vec![error.clone()],
            ..Default::default()
        }),
        outcome: Outcome::RejectedBeforeWrite,
        result: None,
        receipt: Receipt::default(),
        error: Some(error),
        helper_reaped: true,
        helper_lifetime: ClientHelperLifetime::Reaped,
    }
}
fn fixture(
    platform: &str,
    probes: Vec<Update>,
    applies: Vec<Update>,
    finishes: Vec<Update>,
) -> (DesktopApp, Arc<Calls>) {
    let snapshot = snapshot_fixture(platform);
    let calls = Arc::new(Calls::default());
    let mut controller = Controller::new(FixtureBackend {
        snapshot: snapshot.clone(),
        probes: probes.into(),
        applies: applies.into(),
        finishes: finishes.into(),
        calls: calls.clone(),
    })
    .unwrap();
    controller.snapshot = Some(snapshot);
    let mut app = DesktopApp::with_controller(
        controller,
        Preferences {
            auto_refresh: false,
            ..Default::default()
        },
    );
    app.select_section(Section::Controls);
    app.select_control_section(ControlSection::WindowsFn);
    (app, calls)
}
fn assert_no_jobs(calls: &Calls) {
    for count in [
        &calls.probe,
        &calls.apply,
        &calls.finish,
        &calls.other,
        &calls.snapshot,
        &calls.catalog,
    ] {
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
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
        // Acknowledge updates and releases even without a GPU renderer.
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
        panic!("keyboard traversal did not reach the requested Fn-lock control");
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
                "fixture Fn-lock job did not finish"
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
    panic!("Fn-lock control '{label}' remained clipped after bounded scrolling");
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
        "{label}: Fn-lock page bottom cannot be reached"
    );
}

fn click_button(harness: &mut Harness, app: &mut DesktopApp, label: &str) -> FullOutput {
    let output = reveal(harness, app, label, egui::accesskit::Role::Button);
    let (_, bounds) = control(&output, label, egui::accesskit::Role::Button);
    harness.click(app, bounds.center())
}
fn show_label(harness: &mut Harness, app: &mut DesktopApp, label: &str) -> FullOutput {
    let output = reveal(harness, app, label, egui::accesskit::Role::Label);
    assert!(
        text_equals(&output, label),
        "Fn-lock label was not drawn: {label}"
    );
    output
}
fn prepare_preview(harness: &mut Harness, app: &mut DesktopApp, calls: &Calls) {
    click_button(harness, app, "检查 Windows Fn 锁（需授权）");
    harness.wait(app, |app| {
        !app.controller.windows_fn_needs_probe && !app.controller.busy()
    });
    show_label(harness, app, "上次检查的请求状态：关闭");
    show_label(harness, app, "候选支持：尚未完成 Windows 实机验收。");
    let output = reveal(
        harness,
        app,
        "锁定 Fn 功能键",
        egui::accesskit::Role::CheckBox,
    );
    let (checkbox, bounds) = control(&output, "锁定 Fn 功能键", egui::accesskit::Role::CheckBox);
    harness.click(app, bounds.center());
    assert_eq!(app.controller.windows_fn_requested, Some(true));
    harness.focus(app, checkbox);
    harness.key(app, egui::Key::Space);
    assert_eq!(app.controller.windows_fn_requested, Some(false));
    harness.key(app, egui::Key::Space);
    assert_eq!(app.controller.windows_fn_requested, Some(true));
    assert_eq!(calls.probe.load(Ordering::SeqCst), 1);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
    assert_eq!(calls.finish.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    click_button(harness, app, "预览 Fn 锁更改");
    let expected = windows_oem::preview(&status_fixture(false), true).unwrap();
    assert_eq!(app.controller.windows_fn_preview.as_ref(), Some(&expected));
    assert_eq!(
        calls.apply.load(Ordering::SeqCst),
        0,
        "preview must be a local operation"
    );
}
fn open_details(harness: &mut Harness, app: &mut DesktopApp) {
    let output = reveal(
        harness,
        app,
        "Windows Fn 锁详情",
        egui::accesskit::Role::DisclosureTriangle,
    );
    let (details, bounds) = control(
        &output,
        "Windows Fn 锁详情",
        egui::accesskit::Role::DisclosureTriangle,
    );
    let output = harness.click(app, bounds.center());
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
    harness.settle(app);
}

#[test]
fn explicit_probe_and_real_boolean_editing_preview_locally_before_the_apply_worker() {
    let (mut app, calls) = fixture("windows", vec![observed(false)], vec![applied()], vec![]);
    app.select_control_section(ControlSection::DeviceFunctions);
    let mut harness = Harness::new([1120, 760], 1.0);
    harness.settle(&mut app);
    assert_no_jobs(&calls);
    click_button(&mut harness, &mut app, "Windows Fn 锁");
    harness.settle(&mut app);
    assert_no_jobs(&calls);
    assert!(app.controller.windows_fn_needs_probe);
    prepare_preview(&mut harness, &mut app, &calls);
    let expected = app.controller.windows_fn_preview.clone().unwrap();
    click_button(&mut harness, &mut app, "应用 Fn 锁（需授权）");
    harness.wait(&mut app, |app| {
        app.controller.windows_fn_result.is_some() && !app.controller.busy()
    });
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(calls.plans.lock().unwrap().as_slice(), &[expected]);
    assert_eq!(calls.probe.load(Ordering::SeqCst), 1);
    assert_eq!(calls.finish.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    assert!(app.controller.windows_fn_preview.is_none());
    assert_eq!(
        app.controller.windows_fn_outcome,
        Some(Outcome::AppliedVerified)
    );
    let receipt = app.controller.windows_fn_receipt.as_ref().unwrap();
    assert_eq!(receipt.full_byte_verified, Some(true));
    assert_eq!(receipt.protected_bits_preserved, Some(true));
    show_label(&mut harness, &mut app, "上次检查的请求状态：开启");
    show_label(&mut harness, &mut app, "Fn 锁设置已回读确认。");
}

#[test]
fn unsupported_probe_cannot_enable_a_boolean_write() {
    let (mut app, calls) = fixture("windows", vec![unsupported()], vec![], vec![]);
    let mut harness = Harness::new([640, 480], 1.5);
    click_button(&mut harness, &mut app, "检查 Windows Fn 锁（需授权）");
    harness.wait(&mut app, |app| {
        app.controller.windows_fn_error.is_some() && !app.controller.busy()
    });
    show_label(&mut harness, &mut app, "上次检查的请求状态：未确认");
    assert!(app.controller.preview_windows_fn().is_err());
    assert!(app.controller.apply_windows_fn_preview().is_err());
    assert_eq!(calls.probe.load(Ordering::SeqCst), 1);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
    assert_eq!(calls.finish.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

#[test]
fn a_reaped_but_unknown_write_needs_a_new_explicit_probe_without_replaying_the_write() {
    let update = unknown(false);
    let evidence = serde_json::to_value(&update.receipt).unwrap();
    let (mut app, calls) = fixture(
        "windows",
        vec![observed(false), observed(false)],
        vec![update],
        vec![],
    );
    let mut harness = Harness::new([640, 480], 1.5);
    prepare_preview(&mut harness, &mut app, &calls);
    click_button(&mut harness, &mut app, "应用 Fn 锁（需授权）");
    harness.wait(&mut app, |app| {
        app.controller.windows_fn_state_unknown && !app.controller.busy()
    });
    assert!(app.controller.windows_fn_lifetime_clean());
    assert!(app.controller.windows_fn_needs_probe);
    show_label(&mut harness, &mut app, "上次检查的请求状态：未确认");
    assert!(app.controller.preview_windows_fn().is_err());
    assert!(app.controller.apply_windows_fn_preview().is_err());
    for _ in 0..4 {
        harness.frame(&mut app);
    }
    assert_eq!(calls.probe.load(Ordering::SeqCst), 1);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(calls.finish.load(Ordering::SeqCst), 0);
    assert_eq!(
        serde_json::to_value(app.controller.windows_fn_receipt.as_ref().unwrap()).unwrap(),
        evidence
    );
    click_button(&mut harness, &mut app, "检查 Windows Fn 锁（需授权）");
    harness.wait(&mut app, |app| {
        !app.controller.windows_fn_state_unknown
            && !app.controller.windows_fn_needs_probe
            && !app.controller.busy()
    });
    assert_eq!(
        serde_json::to_value(app.controller.windows_fn_receipt.as_ref().unwrap()).unwrap(),
        evidence
    );
    click_button(&mut harness, &mut app, "预览 Fn 锁更改");
    assert!(app.controller.windows_fn_preview.is_some());
    assert_eq!(calls.probe.load(Ordering::SeqCst), 2);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

#[test]
fn a_live_or_unconfirmed_helper_only_finishes_when_its_existing_child_is_explicitly_waited() {
    let pending = unknown(true);
    let mut finished = unknown(false);
    finished.helper_lifetime = ClientHelperLifetime::Reaped;
    let (mut app, calls) = fixture(
        "windows",
        vec![observed(false), observed(false)],
        vec![pending],
        vec![finished],
    );
    let mut harness = Harness::new([640, 480], 2.0);
    prepare_preview(&mut harness, &mut app, &calls);
    click_button(&mut harness, &mut app, "应用 Fn 锁（需授权）");
    harness.wait(&mut app, |app| {
        app.controller.windows_fn_state_unknown && !app.controller.windows_fn_activity()
    });
    assert!(!app.controller.windows_fn_lifetime_clean());
    show_label(&mut harness, &mut app, "上次检查的请求状态：未确认");
    assert!(app.controller.probe_windows_fn().is_err());
    assert!(app.controller.preview_windows_fn().is_err());
    assert!(app.controller.apply_windows_fn_preview().is_err());
    for _ in 0..4 {
        harness.frame(&mut app);
    }
    assert_eq!(
        calls.finish.load(Ordering::SeqCst),
        0,
        "painting must not start a hidden child wait or new helper"
    );
    click_button(&mut harness, &mut app, "继续等待辅助进程");
    harness.wait(&mut app, |app| {
        app.controller.windows_fn_lifetime_clean() && !app.controller.windows_fn_activity()
    });
    assert!(app.controller.windows_fn_state_unknown);
    assert!(app.controller.windows_fn_needs_probe);
    assert_eq!(calls.finish.load(Ordering::SeqCst), 1);
    assert_eq!(calls.probe.load(Ordering::SeqCst), 1);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(
        app.controller
            .windows_fn_receipt
            .as_ref()
            .unwrap()
            .primary_error
            .as_ref()
            .unwrap()
            .code,
        "fixture_fn_apply_unknown"
    );
    assert!(app.controller.preview_windows_fn().is_err());
    click_button(&mut harness, &mut app, "检查 Windows Fn 锁（需授权）");
    harness.wait(&mut app, |app| {
        !app.controller.windows_fn_needs_probe && !app.controller.busy()
    });
    assert_eq!(calls.probe.load(Ordering::SeqCst), 2);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}

#[test]
fn cached_fn_receipts_remain_visible_on_another_platform_but_do_not_authorize_a_write() {
    let (mut app, calls) = fixture("linux", vec![], vec![], vec![]);
    app.controller.windows_fn_status = Some(status_fixture(false));
    app.controller.windows_fn_outcome = Some(Outcome::FailedStateUnknown);
    let update = unknown(false);
    app.controller.windows_fn_receipt = Some(update.receipt);
    app.controller.windows_fn_error = update.error;
    app.controller.windows_fn_state_unknown = true;
    app.controller.windows_fn_requested = Some(true);
    let mut harness = Harness::new([640, 480], 1.0);
    harness.settle(&mut app);
    assert!(app.controller.windows_fn_visible());
    show_label(&mut harness, &mut app, "上次检查的请求状态：关闭");
    show_label(
        &mut harness,
        &mut app,
        "请求状态未确认；重新授权检查成功前不能预览或应用更改。",
    );
    assert!(app.controller.preview_windows_fn().is_err());
    open_details(&mut harness, &mut app);
    let output = show_label(&mut harness, &mut app, "fixture_fn_apply_unknown");
    assert!(text_contains(&output, "fixture_fn_apply_unknown"));
    app.select_control_section(ControlSection::DeviceFunctions);
    harness.settle(&mut app);
    click_button(&mut harness, &mut app, "Windows Fn 锁");
    harness.settle(&mut app);
    assert_no_jobs(&calls);
}

#[test]
fn empty_unsupported_unknown_and_expanded_fn_evidence_fit_small_windows_and_dpi() {
    for dpi in [1.0, 2.0] {
        for case in [
            "empty",
            "unsupported",
            "unknown",
            "long_error",
            "cached_receipt",
            "preview",
        ] {
            let (mut app, calls) = fixture("windows", vec![], vec![], vec![]);
            if case == "preview" {
                app.controller.windows_fn_status = Some(status_fixture(false));
                app.controller.windows_fn_outcome = Some(Outcome::Observed);
                app.controller.windows_fn_needs_probe = false;
                app.controller.set_windows_fn_target(true);
                app.controller.preview_windows_fn().unwrap();
            } else if case == "unsupported" {
                let update = unsupported();
                app.controller.windows_fn_status = update.status;
                app.controller.windows_fn_error = update.error;
            } else if case != "empty" {
                let mut update = unknown(false);
                if case == "long_error" {
                    let long = "UnbrokenWindowsFnReadbackDiagnostic0123456789".repeat(8);
                    update.receipt.readback_errors[0].details =
                        vec![format!("fixture://{long}"), "Fn锁回读详情结束。".into()];
                }
                app.controller.windows_fn_status = update.status;
                app.controller.windows_fn_receipt = Some(update.receipt);
                app.controller.windows_fn_error = update.error;
                app.controller.windows_fn_outcome = Some(update.outcome);
                app.controller.windows_fn_state_unknown = true;
                if case == "cached_receipt" {
                    app.controller.snapshot.as_mut().unwrap().platform = "linux".into();
                }
            }
            let logical = if matches!(case, "long_error" | "preview") {
                [360, 280]
            } else {
                [640, 480]
            };
            let mut harness = Harness::new(logical, dpi);
            if case != "empty" {
                open_details(&mut harness, &mut app);
                if case == "long_error" {
                    show_label(&mut harness, &mut app, "Fn锁回读详情结束。");
                }
            }
            assert_complete_layout(case, &mut harness, &mut app);
            let output = reveal(
                &mut harness,
                &mut app,
                "预览 Fn 锁更改",
                egui::accesskit::Role::Button,
            );
            assert!(
                text_equals(&output, "预览 Fn 锁更改"),
                "last-page action must be painted after actual scrolling"
            );
            if case == "preview" {
                let output = reveal(
                    &mut harness,
                    &mut app,
                    "应用 Fn 锁（需授权）",
                    egui::accesskit::Role::Button,
                );
                assert!(text_equals(&output, "应用 Fn 锁（需授权）"));
                let output = reveal(
                    &mut harness,
                    &mut app,
                    "取消 Fn 锁预览",
                    egui::accesskit::Role::Button,
                );
                assert!(text_equals(&output, "取消 Fn 锁预览"));
            }
            assert_no_jobs(&calls);
        }
    }
}

#[test]
#[cfg(not(target_os = "windows"))]
fn non_windows_without_a_cached_fn_result_does_not_expose_the_windows_entry() {
    let (mut app, calls) = fixture("linux", vec![], vec![], vec![]);
    app.select_control_section(ControlSection::DeviceFunctions);
    let mut harness = Harness::new([640, 480], 1.0);
    let (_, output) = harness.settle(&mut app);
    assert!(!app.controller.windows_fn_visible());
    assert!(!output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .any(|(_, node)| node.role() == egui::accesskit::Role::Button
            && node_named(&output, node, "Windows Fn 锁")));
    assert_no_jobs(&calls);
}

#[test]
fn automatic_windows_refresh_never_starts_fn_authorization_or_reuses_a_cached_plan() {
    let (mut app, calls) = fixture("windows", vec![], vec![], vec![]);
    app.preferences.auto_refresh = true;
    let due = Instant::now() - Duration::from_secs(4);
    app.controller.last_refresh = Some(due);
    app.controller.thermal_last_attempt = Some(due);
    app.controller.thermal_last_finished = Some(due);
    let mut harness = Harness::new([640, 480], 1.5);
    harness.wait(&mut app, |app| {
        calls.snapshot.load(Ordering::SeqCst) > 0
            && calls.catalog.load(Ordering::SeqCst) > 0
            && !app.controller.busy()
    });
    for _ in 0..4 {
        harness.frame(&mut app);
    }
    assert!(calls.snapshot.load(Ordering::SeqCst) > 0);
    assert!(calls.catalog.load(Ordering::SeqCst) > 0);
    assert_eq!(calls.probe.load(Ordering::SeqCst), 0);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 0);
    assert_eq!(calls.finish.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    assert!(app.controller.windows_fn_status.is_none());
    assert!(app.controller.windows_fn_preview.is_none());
    assert!(app.controller.windows_fn_outcome.is_none());
    assert!(app.controller.windows_fn_receipt.is_none());
}

#[test]
fn an_unreaped_success_shaped_reply_never_claims_confirmed_success() {
    let mut reply = applied();
    reply.helper_reaped = false;
    reply.helper_lifetime = ClientHelperLifetime::Unknown;
    reply.receipt.writer_may_continue = true;
    let (mut app, calls) = fixture("windows", vec![observed(false)], vec![reply], vec![]);
    let mut harness = Harness::new([640, 480], 1.0);
    prepare_preview(&mut harness, &mut app, &calls);
    click_button(&mut harness, &mut app, "应用 Fn 锁（需授权）");
    harness.wait(&mut app, |app| {
        app.controller.windows_fn_outcome == Some(Outcome::AppliedVerified)
            && !app.controller.windows_fn_activity()
    });
    assert!(app.controller.windows_fn_state_unknown);
    assert!(!app.controller.windows_fn_lifetime_clean());
    assert!(
        app.controller.windows_fn_result.is_some(),
        "retain the raw result without presenting it as completed"
    );
    let output = reveal(
        &mut harness,
        &mut app,
        "Windows Fn 锁详情",
        egui::accesskit::Role::DisclosureTriangle,
    );
    assert!(!text_contains(&output, "Fn 锁设置已回读确认。"));
    assert!(
        !output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .any(|(_, node)| node_named(&output, node, "Fn 锁设置已回读确认。")),
        "a success label must not exist offscreen either"
    );
    assert_eq!(calls.probe.load(Ordering::SeqCst), 1);
    assert_eq!(calls.apply.load(Ordering::SeqCst), 1);
    assert_eq!(calls.finish.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
}
