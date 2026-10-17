#![cfg(feature = "gui")]
//! Exercise the actual desktop renderer without a window server or hardware access.
//! Optional PNG captures are generated from egui's own meshes and font atlas; fixtures
//! never enter the application startup path. Set MECHREVO_GUI_CAPTURE_DIR to capture.
//! MECHREVO_GUI_CAPTURE_MATCH optionally selects comma-separated filename substrings.
use eframe::egui::{self, pos2, vec2, ColorImage, Context, FullOutput, RawInput, Rect, TextureId};
use egui_system_font_provider::SystemFontProvider;
use mechrevo_console_core::{
    ApplyResult, CatalogModel, ControlError, PowerProfile, ProfilePlan, Snapshot,
};
use mechrevo_console_gui::{
    desktop::{DesktopApp, RenderMetrics, Section},
    Backend, Controller, DisplayMode, Preferences,
};
use serde_json::json;
use std::{collections::HashMap, path::Path, sync::Arc};

struct FixtureBackend(Snapshot);
impl Backend for FixtureBackend {
    fn snapshot(&mut self) -> Snapshot {
        self.0.clone()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        vec![]
    }
    fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
        panic!("Rendering must not request a hardware control preview")
    }
    fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        panic!("Rendering must not write hardware")
    }
}

fn fixture(case: &str) -> DesktopApp {
    let long = "VeryLongUnbrokenHardwareIdentifier0123456789".repeat(5);
    let source_path = if case == "multi_gpu" {
        format!("fixture://{long}/hwmon/{long}/temp1_input")
    } else {
        "fixture://只读测试".into()
    };
    let sensor = |id: String, label: String, device: String, value: Option<f64>| {
        json!({
            "id":id, "label":label, "device_id":device, "kind":"temperature", "value":value,
            "unit":"°C", "source":{"backend":"fixture-only", "path":source_path, "device_id":device},
            "status":if value.is_some() { "available" } else { "unavailable" },
            "reason":if value.is_some() { None } else { Some("当前设备没有提供温度读数；不会把未知数值显示成零。") }
        })
    };
    let mut telemetry = vec![sensor(
        "cpu_temp".into(),
        format!("CPU / 处理器 {long}"),
        "fixture-cpu".into(),
        Some(63.5),
    )];
    for gpu in 0..if case == "multi_gpu" { 8 } else { 2 } {
        telemetry.push(sensor(
            format!("gpu{gpu}_temp"),
            format!("GPU {gpu} / 显卡 {long}"),
            format!("fixture-gpu-{gpu}"),
            if case == "unavailable" {
                None
            } else {
                Some(48.0 + gpu as f64)
            },
        ));
    }
    let mut snapshot: Snapshot = serde_json::from_value(json!({
        "platform":"fixture-only / 非真实硬件", "identity":{
            "manufacturer":"机械革命 / TEST FIXTURE", "product_name":format!("旷世水冷 {long}"),
            "product_version":"测试型号", "board_name":format!("TEST-BOARD-{long}"),
            "bios_version":"TEST.0.0", "mechrevo_brand_detected":true,
            "support_note":"本数据只用于界面测试，检测到品牌不代表已经核验硬件控制能力。"
        },
        "telemetry":telemetry, "brightness":[],
        "capabilities":[{
            "id":"power_profile", "label":"系统电源策略", "status":if case == "unavailable" { "unavailable" } else { "available" },
            "reason":if case == "unavailable" { "当前系统没有可用的电源策略服务；请安装对应服务后重新检测。" } else { "操作系统提供的策略接口" },
            "source":null, "options":["balanced", "performance", "power-saver"]
        },{
            "id":"fan_control", "label":"风扇控制", "status":"unsupported",
            "reason":"没有已核验的型号专属接口；不会猜测 EC 寄存器或风扇参数。", "source":null, "options":[]
        }],
        "power_policy":{"scope":"操作系统 / 不修改 OEM 固件", "current":"balanced", "options":["balanced", "performance", "power-saver"], "source":null},
        "diagnostics":[{"code":"fixture_only", "message":format!("诊断信息包含长路径：{long}，需要完整换行展示。")}]
    })).unwrap();
    if case == "no_telemetry" {
        // Missing hardware identity and a wholly empty sensor inventory are
        // different from individual GPU readings being unavailable.
        snapshot.identity = Default::default();
        snapshot.telemetry.clear();
        snapshot.power_policy.current = None;
        snapshot.power_policy.options.clear();
        for capability in &mut snapshot.capabilities {
            capability.status = mechrevo_console_core::CapabilityStatus::Unavailable;
            capability.options.clear();
        }
    }
    let mut controller = Controller::new(FixtureBackend(snapshot.clone())).unwrap();
    controller.snapshot = Some(snapshot);
    if case == "error" {
        let mut error =
            ControlError::new("access_denied", format!("权限不足，操作没有完成。{long}"));
        error.details = vec![
            format!("保留原始错误以便排查：{long}"),
            "已保留原有设置，重新检测后再试。".into(),
        ];
        controller.error = Some(error);
    }
    controller.catalog = (0..3)
        .map(|index| CatalogModel {
            id: format!("fixture-{index}"),
            name: format!("机械革命测试机型 {index} {long}"),
            slug: format!("fixture-{long}-{index}"),
            category: Some("游戏本".into()),
            series: Some("旷世".into()),
            entry_kind: "fixture-only".into(),
            url: "https://example.invalid/fixture-only".into(),
            evidence: vec![],
            support_note: "仅测试布局，未核验型号兼容性。".into(),
        })
        .collect();
    let mut preferences = Preferences::default();
    if case == "large_compact" {
        preferences.display_mode = DisplayMode::Compact;
        preferences.text_scale = 1.6;
    }
    DesktopApp::with_controller(controller, preferences)
}

struct Harness {
    ctx: Context,
    textures: HashMap<TextureId, ColorImage>,
    physical: [u32; 2],
    dpi: f32,
    frame_number: u32,
}
impl Harness {
    fn new(physical: [u32; 2], dpi: f32, fonts: Arc<SystemFontProvider>) -> Self {
        let ctx = Context::default();
        ctx.add_font_provider(fonts);
        // A tofu/replacement glyph must fail the same layout regression that draws it.
        ctx.set_missing_glyph_policy(egui::epaint::text::MissingGlyphPolicy::Panic);
        Self {
            ctx,
            textures: HashMap::new(),
            physical,
            dpi,
            frame_number: 0,
        }
    }
    fn frame(&mut self, app: &mut DesktopApp) -> (RenderMetrics, FullOutput) {
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(
                    self.physical[0] as f32 / (self.dpi * self.ctx.zoom_factor()),
                    self.physical[1] as f32 / (self.dpi * self.ctx.zoom_factor()),
                ),
            )),
            time: Some(self.frame_number as f64 * 0.1),
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(self.dpi);
        let mut metrics = None;
        let mut output = self.ctx.run_ui(input, |ui| metrics = Some(app.render(ui)));
        self.frame_number += 1;
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                let egui::ImageData::Color(image) = delta.image;
                if let Some([x, y]) = delta.pos {
                    let texture = self
                        .textures
                        .get_mut(&id)
                        .expect("partial update requires a known texture");
                    assert!(
                        x + image.size[0] <= texture.size[0]
                            && y + image.size[1] <= texture.size[1]
                    );
                    for row in 0..image.size[1] {
                        let start = (y + row) * texture.size[0] + x;
                        texture.pixels[start..start + image.size[0]].copy_from_slice(
                            &image.pixels[row * image.size[0]..(row + 1) * image.size[0]],
                        );
                    }
                } else {
                    self.textures.insert(id, (*image).clone());
                }
            }
        }
        // Our captures need only textures present in this frame. egui currently uses
        // the persistent font atlas; free requests must still be acknowledged.
        output.textures_delta.free.clear();
        (metrics.unwrap(), output)
    }
    fn capture(&self, output: FullOutput, path: &Path) {
        // Limit optional PNG artifacts without skipping a render or assertion.
        if let Ok(filter) = std::env::var("MECHREVO_GUI_CAPTURE_MATCH") {
            let patterns: Vec<_> = filter
                .split(',')
                .map(str::trim)
                .filter(|pattern| !pattern.is_empty())
                .collect();
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if !patterns.is_empty() && !patterns.iter().any(|pattern| filename.contains(pattern)) {
                return;
            }
        }
        let [width, height] = self.physical;
        let mut pixels = vec![[27, 29, 34, 255]; (width * height) as usize];
        for primitive in self.ctx.tessellate(output.shapes, output.pixels_per_point) {
            let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive else {
                panic!("headless renderer does not accept GPU callbacks");
            };
            let texture = self
                .textures
                .get(&mesh.texture_id)
                .expect("mesh texture must be uploaded");
            let clip = primitive.clip_rect * output.pixels_per_point;
            for triangle in mesh.indices.chunks_exact(3) {
                let v = [
                    mesh.vertices[triangle[0] as usize],
                    mesh.vertices[triangle[1] as usize],
                    mesh.vertices[triangle[2] as usize],
                ];
                let p = v.map(|v| v.pos * output.pixels_per_point);
                let cross = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| {
                    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
                };
                let area = cross(p[0], p[1], p[2]);
                if area.abs() < 0.00001 {
                    continue;
                }
                let min_x = p
                    .iter()
                    .map(|p| p.x)
                    .fold(f32::INFINITY, f32::min)
                    .max(clip.min.x)
                    .max(0.0)
                    .floor() as u32;
                let min_y = p
                    .iter()
                    .map(|p| p.y)
                    .fold(f32::INFINITY, f32::min)
                    .max(clip.min.y)
                    .max(0.0)
                    .floor() as u32;
                let max_x = p
                    .iter()
                    .map(|p| p.x)
                    .fold(f32::NEG_INFINITY, f32::max)
                    .min(clip.max.x)
                    .min(width as f32)
                    .ceil()
                    .max(0.0) as u32;
                let max_y = p
                    .iter()
                    .map(|p| p.y)
                    .fold(f32::NEG_INFINITY, f32::max)
                    .min(clip.max.y)
                    .min(height as f32)
                    .ceil()
                    .max(0.0) as u32;
                for y in min_y..max_y {
                    for x in min_x..max_x {
                        let sample = pos2(x as f32 + 0.5, y as f32 + 0.5);
                        if !clip.contains(sample) {
                            continue;
                        }
                        let weights = [
                            cross(p[1], p[2], sample) / area,
                            cross(p[2], p[0], sample) / area,
                            cross(p[0], p[1], sample) / area,
                        ];
                        if weights.iter().any(|w| *w < -0.00001) {
                            continue;
                        }
                        let u = (0..3).map(|i| weights[i] * v[i].uv.x).sum::<f32>();
                        let t = (0..3).map(|i| weights[i] * v[i].uv.y).sum::<f32>();
                        let texel = sample_texture(texture, u, t);
                        let mut color = [0.0; 4];
                        for (channel, out) in color.iter_mut().enumerate() {
                            *out = (0..3)
                                .map(|i| weights[i] * v[i].color.to_array()[channel] as f32 / 255.0)
                                .sum::<f32>()
                                * texel[channel];
                        }
                        let target = &mut pixels[(y * width + x) as usize];
                        for channel in 0..3 {
                            target[channel] = ((color[channel]
                                + target[channel] as f32 / 255.0 * (1.0 - color[3]))
                                * 255.0)
                                .round()
                                .clamp(0.0, 255.0)
                                as u8;
                        }
                    }
                }
            }
        }
        write_png(path, width, height, &pixels);
    }
}
fn sample_texture(image: &ColorImage, u: f32, v: f32) -> [f32; 4] {
    let x = (u * image.size[0] as f32 - 0.5).clamp(0.0, (image.size[0] - 1) as f32);
    let y = (v * image.size[1] as f32 - 0.5).clamp(0.0, (image.size[1] - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(image.size[0] - 1);
    let y1 = (y0 + 1).min(image.size[1] - 1);
    let get = |x: usize, y: usize| {
        image.pixels[y * image.size[0] + x]
            .to_array()
            .map(|c| c as f32 / 255.0)
    };
    let a = get(x0, y0);
    let b = get(x1, y0);
    let c = get(x0, y1);
    let d = get(x1, y1);
    std::array::from_fn(|i| {
        (a[i] * (1.0 - x.fract()) + b[i] * x.fract()) * (1.0 - y.fract())
            + (c[i] * (1.0 - x.fract()) + d[i] * x.fract()) * y.fract()
    })
}

// @mark mechrevo.console_ui role=verification
#[test]
fn responsive_layout_and_last_action_are_accessible_at_every_dpi() {
    let fonts = Arc::new(SystemFontProvider::new().with_locale(Some("zh-Hans")));
    let capture = std::env::var_os("MECHREVO_GUI_CAPTURE_DIR").map(std::path::PathBuf::from);
    if let Some(path) = &capture {
        std::fs::create_dir_all(path).unwrap();
    }
    let mut verified = 0;
    for dpi in [1.0, 1.25, 1.5, 2.0] {
        for logical in [[360, 280], [800, 600]] {
            // Native window minimum sizes are logical OS pixels; the monitor DPI
            // converts these to physical pixels independently of the text zoom.
            let physical = [
                (logical[0] as f32 * dpi).round() as u32,
                (logical[1] as f32 * dpi).round() as u32,
            ];
            for case in [
                "long_names",
                "multi_gpu",
                "error",
                "unavailable",
                "large_compact",
            ] {
                let label = format!(
                    "logical{}x{}-physical{}x{}-dpi{}-{case}",
                    logical[0],
                    logical[1],
                    physical[0],
                    physical[1],
                    (dpi * 100.0) as u32
                );
                let mut app = fixture(case);
                let mut harness = Harness::new(physical, dpi, fonts.clone());
                assert_layout(&label, &mut app, &mut harness, capture.as_deref(), true);
                verified += 1;
            }
            for case in [
                "settings_open",
                "sensor_sources",
                "catalog",
                "empty_dashboard",
                "empty_sensors",
            ] {
                let label = format!(
                    "logical{}x{}-physical{}x{}-dpi{}-{case}",
                    logical[0],
                    logical[1],
                    physical[0],
                    physical[1],
                    (dpi * 100.0) as u32
                );
                let mut app = fixture(match case {
                    "sensor_sources" => "multi_gpu",
                    "empty_dashboard" | "empty_sensors" => "no_telemetry",
                    _ => "long_names",
                });
                match case {
                    "settings_open" => {
                        app.preferences.text_scale = 1.6;
                        app.open_display_settings();
                    }
                    "sensor_sources" | "empty_sensors" => app.select_section(Section::Sensors),
                    "catalog" => app.select_section(Section::Catalog),
                    "empty_dashboard" => {}
                    _ => unreachable!(),
                }
                let mut harness = Harness::new(physical, dpi, fonts.clone());
                if case == "sensor_sources" {
                    assert_source_details_expand(&label, &mut app, &mut harness);
                }
                assert_layout(&label, &mut app, &mut harness, capture.as_deref(), false);
                verified += 1;
            }
        }
    }
    assert_eq!(verified, 80);
    println!("Verified {verified} rendered layouts: 4 DPI scales x 2 logical window sizes (native physical pixels = logical size x DPI) x 10 states/views; Chinese missing-glyph policy=Panic; long source paths wrap; empty telemetry is safe; last marker reachable.");
}

fn assert_source_details_expand(label: &str, app: &mut DesktopApp, harness: &mut Harness) {
    app.set_source_details_expanded(false);
    let mut collapsed = RenderMetrics::default();
    for _ in 0..4 {
        collapsed = harness.frame(app).0;
    }
    assert_width(label, &collapsed);
    // Exercise the same state change as the user-facing source-details button.
    // The real layout must grow; a flag that never renders the paths cannot pass.
    app.set_source_details_expanded(true);
    let mut expanded = RenderMetrics::default();
    for _ in 0..4 {
        expanded = harness.frame(app).0;
    }
    assert_width(label, &expanded);
    assert!(
        expanded.content_size.y > collapsed.content_size.y + 20.0,
        "{label}: source details did not expand the actual rendered content: collapsed={:?}, expanded={:?}",
        collapsed.content_size,
        expanded.content_size
    );
}

fn assert_layout(
    label: &str,
    app: &mut DesktopApp,
    harness: &mut Harness,
    capture: Option<&Path>,
    require_scroll: bool,
) {
    let mut top = None;
    for _ in 0..4 {
        top = Some(harness.frame(app));
    }
    let (metrics, output) = top.unwrap();
    assert!(
        (output.pixels_per_point - harness.dpi * app.preferences.text_scale).abs() < 0.001,
        "{label}: wrong DPI"
    );
    assert_width(label, &metrics);
    assert_client_bounds(label, &metrics, harness.physical, output.pixels_per_point);
    if let Some(path) = capture {
        harness.capture(output, &path.join(format!("{label}-top.png")));
    }
    app.scroll_to_bottom();
    let mut bottom = None;
    for _ in 0..4 {
        bottom = Some(harness.frame(app));
    }
    let (metrics, output) = bottom.unwrap();
    assert_width(label, &metrics);
    assert_client_bounds(label, &metrics, harness.physical, output.pixels_per_point);
    assert!(
        metrics.bottom_marker.min.y >= metrics.viewport.min.y - 1.0
            && metrics.bottom_marker.max.y <= metrics.viewport.max.y + 1.0,
        "{label}: last action/marker cannot be reached: marker={:?}, viewport={:?}, scroll={:?}",
        metrics.bottom_marker,
        metrics.viewport,
        metrics.scroll_offset
    );
    if require_scroll {
        assert!(
            metrics.scroll_offset.y > 0.0,
            "{label}: fixture should require vertical scrolling"
        );
    }
    if let Some(path) = capture {
        harness.capture(output, &path.join(format!("{label}-bottom.png")));
    }
}
fn assert_client_bounds(label: &str, metrics: &RenderMetrics, physical: [u32; 2], ppp: f32) {
    assert!(
        metrics.viewport.min.x >= -1.0
            && metrics.viewport.min.y >= -1.0
            && metrics.viewport.max.x <= physical[0] as f32 / ppp + 1.0
            && metrics.viewport.max.y <= physical[1] as f32 / ppp + 1.0,
        "{label}: content viewport exceeds client bounds"
    );
}
fn assert_width(label: &str, metrics: &RenderMetrics) {
    assert!(
        metrics.content_size.x.is_finite() && metrics.content_size.y.is_finite(),
        "{label}: invalid content size"
    );
    assert!(
        metrics.content_size.x <= metrics.viewport.width() + 1.0,
        "{label}: horizontal overflow: content={:?}, viewport={:?}",
        metrics.content_size,
        metrics.viewport
    );
    assert!(
        metrics.scroll_offset.x.abs() < 0.001,
        "{label}: horizontal scrolling must be unnecessary"
    );
}

// Minimal lossless PNG encoder avoids a screenshot dependency in the product.
// DEFLATE stored blocks are sufficient for bounded, optional test artifacts.
fn write_png(path: &Path, width: u32, height: u32, pixels: &[[u8; 4]]) {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = 0xffff_ffffu32;
        for byte in kind.iter().chain(data) {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
            }
        }
        out.extend_from_slice(&(!crc).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for row in pixels.chunks_exact(width as usize) {
        raw.push(0);
        for pixel in row {
            raw.extend_from_slice(pixel);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks = raw.chunks(65535);
    let count = blocks.len();
    for (index, block) in blocks.enumerate() {
        zlib.push(u8::from(index + 1 == count));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png).unwrap();
}
