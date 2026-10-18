#![cfg(feature = "gui")]
//! Draw the production fan page with typed, local-only fixtures. No test backend
//! accesses a device, and no fixture is reachable from application startup.
use eframe::egui::{self, pos2, vec2, Context, FullOutput, RawInput, Rect};
use egui_system_font_provider::SystemFontProvider;
use mechrevo_console_core::{
    fan_curve::{
        broker::{
            validate_response, BrokerOutcome, BrokerPhase, BrokerResponse, BrokerState, BACKEND,
            DEVICE_PATH, PROTOCOL_VERSION,
        },
        client::FanClientUpdate,
        FanCurve, FanCurveChannel, FanCurveLayout, RawFanTable, ValidatedFanCurve,
    },
    ApplyResult, BrightnessApplyResult, BrightnessPlan, CatalogModel, ControlError, OemApplyResult,
    OemPlan, OemValue, PowerProfile, ProfilePlan, Snapshot,
};
use mechrevo_console_gui::{
    desktop::{DesktopApp, RenderMetrics, Section},
    fan_editor::FanChannel,
    Backend, Controller, Preferences, Theme,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

/// Rendering and draft editing must not submit even a read-only backend job.
/// Counting also catches a background worker panic which the controller would
/// otherwise turn into an error instead of failing this integration test.
struct FixtureBackend {
    snapshot: Snapshot,
    calls: Arc<AtomicUsize>,
}
impl Backend for FixtureBackend {
    fn snapshot(&mut self) -> Snapshot {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.snapshot.clone()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        vec![]
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn plan_brightness(&mut self, _: u8, _: &str) -> Result<BrightnessPlan, ControlError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn apply_brightness(
        &mut self,
        _: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn plan_oem(&mut self, _: &str, _: OemValue) -> Result<OemPlan, ControlError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn apply_oem(&mut self, _: &OemPlan) -> Result<OemApplyResult, ControlError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn fan_probe(&mut self) -> FanClientUpdate {
        self.calls.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate::default()
    }
    fn fan_refresh(&mut self) -> FanClientUpdate {
        self.calls.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate::default()
    }
    fn fan_apply(&mut self, _: &ValidatedFanCurve) -> FanClientUpdate {
        self.calls.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate::default()
    }
    fn fan_restore(&mut self) -> FanClientUpdate {
        self.calls.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate::default()
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        self.calls.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate::default()
    }
}

fn app_fixture() -> (DesktopApp, Arc<AtomicUsize>) {
    let snapshot: Snapshot = serde_json::from_value(json!({
        "platform":"fixture-only / 非真实硬件",
        "identity":{
            "manufacturer":"TEST FIXTURE", "product_name":"风扇界面测试设备",
            "product_version":null, "board_name":"NOT-A-HARDWARE-BINDING",
            "bios_version":"TEST.0.0", "mechrevo_brand_detected":false,
            "support_note":"此数据只用于布局，不建立型号或散热能力核验。"
        },
        "telemetry":[], "brightness":[], "oem_controls":[],
        "capabilities":[], "diagnostics":[],
        "power_policy":{"scope":"fixture-only", "current":null, "options":[], "source":null}
    }))
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut controller = Controller::new(FixtureBackend {
        snapshot: snapshot.clone(),
        calls: calls.clone(),
    })
    .unwrap();
    controller.snapshot = Some(snapshot);
    let preferences = Preferences {
        auto_refresh: false,
        ..Default::default()
    };
    let mut app = DesktopApp::with_controller(controller, preferences);
    app.select_section(Section::Fans);
    (app, calls)
}

/// Deliberately synthetic and distinct channels, never a default for a model.
fn curve_fixture() -> FanCurve {
    let channel = |base: u8| FanCurveChannel {
        rise_celsius: std::array::from_fn(|stage| 40 + stage as u8 * 2),
        fall_celsius: std::array::from_fn(|stage| 38 + stage as u8 * 2),
        duty_raw: std::array::from_fn(|stage| base + stage as u8 * 4),
    };
    FanCurve {
        schema_version: 1,
        layout: FanCurveLayout::UniwillRamFan1p5,
        respective: true,
        cpu: channel(70),
        gpu: channel(90),
    }
}

fn import_fixture(app: &mut DesktopApp) -> FanCurve {
    let curve = curve_fixture();
    app.fan_editor_mut()
        .load_json(&serde_json::to_vec(&curve).unwrap())
        .unwrap();
    assert_eq!(app.fan_editor().curve(), Some(&curve));
    curve
}

fn session_fixture(phase: BrokerPhase) -> BrokerState {
    let curve = curve_fixture().validate().unwrap();
    let active = phase == BrokerPhase::Active;
    let table = RawFanTable::from_bytes([129; 96]);
    BrokerState {
        backend: BACKEND.into(),
        device_path: DEVICE_PATH.into(),
        epoch: 41,
        generation: 3,
        lease_id: 17,
        last_request_id: 2,
        phase,
        capabilities: 7,
        owner: true,
        revoked: false,
        modified_or_uncertain: phase != BrokerPhase::Acquired,
        state_is_current: true,
        restored_verified: false,
        live_error: 0,
        last_error: if phase == BrokerPhase::RecoveryRequired {
            -5
        } else {
            0
        },
        restore_error: if phase == BrokerPhase::RecoveryRequired {
            -5
        } else {
            0
        },
        live_table: Some(if active {
            table.with_curve(&curve).as_bytes().to_vec()
        } else {
            vec![127; 96]
        }),
        original_table: Some(vec![129; 96]),
        live_controls: Some(if active {
            [0x80, 0x04, 0, 0, 0, 0]
        } else {
            [0; 6]
        }),
        original_controls: Some([0; 6]),
        cached_original_targets: Some([0; 4]),
        applied_curve: active.then(|| curve.curve().clone()),
    }
}

fn imported_state_fixture(app: &mut DesktopApp) -> ValidatedFanCurve {
    let curve = import_fixture(app).validate().unwrap();
    // State fixtures represent the moment after an earlier import has already
    // been published. Do not let that setup event clear a later recovery error
    // on the first render. Interaction tests retain the real pending update.
    let _ = app.fan_editor_mut().take_update();
    app.controller.fan_draft = Some(curve.clone());
    curve
}

fn case_fixture(case: &str) -> (DesktopApp, Arc<AtomicUsize>) {
    let (mut app, calls) = app_fixture();
    match case {
        "empty" => app.controller.snapshot = None,
        "unavailable" => {
            app.controller.fan_error = Some(ControlError::new(
                "fan_backend_unsupported",
                "该测试后端没有风扇控制接口，未启动任何硬件会话。",
            ));
        }
        "table_only" => {
            let mut state = session_fixture(BrokerPhase::Idle);
            state.owner = false;
            state.modified_or_uncertain = false;
            state.lease_id = 0;
            state.original_table = None;
            state.original_controls = None;
            state.cached_original_targets = None;
            app.controller.fan_state = Some(state);
        }
        "draft" => {
            imported_state_fixture(&mut app);
        }
        "leased" | "recovery" | "orphan_recovery" | "stale_metadata" | "live_read_error" => {
            let curve = imported_state_fixture(&mut app);
            let recovery = matches!(case, "recovery" | "orphan_recovery");
            let stale = matches!(case, "stale_metadata" | "live_read_error");
            let phase = if recovery {
                BrokerPhase::RecoveryRequired
            } else {
                BrokerPhase::Active
            };
            let mut state = session_fixture(phase);
            if case == "orphan_recovery" {
                state.owner = false;
                state.lease_id = 0;
            }
            if stale {
                state.applied_curve = None;
                if case == "stale_metadata" {
                    state.state_is_current = false;
                } else {
                    state.live_error = -5;
                    state.live_table = None;
                    state.live_controls = None;
                }
            }
            app.controller.fan_state = Some(state);
            app.controller.fan_session_held = case != "orphan_recovery";
            app.controller.fan_helper_reaped = case == "orphan_recovery";
            app.controller.fan_cleanup_complete = false;
            app.controller.fan_restoration_confirmed = false;
            if !stale && case != "orphan_recovery" {
                app.controller.fan_applied = Some(curve);
            }
            if recovery {
                app.controller.fan_error = Some(ControlError::new(
                    "fixture_restore_failed",
                    "恢复回读未通过，保留原始恢复信息与会话。",
                ));
            }
        }
        "long_error" => {
            imported_state_fixture(&mut app);
            let path = "UnbrokenDriverOrDeviceIdentity0123456789".repeat(9);
            let mut error = ControlError::new(
                "fixture_long_error",
                format!("诊断起点：接口暂不可用，原始路径 fixture://{path}；诊断终点。"),
            );
            error.details = vec![
                format!("保留错误证据：fixture://{path}"),
                "此数据仅测试长错误换行，未进行硬件操作。".into(),
            ];
            app.controller.fan_error = Some(error);
        }
        _ => panic!("unknown fan fixture '{case}'"),
    }
    if let Some(state) = &app.controller.fan_state {
        validate_response(&BrokerResponse {
            schema_version: PROTOCOL_VERSION,
            request_id: 1,
            session_id: "00112233445566778899aabbccddeeff".into(),
            outcome: BrokerOutcome::Inspected,
            state: Some(state.clone()),
            error: None,
            recovery_error: None,
            terminal: false,
            restoration_confirmed: false,
            no_acquisition_attempted: false,
            close_fallback_unconfirmed: false,
        })
        .expect("layout fixtures must also satisfy the real broker response contract");
    }
    (app, calls)
}

struct Harness {
    ctx: Context,
    physical: [u32; 2],
    dpi: f32,
    frame: u32,
}
impl Harness {
    fn new(logical: [u32; 2], dpi: f32, fonts: Arc<SystemFontProvider>) -> Self {
        let ctx = Context::default();
        ctx.add_font_provider(fonts);
        ctx.enable_accesskit();
        ctx.set_missing_glyph_policy(egui::epaint::text::MissingGlyphPolicy::Panic);
        Self {
            ctx,
            physical: logical.map(|side| (side as f32 * dpi).round() as u32),
            dpi,
            frame: 0,
        }
    }
    fn frame(&mut self, app: &mut DesktopApp) -> (RenderMetrics, FullOutput) {
        self.events(app, vec![])
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
        // There is deliberately no renderer. Acknowledge both texture updates
        // and releases instead of leaving an unconsumed delta in FullOutput.
        output.textures_delta.set.clear();
        output.textures_delta.free.clear();
        (metrics.unwrap(), output)
    }
    fn settle(&mut self, app: &mut DesktopApp) -> (RenderMetrics, FullOutput) {
        for _ in 0..3 {
            self.frame(app);
        }
        self.frame(app)
    }
    fn click(&mut self, app: &mut DesktopApp, position: egui::Pos2) -> FullOutput {
        self.events(
            app,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        self.events(
            app,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
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

    fn focus(&mut self, app: &mut DesktopApp, target: egui::accesskit::NodeId) -> FullOutput {
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
                return output;
            }
        }
        panic!("keyboard traversal did not reach the requested fan control");
    }
}

fn control(
    output: &FullOutput,
    label: &str,
    role: egui::accesskit::Role,
) -> (egui::accesskit::NodeId, Rect) {
    let update = output.platform_output.accesskit_update.as_ref().unwrap();
    let (id, node) = update
        .nodes
        .iter()
        .find(|(_, node)| node.role() == role && node_named(output, node, label))
        .unwrap_or_else(|| panic!("Accessible {role:?} '{label}' is missing"));
    let bounds = node.bounds().expect("interactive control needs bounds");
    (
        *id,
        Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        ),
    )
}

fn node_named(output: &FullOutput, node: &egui::accesskit::Node, name: &str) -> bool {
    if node.label() == Some(name) {
        return true;
    }
    // A visible label is correctly represented by labelled_by, rather than a
    // second hidden label on the control. egui flattens these naming relations.
    let nodes = &output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes;
    node.labelled_by().iter().any(|label| {
        nodes.iter().any(|(id, node)| {
            id == label && (node.label() == Some(name) || node.value() == Some(name))
        })
    })
}

fn contains_text(output: &FullOutput, expected: &str) -> bool {
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

fn has_control(output: &FullOutput, label: &str, role: egui::accesskit::Role) -> bool {
    output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .any(|(_, node)| node.role() == role && node_named(output, node, label))
}

fn assert_bounds(label: &str, harness: &Harness, metrics: &RenderMetrics, output: &FullOutput) {
    assert!(
        metrics.content_size.is_finite(),
        "{label}: invalid content size"
    );
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
    assert!(
        (output.pixels_per_point - harness.dpi * app.preferences.text_scale).abs() < 0.001,
        "{label}: DPI and text zoom were conflated"
    );
    assert_bounds(label, harness, &metrics, &output);
    assert!(
        contains_text(&output, "风扇曲线"),
        "{label}: missing page heading"
    );
    app.scroll_to_bottom();
    let (metrics, output) = harness.settle(app);
    assert_bounds(label, harness, &metrics, &output);
    assert!(
        metrics.bottom_marker.top() >= metrics.viewport.top() - 1.0
            && metrics.bottom_marker.bottom() <= metrics.viewport.bottom() + 1.0,
        "{label}: last action/marker cannot be reached: {:?} outside {:?}",
        metrics.bottom_marker,
        metrics.viewport
    );
}

fn reveal(
    harness: &mut Harness,
    app: &mut DesktopApp,
    label: &str,
    role: egui::accesskit::Role,
) -> FullOutput {
    let (mut metrics, mut output) = harness.settle(app);
    for _ in 0..12 {
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
    panic!("fan control '{label}' remained clipped after bounded scrolling");
}

#[test]
fn fan_page_states_wrap_and_keep_the_final_action_reachable_at_every_dpi() {
    let fonts = Arc::new(SystemFontProvider::new().with_locale(Some("zh-Hans")));
    let mut rendered = 0;
    for dpi in [1.0, 1.25, 1.5, 2.0] {
        for logical in [[640, 480], [1120, 760]] {
            for case in [
                "empty",
                "unavailable",
                "draft",
                "leased",
                "recovery",
                "orphan_recovery",
                "long_error",
            ] {
                let (mut app, calls) = case_fixture(case);
                if dpi == 1.5 {
                    app.preferences.theme = Theme::Light;
                }
                let label = format!("fan-{case}-{}x{}-dpi{dpi}", logical[0], logical[1]);
                let mut harness = Harness::new(logical, dpi, fonts.clone());
                assert_complete_layout(&label, &mut harness, &mut app);
                if matches!(case, "leased" | "recovery" | "orphan_recovery") {
                    // Reaching the footer alone does not prove that the real
                    // restoration action is visible or operable.
                    let output = reveal(
                        &mut harness,
                        &mut app,
                        "恢复旧表并释放",
                        egui::accesskit::Role::Button,
                    );
                    let (restore, _) =
                        control(&output, "恢复旧表并释放", egui::accesskit::Role::Button);
                    let (_, node) = output
                        .platform_output
                        .accesskit_update
                        .as_ref()
                        .unwrap()
                        .nodes
                        .iter()
                        .find(|(id, _)| *id == restore)
                        .unwrap();
                    assert!(!node.is_disabled(), "{label}: recovery must stay available");
                }
                assert!(
                    !app.controller.busy(),
                    "{label}: render submitted a request"
                );
                if case == "long_error" {
                    assert!(
                        app.controller.fan_error.as_ref().is_some_and(|error| {
                            error.code == "fixture_long_error" && error.message.contains("诊断终点")
                        }),
                        "the long-error fixture must remain an actual error during rendering"
                    );
                }
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    0,
                    "{label}: unexpected backend call"
                );
                rendered += 1;
            }
        }
    }
    assert_eq!(rendered, 56);

    for case in ["leased", "long_error"] {
        let (mut app, calls) = case_fixture(case);
        app.preferences.text_scale = 1.6;
        let mut harness = Harness::new([640, 480], 2.0, fonts.clone());
        assert_complete_layout(&format!("fan-large-text-{case}"), &mut harness, &mut app);
        assert!(!app.controller.busy());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    println!("Verified 58 production fan-page layouts: seven typed states including orphan recovery, two logical sizes, four native DPI scales, light theme, and two 160% text cases; no backend jobs or hardware access.");
}

#[test]
fn an_empty_editor_and_missing_backend_do_not_invent_curves_or_offer_application() {
    let fonts = Arc::new(SystemFontProvider::new().with_locale(Some("zh-Hans")));
    for case in ["empty", "unavailable", "table_only"] {
        let (mut app, calls) = case_fixture(case);
        let mut harness = Harness::new([1120, 760], 1.0, fonts.clone());
        let (_, output) = harness.settle(&mut app);
        assert!(contains_text(&output, "还没有曲线草稿"));
        assert!(app.fan_editor().curve().is_none());
        assert!(!has_control(&output, "CPU", egui::accesskit::Role::Button));
        assert!(!has_control(&output, "GPU", egui::accesskit::Role::Button));
        if case == "unavailable" {
            assert!(contains_text(&output, "该测试后端没有风扇控制接口"));
        }
        assert!(!output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .any(|(_, node)| {
                node.role() == egui::accesskit::Role::Button
                    && node_named(&output, node, "应用曲线")
                    && !node.is_disabled()
            }));
        assert!(!app.controller.busy());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        if case == "unavailable" {
            let curve = imported_state_fixture(&mut app);
            app.controller.fan_preview = Some(curve);
            let (_, output) = harness.settle(&mut app);
            let (apply, _) = control(&output, "应用曲线", egui::accesskit::Role::Button);
            let (_, node) = output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|(id, _)| *id == apply)
                .unwrap();
            assert!(
                node.is_disabled(),
                "a valid draft and preview cannot make a missing backend writable"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn stale_active_state_is_uncertain_and_keeps_the_recovery_action_available() {
    let fonts = Arc::new(SystemFontProvider::new().with_locale(Some("zh-Hans")));
    for case in ["stale_metadata", "live_read_error"] {
        for (logical, dpi) in [([640, 480], 2.0), ([1120, 760], 1.25)] {
            let (mut app, calls) = case_fixture(case);
            let mut harness = Harness::new(logical, dpi, fonts.clone());
            let (_, output) = harness.settle(&mut app);
            assert!(contains_text(&output, "当前风扇状态未确认，请刷新或恢复"));
            assert!(!contains_text(&output, "本会话曲线控制正在运行"));
            assert!(!contains_text(&output, "风扇接口已就绪"));
            assert_complete_layout(case, &mut harness, &mut app);
            let output = reveal(
                &mut harness,
                &mut app,
                "恢复旧表并释放",
                egui::accesskit::Role::Button,
            );
            let (restore, _) = control(&output, "恢复旧表并释放", egui::accesskit::Role::Button);
            let (_, node) = output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|(id, _)| *id == restore)
                .unwrap();
            assert!(!node.is_disabled());
            assert_eq!(
                app.controller.fan_state.as_ref().unwrap().phase,
                BrokerPhase::Active
            );
            assert!(!app.controller.busy());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn fan_navigation_accepts_pointer_and_keyboard_in_the_minimum_window() {
    let fonts = Arc::new(SystemFontProvider::new().with_locale(Some("zh-Hans")));
    for dpi in [1.0, 2.0] {
        for text_scale in [1.0, 1.6] {
            let (mut app, calls) = app_fixture();
            app.select_section(Section::Dashboard);
            app.preferences.text_scale = text_scale;
            let mut harness = Harness::new([640, 480], dpi, fonts.clone());
            let (_, output) = harness.settle(&mut app);
            let (_, fans) = control(&output, "风扇", egui::accesskit::Role::Button);
            let output = harness.click(&mut app, fans.center());
            assert!(contains_text(&output, "风扇曲线"));
            let (_, overview) = control(&output, "概览", egui::accesskit::Role::Button);
            let output = harness.click(&mut app, overview.center());
            let (fans, _) = control(&output, "风扇", egui::accesskit::Role::Button);
            harness.focus(&mut app, fans);
            let output = harness.key(&mut app, egui::Key::Enter);
            assert!(contains_text(&output, "风扇曲线"));
            let (_, node) = output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|(id, _)| *id == fans)
                .unwrap();
            assert_eq!(node.toggled(), Some(egui::accesskit::Toggled::True));
            assert!(!app.controller.busy());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn typed_json_import_is_local_and_failed_import_preserves_the_existing_draft() {
    let (mut app, calls) = app_fixture();
    assert!(app.fan_editor().curve().is_none());
    let original = import_fixture(&mut app);
    let mut invalid = serde_json::to_value(&original).unwrap();
    invalid["cpu"]["duty_raw"][3] = json!(201);
    assert!(app
        .fan_editor_mut()
        .load_json(&serde_json::to_vec(&invalid).unwrap())
        .is_err());
    assert_eq!(app.fan_editor().curve(), Some(&original));
    assert!(app.fan_editor_mut().load_json(b"{truncated").is_err());
    assert_eq!(app.fan_editor().curve(), Some(&original));
    app.controller.fan_preview = Some(original.validate().unwrap());
    let fonts = Arc::new(SystemFontProvider::new().with_locale(Some("zh-Hans")));
    let mut harness = Harness::new([1120, 760], 1.0, fonts.clone());
    harness.settle(&mut app);
    assert!(!app.fan_editor().is_valid());
    assert!(app.controller.fan_draft.is_none());
    assert!(app.controller.fan_preview.is_none());
    assert!(!app.controller.busy());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // A rejected first import must also clear any controller draft/preview
    // before the empty editor can adopt that older typed value.
    let (mut empty, empty_calls) = app_fixture();
    let old = curve_fixture().validate().unwrap();
    empty.controller.fan_draft = Some(old.clone());
    empty.controller.fan_preview = Some(old);
    assert!(empty.fan_editor().curve().is_none());
    assert!(empty.fan_editor_mut().load_json(b"{broken").is_err());
    let mut harness = Harness::new([1120, 760], 1.0, fonts);
    harness.settle(&mut empty);
    assert!(empty.fan_editor().curve().is_none());
    assert!(!empty.fan_editor().is_valid());
    assert!(empty.controller.fan_draft.is_none());
    assert!(empty.controller.fan_preview.is_none());
    assert!(!empty.controller.busy());
    assert_eq!(empty_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn pointer_and_keyboard_edit_the_selected_channel_and_stage_without_backend_jobs() {
    let fonts = Arc::new(SystemFontProvider::new().with_locale(Some("zh-Hans")));
    for (logical, dpi) in [([640, 480], 1.0), ([640, 480], 2.0), ([1120, 760], 1.5)] {
        let (mut app, calls) = app_fixture();
        let original = import_fixture(&mut app);
        let mut harness = Harness::new(logical, dpi, fonts.clone());
        let output = reveal(&mut harness, &mut app, "GPU", egui::accesskit::Role::Button);
        let (_, gpu) = control(&output, "GPU", egui::accesskit::Role::Button);
        harness.click(&mut app, gpu.center());
        assert_eq!(app.fan_editor().channel(), FanChannel::Gpu);

        let output = reveal(&mut harness, &mut app, "CPU", egui::accesskit::Role::Button);
        let (cpu, _) = control(&output, "CPU", egui::accesskit::Role::Button);
        harness.focus(&mut app, cpu);
        harness.key(&mut app, egui::Key::Enter);
        assert_eq!(app.fan_editor().channel(), FanChannel::Cpu);
        let output = reveal(&mut harness, &mut app, "GPU", egui::accesskit::Role::Button);
        let (gpu, _) = control(&output, "GPU", egui::accesskit::Role::Button);
        harness.focus(&mut app, gpu);
        harness.key(&mut app, egui::Key::Enter);
        assert_eq!(app.fan_editor().channel(), FanChannel::Gpu);

        let output = reveal(
            &mut harness,
            &mut app,
            "编辑档位",
            egui::accesskit::Role::Slider,
        );
        let (stage, bounds) = control(&output, "编辑档位", egui::accesskit::Role::Slider);
        harness.click(&mut app, pos2(bounds.right() - 1.0, bounds.center().y));
        assert_eq!(app.fan_editor().selected_stage(), 15);
        harness.focus(&mut app, stage);
        harness.key(&mut app, egui::Key::ArrowLeft);
        assert_eq!(app.fan_editor().selected_stage(), 14);
        let output = harness.key(&mut app, egui::Key::ArrowRight);
        assert_eq!(app.fan_editor().selected_stage(), 15);
        for boundary in ["升温温界", "降温温界"] {
            assert!(
                !has_control(&output, boundary, egui::accesskit::Role::SpinButton),
                "the sixteenth duty stage has no sixteenth temperature boundary"
            );
        }

        for _ in 0..12 {
            harness.key(&mut app, egui::Key::ArrowLeft);
        }
        assert_eq!(app.fan_editor().selected_stage(), 3);
        let output = reveal(
            &mut harness,
            &mut app,
            "占空比",
            egui::accesskit::Role::SpinButton,
        );
        let (duty, _) = control(&output, "占空比", egui::accesskit::Role::SpinButton);
        harness.focus(&mut app, duty);
        harness.key(&mut app, egui::Key::ArrowUp);
        harness.key(&mut app, egui::Key::Tab);
        let mut expected = original.clone();
        expected.gpu.duty_raw[3] += 1;
        assert_eq!(
            app.fan_editor().curve(),
            Some(&expected),
            "keyboard duty increment must change exactly half a percent in the selected GPU stage"
        );

        let output = reveal(&mut harness, &mut app, "CPU", egui::accesskit::Role::Button);
        let (_, cpu) = control(&output, "CPU", egui::accesskit::Role::Button);
        harness.click(&mut app, cpu.center());
        assert_eq!(app.fan_editor().channel(), FanChannel::Cpu);
        let output = reveal(
            &mut harness,
            &mut app,
            "升温温界",
            egui::accesskit::Role::SpinButton,
        );
        let (rise, _) = control(&output, "升温温界", egui::accesskit::Role::SpinButton);
        harness.focus(&mut app, rise);
        harness.key(&mut app, egui::Key::ArrowUp);
        harness.key(&mut app, egui::Key::Tab);
        let selected = app.fan_editor().selected_stage();
        assert!(
            selected < 15,
            "a temperature boundary must identify a real boundary"
        );
        expected.cpu.rise_celsius[selected] += 1;
        assert_eq!(app.fan_editor().curve(), Some(&expected));
        assert!(
            !app.controller.busy(),
            "editing must not enqueue a worker request"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
