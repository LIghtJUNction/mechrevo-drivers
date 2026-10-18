#![cfg(feature = "gui")]
//! Render CPU power controls with local protocol fixtures. Startup, painting,
//! editing and previewing must never launch a privileged process.
use eframe::egui::{self, pos2, vec2, Context, FullOutput, RawInput, Rect};
use egui_system_font_provider::SystemFontProvider;
use mechrevo_console_core::{
    cpu_power::{Action, CpuPowerUpdate, Outcome, Plan, Policy, Status},
    fan_curve::{
        client::FanClientUpdate, FanCurve, FanCurveChannel, FanCurveLayout, ValidatedFanCurve,
    },
    ApplyResult, BrightnessApplyResult, BrightnessPlan, CatalogModel, ControlError, OemApplyResult,
    OemPlan, OemValue, PowerProfile, ProfilePlan, Snapshot, ThermalSample,
};
use mechrevo_console_gui::{
    desktop::{ControlSection, DesktopApp, RenderMetrics, Section},
    Backend, Controller, Preferences, Theme,
};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Calls {
    probe: AtomicUsize,
    execute: AtomicUsize,
    other: AtomicUsize,
    plans: Mutex<Vec<Plan>>,
}
struct FixtureBackend {
    snapshot: Snapshot,
    calls: Arc<Calls>,
    probe_reply: Option<CpuPowerUpdate>,
    execute_reply: Option<CpuPowerUpdate>,
}
impl FixtureBackend {
    fn denied<T>(&self) -> Result<T, ControlError> {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::new("fixture_only", "No hardware backend"))
    }
    fn denied_fan(&self) -> FanClientUpdate {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        FanClientUpdate {
            error: Some(ControlError::new("fixture_only", "No hardware backend")),
            ..Default::default()
        }
    }
}
impl Backend for FixtureBackend {
    fn cpu_power_probe(&mut self) -> CpuPowerUpdate {
        self.calls.probe.fetch_add(1, Ordering::SeqCst);
        self.probe_reply
            .take()
            .expect("unexpected CPU probe in renderer fixture")
    }
    fn cpu_power_execute(&mut self, plan: &Plan) -> CpuPowerUpdate {
        self.calls.execute.fetch_add(1, Ordering::SeqCst);
        self.calls.plans.lock().unwrap().push(plan.clone());
        self.execute_reply
            .take()
            .expect("unexpected CPU execute in renderer fixture")
    }
    fn snapshot(&mut self) -> Snapshot {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        self.snapshot.clone()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
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
    fn fan_probe(&mut self) -> FanClientUpdate {
        self.denied_fan()
    }
    fn fan_refresh(&mut self) -> FanClientUpdate {
        self.denied_fan()
    }
    fn fan_apply(&mut self, _: &ValidatedFanCurve) -> FanClientUpdate {
        self.denied_fan()
    }
    fn fan_restore(&mut self) -> FanClientUpdate {
        self.denied_fan()
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        self.denied_fan()
    }
    fn fan_poll(&mut self) -> Option<FanClientUpdate> {
        self.calls.other.fetch_add(1, Ordering::SeqCst);
        None
    }
}

fn snapshot_fixture() -> Snapshot {
    serde_json::from_value(json!({
        "platform":"fixture-only / 非真实硬件",
        "identity":{
            "manufacturer":"TEST FIXTURE", "product_name":"CPU策略界面测试设备",
            "product_version":null, "board_name":"NO-HARDWARE-BINDING", "bios_version":"TEST.0",
            "mechrevo_brand_detected":false, "support_note":"仅本地协议与布局测试，不建立硬件资格。"
        },
        "telemetry":[], "brightness":[], "oem_controls":[], "capabilities":[], "diagnostics":[],
        "power_policy":{"scope":"fixture-only", "current":null, "options":[], "source":null}
    }))
    .unwrap()
}

fn policy(cap_watts: u64, native_options: bool) -> Policy {
    Policy {
        cap_uw: cap_watts * 1_000_000,
        target_celsius: (!native_options).then_some(90),
        polling_seconds: (!native_options).then_some(1),
        rapl_debounce_seconds: (!native_options).then_some(0),
    }
}

fn status_fixture(case: &str) -> Status {
    let managed = case == "managed";
    let pending = case == "recovery";
    let unknown = case == "unknown" || case == "long_error";
    let partial = case == "partial";
    serde_json::from_value(json!({
        "management":if managed { "managed" } else if pending { "recovery_pending" } else if unknown { "unknown" } else { "native" },
        "revision":if unknown { None } else { Some("a".repeat(64)) },
        "qualified":!unknown && !partial,
        "allowed_actions":if unknown || partial { vec![] } else if pending { vec!["recover"] } else if managed { vec!["set","clear","reapply"] } else { vec!["set"] },
        "current_policy":if managed { Some(serde_json::to_value(policy(55,false)).unwrap()) } else { None },
        "operational_ceiling_uw":if unknown { None } else { Some(140_000_000_u64) },
        "minimum_limit_uw":35_000_000, "thermal_spec_reference_uw":55_000_000,
        "observed":[
            {"interface":"msr", "pl1_uw":if unknown { None } else { Some(if managed {55_000_000_u64} else {140_000_000_u64}) }},
            {"interface":"mmio", "pl1_uw":if unknown || partial { None } else { Some(if managed {55_000_000_u64} else {140_000_000_u64}) }}
        ],
        "thermald_active":if unknown { None } else { Some(true) },
        "holding_now":managed,
        "blocking_errors":if unknown || partial { vec!["TEST FIXTURE: qualification is incomplete"] } else { vec![] },
        "diagnostic_errors":[]
    })).expect("status fixture must deserialize through the real CPU protocol")
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

fn verified(mut status: Status) -> CpuPowerUpdate {
    status.revision = Some("b".repeat(64));
    CpuPowerUpdate {
        status: Some(status),
        outcome: Outcome::AppliedVerified,
        error: None,
        receipt: Some(json!({"fixture_only":true,"note":"本地 typed backend 回执，不是硬件验证"})),
        helper_reaped: true,
    }
}

fn fixture(
    case: &str,
    probe_reply: Option<CpuPowerUpdate>,
    execute_reply: Option<CpuPowerUpdate>,
) -> (DesktopApp, Arc<Calls>) {
    let snapshot = snapshot_fixture();
    let calls = Arc::new(Calls::default());
    let mut controller = Controller::new(FixtureBackend {
        snapshot: snapshot.clone(),
        calls: calls.clone(),
        probe_reply,
        execute_reply,
    })
    .unwrap();
    controller.snapshot = Some(snapshot);
    if case != "unprobed" {
        controller.cpu_power_status = Some(status_fixture(case));
        controller.cpu_power_outcome = Some(Outcome::Observed);
        controller.cpu_power_received_at = Some(std::time::SystemTime::now());
        controller.cpu_power_response_finished = Some(Instant::now());
        controller.cpu_power_state_unknown = matches!(case, "unknown" | "long_error");
        controller.cpu_power_needs_probe = !matches!(case, "native" | "managed" | "recovery");
        if matches!(case, "native" | "managed") {
            controller.set_cpu_power_draft(policy(
                if case == "managed" { 55 } else { 140 },
                case != "managed",
            ));
        }
    }
    if matches!(case, "long_error" | "partial") {
        let path = "UnbrokenReadOnlyPowerDiagnostic0123456789".repeat(8);
        let mut error = ControlError::new(
            "fixture_partial_cpu_status",
            format!("CPU 状态仅部分可用：fixture://{path}"),
        );
        error.details = vec![
            format!("保留主错误与回读详情：fixture://{path}"),
            "CPU错误详情结束。".into(),
        ];
        controller.cpu_power_error = Some(error);
    }
    let mut app = DesktopApp::with_controller(
        controller,
        Preferences {
            auto_refresh: false,
            ..Default::default()
        },
    );
    app.select_section(Section::Controls);
    app.select_control_section(ControlSection::CpuPower);
    (app, calls)
}

fn assert_no_jobs(calls: &Calls) {
    assert_eq!(calls.probe.load(Ordering::SeqCst), 0);
    assert_eq!(calls.execute.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
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
        panic!("keyboard traversal did not reach the requested CPU policy control");
    }
    fn wait(&mut self, app: &mut DesktopApp, done: impl Fn(&DesktopApp) -> bool) -> FullOutput {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let output = self.frame(app).1;
            if done(app) {
                return output;
            }
            assert!(Instant::now() < deadline, "fixture CPU job did not finish");
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
    panic!("CPU control '{label}' remained clipped after bounded scrolling");
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
        "{label}: CPU page bottom cannot be reached"
    );
}

#[test]
fn cpu_policy_states_and_expanded_errors_fit_the_minimum_window_at_every_dpi() {
    let mut verified = 0;
    for dpi in [1.0, 1.25, 1.5, 2.0] {
        for case in [
            "unprobed",
            "native",
            "managed",
            "recovery",
            "unknown",
            "long_error",
        ] {
            let (mut app, calls) = fixture(case, None, None);
            if dpi == 1.5 {
                app.preferences.theme = Theme::Light;
            }
            let mut harness = Harness::new([360, 280], dpi);
            if case == "long_error" {
                let output = reveal(
                    &mut harness,
                    &mut app,
                    "检查详情",
                    egui::accesskit::Role::DisclosureTriangle,
                );
                let (details, bounds) = control(
                    &output,
                    "检查详情",
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
                assert!(text_contains(&output, "CPU错误详情结束"));
            }
            assert_complete_layout(&format!("cpu-{case}-dpi{dpi}"), &mut harness, &mut app);
            assert_no_jobs(&calls);
            verified += 1;
        }
    }
    for case in ["native", "managed", "recovery"] {
        let (mut app, calls) = fixture(case, None, None);
        let mut harness = Harness::new([1120, 760], 1.0);
        assert_complete_layout(case, &mut harness, &mut app);
        assert_no_jobs(&calls);
        verified += 1;
    }
    assert_eq!(verified, 27);
}

#[test]
fn probe_is_an_explicit_button_job_and_unknown_readbacks_are_not_zero_watts() {
    let mut partial = observed(status_fixture("partial"));
    partial.error = Some(ControlError::new(
        "fixture_partial_probe",
        "MMIO 回读未确认，保留部分 CPU 状态。",
    ));
    let (mut app, calls) = fixture("unprobed", Some(partial), None);
    let mut harness = Harness::new([640, 480], 2.0);
    harness.settle(&mut app);
    assert_no_jobs(&calls);
    let output = reveal(
        &mut harness,
        &mut app,
        "检查 CPU 功耗策略（需授权）",
        egui::accesskit::Role::Button,
    );
    let (_, bounds) = control(
        &output,
        "检查 CPU 功耗策略（需授权）",
        egui::accesskit::Role::Button,
    );
    harness.click(&mut app, bounds.center());
    harness.wait(&mut app, |app| {
        app.controller.cpu_power_status.is_some() && !app.controller.busy()
    });
    // The completed probe adds status/diagnostic rows. Labels register their
    // accessibility metadata before egui culls offscreen painting, so scroll
    // the actual readout row into view before asserting its drawn text.
    reveal(
        &mut harness,
        &mut app,
        "MSR  140 W",
        egui::accesskit::Role::Label,
    );
    let (metrics, output) = harness.settle(&mut app);
    for expected in ["MSR  140 W", "MMIO  —"] {
        let (_, bounds) = control(&output, expected, egui::accesskit::Role::Label);
        assert!(
            metrics.viewport.expand(1.0).contains_rect(bounds),
            "both CPU readouts must be visible in the same frame: {expected}: {bounds:?}"
        );
        assert!(
            text_equals(&output, expected),
            "the visible CPU readout must be painted exactly: {expected}"
        );
    }
    assert_eq!(calls.probe.load(Ordering::SeqCst), 1);
    assert_eq!(calls.execute.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    assert!(text_contains(&output, "MSR") && text_contains(&output, "MMIO"));
    assert!(
        text_contains(&output, "—"),
        "missing MMIO telemetry must remain unknown"
    );
    assert!(!text_contains(&output, "MMIO 0 W") && !text_contains(&output, "MMIO：0 W"));
    assert!(app.controller.cpu_power_preview.is_none());
    assert_eq!(
        app.controller.cpu_power_error.as_ref().unwrap().code,
        "fixture_partial_probe"
    );
}

#[test]
fn keyboard_changes_one_whole_watt_and_local_preview_keeps_the_complete_policy() {
    let mut post = status_fixture("managed");
    post.current_policy = Some(policy(56, false));
    for reading in &mut post.observed {
        reading.pl1_uw = Some(56_000_000);
    }
    let (mut app, calls) = fixture("managed", None, Some(verified(post)));
    let mut harness = Harness::new([640, 480], 2.0);
    let output = reveal(
        &mut harness,
        &mut app,
        "长期上限（W）",
        egui::accesskit::Role::SpinButton,
    );
    let (watts, _) = control(&output, "长期上限（W）", egui::accesskit::Role::SpinButton);
    let (_, node) = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(id, _)| *id == watts)
        .unwrap();
    assert_eq!(node.min_numeric_value(), Some(35.0));
    assert_eq!(
        node.max_numeric_value(),
        Some(140.0),
        "the 55 W thermal reference is not the operational ceiling"
    );
    harness.focus(&mut app, watts);
    harness.key(&mut app, egui::Key::ArrowUp);
    harness.key(&mut app, egui::Key::Tab);
    let expected = policy(56, false);
    assert_eq!(app.controller.cpu_power_draft.as_ref(), Some(&expected));
    assert_no_jobs(&calls);
    let output = reveal(
        &mut harness,
        &mut app,
        "预览完整策略",
        egui::accesskit::Role::Button,
    );
    let (_, bounds) = control(&output, "预览完整策略", egui::accesskit::Role::Button);
    harness.click(&mut app, bounds.center());
    let plan = app
        .controller
        .cpu_power_preview
        .clone()
        .expect("a real local preview is required before authorization");
    assert_eq!(
        plan.action,
        Action::Set {
            policy: expected.clone()
        }
    );
    assert_no_jobs(&calls);
    let output = reveal(
        &mut harness,
        &mut app,
        "授权应用并回读",
        egui::accesskit::Role::Button,
    );
    let (_, bounds) = control(&output, "授权应用并回读", egui::accesskit::Role::Button);
    harness.click(&mut app, bounds.center());
    harness.wait(&mut app, |app| {
        app.controller.cpu_power_outcome == Some(Outcome::AppliedVerified) && !app.controller.busy()
    });
    assert_eq!(calls.execute.load(Ordering::SeqCst), 1);
    assert_eq!(calls.probe.load(Ordering::SeqCst), 0);
    assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    assert_eq!(calls.plans.lock().unwrap().as_slice(), &[plan]);
    assert_eq!(
        app.controller
            .cpu_power_status
            .as_ref()
            .unwrap()
            .current_policy
            .as_ref(),
        Some(&expected)
    );
}

#[test]
fn native_options_and_explicit_zero_are_distinct_and_survive_other_option_edits() {
    let (mut app, calls) = fixture("native", None, None);
    let mut harness = Harness::new([640, 480], 1.5);
    for (label, expected) in [
        (
            "90°C",
            Policy {
                target_celsius: Some(90),
                ..policy(140, true)
            },
        ),
        (
            "1 秒",
            Policy {
                target_celsius: Some(90),
                polling_seconds: Some(1),
                ..policy(140, true)
            },
        ),
        ("0 秒", policy(140, false)),
        (
            "原生递进",
            Policy {
                rapl_debounce_seconds: None,
                ..policy(140, false)
            },
        ),
        (
            "原生轮询",
            Policy {
                polling_seconds: None,
                rapl_debounce_seconds: None,
                ..policy(140, false)
            },
        ),
        ("原生温度策略", policy(140, true)),
    ] {
        let output = reveal(&mut harness, &mut app, label, egui::accesskit::Role::Button);
        let (_, bounds) = control(&output, label, egui::accesskit::Role::Button);
        harness.click(&mut app, bounds.center());
        assert_eq!(
            app.controller.cpu_power_draft.as_ref(),
            Some(&expected),
            "editing '{label}' erased another explicit policy option"
        );
        assert_no_jobs(&calls);
    }
}

#[test]
fn editing_a_parent_option_never_erases_dependent_policy_fields() {
    for (label, expected) in [
        (
            "原生温度策略",
            Policy {
                target_celsius: None,
                ..policy(55, false)
            },
        ),
        (
            "原生轮询",
            Policy {
                polling_seconds: None,
                ..policy(55, false)
            },
        ),
    ] {
        let (mut app, calls) = fixture("managed", None, None);
        let mut harness = Harness::new([640, 480], 1.0);
        let output = reveal(&mut harness, &mut app, label, egui::accesskit::Role::Button);
        let (_, bounds) = control(&output, label, egui::accesskit::Role::Button);
        harness.click(&mut app, bounds.center());
        assert_eq!(
            app.controller.cpu_power_draft.as_ref(),
            Some(&expected),
            "changing '{label}' silently erased an explicit dependent field"
        );
        let output = reveal(
            &mut harness,
            &mut app,
            "预览完整策略",
            egui::accesskit::Role::Button,
        );
        let (_, bounds) = control(&output, "预览完整策略", egui::accesskit::Role::Button);
        harness.click(&mut app, bounds.center());
        assert!(app.controller.cpu_power_preview.is_none());
        assert!(app
            .controller
            .preview_cpu_power(Action::Set { policy: expected })
            .is_err());
        assert!(
            app.controller.cpu_power_draft_error.is_some(),
            "an invalid dependency must be rejected at the local preview boundary"
        );
        assert_no_jobs(&calls);
    }
}

#[test]
fn clearing_and_recovery_each_require_a_local_preview_before_their_authorized_job() {
    for (case, label, action) in [
        ("managed", "预览撤销托管", Action::Clear),
        ("recovery", "预览恢复原生策略", Action::Recover),
    ] {
        let (mut app, calls) = fixture(case, None, Some(verified(status_fixture("native"))));
        let mut harness = Harness::new([640, 480], 1.0);
        let output = reveal(&mut harness, &mut app, label, egui::accesskit::Role::Button);
        let (_, bounds) = control(&output, label, egui::accesskit::Role::Button);
        harness.click(&mut app, bounds.center());
        assert_no_jobs(&calls);
        assert_eq!(
            app.controller.cpu_power_preview.as_ref().unwrap().action,
            action
        );
        let output = reveal(
            &mut harness,
            &mut app,
            "授权应用并回读",
            egui::accesskit::Role::Button,
        );
        let (_, bounds) = control(&output, "授权应用并回读", egui::accesskit::Role::Button);
        harness.click(&mut app, bounds.center());
        harness.wait(&mut app, |app| {
            app.controller.cpu_power_outcome == Some(Outcome::AppliedVerified)
                && !app.controller.busy()
        });
        assert_eq!(calls.execute.load(Ordering::SeqCst), 1);
        assert_eq!(calls.plans.lock().unwrap()[0].action, action);
        assert_eq!(calls.other.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn editing_or_previewing_cpu_policy_does_not_discard_a_pending_fan_preview() {
    let (mut app, calls) = fixture("managed", None, None);
    let mut harness = Harness::new([640, 480], 1.0);
    let output = reveal(
        &mut harness,
        &mut app,
        "长期上限（W）",
        egui::accesskit::Role::SpinButton,
    );
    let (watts, _) = control(&output, "长期上限（W）", egui::accesskit::Role::SpinButton);
    harness.focus(&mut app, watts);
    harness.key(&mut app, egui::Key::ArrowUp);
    harness.key(&mut app, egui::Key::Tab);
    assert_eq!(
        app.controller.cpu_power_draft.as_ref(),
        Some(&policy(56, false))
    );
    assert_no_jobs(&calls);
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
    let foreign = app.controller.fan_preview.clone();
    assert!(app.controller.cpu_power_preview.is_none());
    let output = reveal(
        &mut harness,
        &mut app,
        "预览完整策略",
        egui::accesskit::Role::Button,
    );
    let (_, bounds) = control(&output, "预览完整策略", egui::accesskit::Role::Button);
    harness.click(&mut app, bounds.center());
    assert_eq!(app.controller.fan_preview, foreign);
    assert!(
        app.controller.cpu_power_preview.is_none(),
        "a foreign preview must block a second CPU confirmation"
    );
    assert_no_jobs(&calls);
}

#[test]
fn an_old_cpu_preview_cannot_offer_authorization_after_status_becomes_unknown() {
    let (mut app, calls) = fixture("managed", None, None);
    app.controller
        .preview_cpu_power(Action::Set {
            policy: policy(55, false),
        })
        .unwrap();
    app.controller.cpu_power_status = Some(status_fixture("unknown"));
    app.controller.cpu_power_state_unknown = true;
    app.controller.cpu_power_needs_probe = true;
    let mut harness = Harness::new([640, 480], 1.5);
    let output = reveal(
        &mut harness,
        &mut app,
        "授权应用并回读",
        egui::accesskit::Role::Button,
    );
    let (apply, bounds) = control(&output, "授权应用并回读", egui::accesskit::Role::Button);
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
        "an old preview must not make unknown CPU state writable"
    );
    harness.click(&mut app, bounds.center());
    assert_no_jobs(&calls);
}
