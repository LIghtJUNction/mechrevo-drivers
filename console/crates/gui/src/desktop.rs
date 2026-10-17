//! Native Cyber-HUD: factual readings, responsive flow and explicit control plans.
use crate::{reading_text, Controller, DisplayMode, JobKind, Preferences, Theme};
use eframe::egui::{self, Color32, FontId, Rect, RichText, Stroke, Vec2};
use mechrevo_console_core::{CapabilityStatus, PowerProfile, SensorReading, Service};
use std::{collections::BTreeMap, time::Duration};

const ACCENT: Color32 = Color32::from_rgb(89, 220, 215);
const SUBTLE: Color32 = Color32::from_rgb(174, 166, 197);
const BACKGROUND: Color32 = Color32::from_rgb(18, 17, 26);
const PANEL: Color32 = Color32::from_rgb(29, 27, 41);
const ERROR: Color32 = Color32::from_rgb(255, 156, 151);

#[derive(Debug, Clone, Copy)]
pub struct RenderMetrics {
    pub content_size: Vec2,
    pub viewport: Rect,
    pub bottom_marker: Rect,
    pub scroll_offset: Vec2,
}

impl Default for RenderMetrics {
    fn default() -> Self {
        Self {
            content_size: Vec2::ZERO,
            viewport: Rect::NOTHING,
            bottom_marker: Rect::NOTHING,
            scroll_offset: Vec2::ZERO,
        }
    }
}

#[derive(Default, PartialEq, Eq, Clone, Copy)]
pub enum Section {
    #[default]
    Dashboard,
    Sensors,
    Catalog,
}

pub struct DesktopApp {
    pub controller: Controller,
    pub preferences: Preferences,
    section: Section,
    catalog_query: String,
    bottom_requested: bool,
    last_metrics: RenderMetrics,
    pending_close: bool,
    display_settings_requested: bool,
}

impl DesktopApp {
    pub fn new(creation: &eframe::CreationContext<'_>) -> Result<Self, std::io::Error> {
        let mut preferences: Preferences = creation
            .storage
            .and_then(|storage| eframe::get_value(storage, "display_preferences"))
            .unwrap_or_default();
        preferences.sanitize();
        let mut app = Self::with_controller(Controller::new(Service::new())?, preferences);
        app.configure(&creation.egui_ctx);
        // Startup only requests a read. No elevation, service install or hardware write.
        let _ = app.controller.refresh();
        Ok(app)
    }

    #[doc(hidden)]
    pub fn with_controller(controller: Controller, mut preferences: Preferences) -> Self {
        preferences.sanitize();
        Self {
            controller,
            preferences,
            section: Section::Dashboard,
            catalog_query: String::new(),
            bottom_requested: false,
            last_metrics: RenderMetrics::default(),
            pending_close: false,
            display_settings_requested: false,
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.bottom_requested = true;
    }

    pub fn select_section(&mut self, section: Section) {
        self.section = section;
    }
    pub fn open_display_settings(&mut self) {
        self.display_settings_requested = true;
    }

    fn configure(&self, ctx: &egui::Context) {
        let dark = match self.preferences.theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => ctx.system_theme().unwrap_or(egui::Theme::Dark) == egui::Theme::Dark,
        };
        let mut visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        if dark {
            visuals.panel_fill = BACKGROUND;
            visuals.window_fill = PANEL;
            visuals.extreme_bg_color = Color32::from_rgb(13, 13, 21);
            visuals.faint_bg_color = PANEL;
            visuals.override_text_color = Some(Color32::from_rgb(233, 229, 244));
            visuals.widgets.noninteractive.bg_stroke =
                Stroke::new(1.0, Color32::from_rgb(64, 57, 84));
            visuals.widgets.inactive.bg_fill = Color32::from_rgb(44, 39, 60);
            visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 51, 77);
            visuals.widgets.active.bg_fill = Color32::from_rgb(51, 71, 76);
            visuals.selection.bg_fill = Color32::from_rgb(38, 98, 103);
            visuals.selection.stroke = Stroke::new(1.0, ACCENT);
            visuals.hyperlink_color = ACCENT;
        }
        ctx.set_visuals(visuals);
        ctx.set_zoom_factor(self.preferences.text_scale);
        ctx.style_mut_of(
            if dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            },
            |style| {
                style.spacing.item_spacing = Vec2::new(10.0, 9.0);
                style.spacing.button_padding = Vec2::new(12.0, 7.0);
                style.spacing.interact_size.y = 32.0;
                style.wrap_mode = Some(egui::TextWrapMode::Wrap);
            },
        );
    }

    fn tick(&mut self, ctx: &egui::Context) {
        // A native close must not terminate the process between write, readback
        // and rollback. Defer closing while that transaction owns the worker.
        if ctx.input(|input| input.viewport().close_requested())
            && self.controller.write_in_progress()
        {
            self.pending_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.controller.poll();
        if self.pending_close && !self.controller.busy() {
            self.pending_close = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.preferences.auto_refresh
            && !self.controller.busy()
            && self.controller.preview.is_none()
            && self.controller.brightness_preview.is_none()
            && self.controller.worker_available
            && !self.pending_close
            && self
                .controller
                .last_refresh
                .is_some_and(|time| time.elapsed() >= Duration::from_secs(3))
        {
            let _ = self.controller.refresh();
        }
        if self.controller.busy() || self.preferences.auto_refresh {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    // @mark mechrevo.console_ui role=presentation
    pub fn render(&mut self, root: &mut egui::Ui) -> RenderMetrics {
        self.configure(root.ctx());
        self.tick(root.ctx());
        let mut bottom_marker = Rect::NOTHING;
        let scroll = egui::ScrollArea::vertical()
            .id_salt("console-content")
            .auto_shrink([false, false]);
        let scroll = if self.bottom_requested {
            // Finite content bounds avoid f32::MAX losing the viewport offset
            // during egui's layout subtraction and snapping back to the top.
            scroll.vertical_scroll_offset(self.last_metrics.content_size.y.max(0.0))
        } else {
            scroll
        };
        self.bottom_requested = false;
        let output = egui::Frame::central_panel(root.style())
            .show(root, |root| {
                scroll.show(root, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.heading(RichText::new("机械革命控制台").size(24.0));
                    ui.label("Windows / Linux · 硬件读数与操作计划");
                    self.toolbar(ui);
                    ui.separator();
                    self.feedback(ui);
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.section, Section::Dashboard, "设备与控制");
                        ui.selectable_value(&mut self.section, Section::Sensors, "传感器来源");
                        ui.selectable_value(&mut self.section, Section::Catalog, "型号目录");
                    });
                    ui.add_space(8.0);
                    match self.section {
                        Section::Dashboard => {
                            self.identity(ui);
                            section_title(ui, "运行拓扑");
                            topology(
                                ui,
                                self.controller
                                    .snapshot
                                    .as_ref()
                                    .map(|snapshot| snapshot.telemetry.as_slice())
                                    .unwrap_or(&[]),
                            );
                            ui.label(
                                RichText::new(
                                    "连线表示采集通道，不代表已验证的热管结构。无数据时保持静止。",
                                )
                                .small()
                                .color(secondary(ui)),
                            );
                            self.power_source(ui);
                            self.power_controls(ui);
                            self.brightness_controls(ui);
                            self.capabilities(ui);
                        }
                        Section::Sensors => self.sensors(ui),
                        Section::Catalog => self.catalog(ui),
                    }
                    ui.add_space(12.0);
                    ui.separator();
                    bottom_marker = ui
                        .label(
                            RichText::new("读取数据不会更改硬件。应用按钮仅执行已预览的系统操作。")
                                .small()
                                .color(secondary(ui)),
                        )
                        .rect;
                })
            })
            .inner;
        self.last_metrics = RenderMetrics {
            content_size: output.content_size,
            viewport: output.inner_rect,
            bottom_marker,
            scroll_offset: output.state.clamped_offset(),
        };
        self.last_metrics
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.controller.busy() && self.controller.worker_available,
                    egui::Button::new("刷新读数"),
                )
                .clicked()
            {
                let _ = self.controller.refresh();
            }
            ui.checkbox(&mut self.preferences.auto_refresh, "每 3 秒刷新");
            if let Some(last) = self.controller.last_refresh {
                ui.label(
                    RichText::new(format!("{} 秒前采集", last.elapsed().as_secs()))
                        .small()
                        .color(secondary(ui)),
                );
            }
        });
        let open = if std::mem::take(&mut self.display_settings_requested) {
            Some(true)
        } else {
            None
        };
        egui::CollapsingHeader::new("显示设置").id_salt("appearance").open(open).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("布局");
                ui.selectable_value(&mut self.preferences.display_mode, DisplayMode::Standard, "完整");
                ui.selectable_value(&mut self.preferences.display_mode, DisplayMode::Compact, "紧凑");
                ui.label("主题");
                ui.selectable_value(&mut self.preferences.theme, Theme::System, "随系统");
                ui.selectable_value(&mut self.preferences.theme, Theme::Dark, "深色");
                ui.selectable_value(&mut self.preferences.theme, Theme::Light, "浅色");
            });
            let label=ui.label(format!("文字缩放：{:.0}%",self.preferences.text_scale*100.0));
            ui.scope(|ui| {
                ui.spacing_mut().slider_width=ui.available_width().max(1.0);
                ui.add(egui::Slider::new(&mut self.preferences.text_scale,0.8..=1.6).step_by(0.05).show_value(false)).labelled_by(label.id);
            });
            ui.label(RichText::new("显示设置会自动保存；硬件操作计划不会保存。窗口可缩小，全部操作可纵向滚动。")
                .small().color(secondary(ui)));
        });
    }

    fn feedback(&self, ui: &mut egui::Ui) {
        if self.pending_close {
            ui.label("正在完成写入、回读或恢复。完成后自动退出；请暂时保留此窗口。");
        }
        if let Some((kind, elapsed)) = self.controller.activity() {
            let activity = match kind {
                JobKind::Refresh => "正在读取设备",
                JobKind::Preview => "正在生成操作计划",
                JobKind::Apply => "正在应用并回读",
            };
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(format!("{activity} · {:.1} 秒", elapsed.as_secs_f32()));
            });
            if elapsed > Duration::from_secs(10) {
                ui.label("后台响应较慢；窗口仍可操作。请等待本次请求结束。");
            }
        }
        if let Some(error) = &self.controller.error {
            ui.colored_label(error_color(ui), format!("操作未确认 · {}", error.message));
            for detail in &error.details {
                ui.label(detail);
            }
            ui.label(
                RichText::new(format!(
                    "错误代码：{}。可刷新核对；若后台已退出，请重新打开控制台。",
                    error.code
                ))
                .small(),
            );
        }
        if let Some(applied) = &self.controller.applied {
            ui.label(
                RichText::new(format!(
                    "上次操作已应用并回读：{} → {} · {}",
                    applied.previous, applied.confirmed, applied.scope
                ))
                .color(accent(ui)),
            );
        }
        if let Some(applied) = &self.controller.brightness_applied {
            ui.label(
                RichText::new(format!(
                    "亮度请求已回读：{} · 原始值 {} → {}",
                    applied.device_id, applied.previous, applied.confirmed
                ))
                .color(accent(ui)),
            );
            if let Some(actual) = applied.actual_value {
                ui.label(format!(
                    "驱动报告的 actual_brightness：{actual}（可能延迟，并非实测亮度）"
                ));
            }
        }
    }

    fn identity(&self, ui: &mut egui::Ui) {
        let Some(snapshot) = &self.controller.snapshot else {
            ui.label("等待设备检测。型号、温度与风扇读数尚未采集。");
            return;
        };
        let identity = &snapshot.identity;
        ui.heading(identity.product_name.as_deref().unwrap_or("未识别的设备"));
        let fields = [
            format!("系统：{}", snapshot.platform),
            format!(
                "厂商：{}",
                identity.manufacturer.as_deref().unwrap_or("未采集")
            ),
            format!(
                "主板：{}",
                identity.board_name.as_deref().unwrap_or("未采集")
            ),
            format!(
                "BIOS：{}",
                identity.bios_version.as_deref().unwrap_or("未采集")
            ),
        ];
        let count = if ui.available_width() >= 620.0 { 2 } else { 1 };
        ui.columns(count, |columns| {
            for (index, text) in fields.iter().enumerate() {
                columns[index % count].label(text);
            }
        });
        let identification = mechrevo_console_core::registry::identify(identity);
        let state = match identification.status.as_str() {
            "hardware_partial" => "身份与历史 BIOS 证据匹配 · 仅有部分实机验证",
            "identified" => "身份与来源匹配 · 当前 BIOS 尚未验证",
            "ambiguous" => "身份存在多个匹配 · 不启用专有控制",
            _ => "未匹配到确切型号 · 不启用专有控制",
        };
        ui.colored_label(accent(ui), state);
        egui::CollapsingHeader::new("识别依据")
            .id_salt("identity-evidence")
            .show(ui, |ui| {
                ui.label(&identification.reason);
                for matched in &identification.matches {
                    ui.label(format!("{} · {}", matched.model_id, matched.title));
                    for source in &matched.sources {
                        if source.starts_with("https://") {
                            ui.hyperlink(source);
                        } else {
                            ui.add(egui::Label::new(source).selectable(true));
                        }
                    }
                }
                ui.label(&identity.support_note);
            });
    }

    fn power_controls(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "系统电源策略");
        ui.label("此处更改操作系统电源策略；OEM 性能模式另列于能力清单。");
        let current = self
            .controller
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.power_policy.current.as_deref())
            .unwrap_or("未采集");
        ui.label(format!("当前：{current}"));
        let mut selected = self.controller.selected_profile;
        ui.horizontal_wrapped(|ui| {
            for (profile, label) in [
                (PowerProfile::PowerSaver, "节能"),
                (PowerProfile::Balanced, "平衡"),
                (PowerProfile::Performance, "性能"),
            ] {
                let supported = self.controller.snapshot.as_ref().is_some_and(|snapshot| {
                    mechrevo_console_core::profile_available(snapshot, profile)
                });
                if ui
                    .add_enabled(
                        supported,
                        egui::Button::selectable(selected == profile, label),
                    )
                    .on_disabled_hover_text("当前系统未公布此电源策略。请刷新查看可用策略。")
                    .clicked()
                {
                    selected = profile;
                }
            }
        });
        self.controller.select_profile(selected);
        let reason = self
            .controller
            .policy_unavailable_reason()
            .map(str::to_owned);
        if let Some(reason) = &reason {
            ui.label(format!("当前不可操作：{reason}"));
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    reason.is_none() && !self.controller.busy(),
                    egui::Button::new("预览更改"),
                )
                .clicked()
            {
                let _ = self.controller.preview_profile();
            }
            if ui
                .add_enabled(
                    reason.is_none()
                        && !self.controller.busy()
                        && self.controller.preview.is_some(),
                    egui::Button::new("应用并回读"),
                )
                .clicked()
            {
                let _ = self.controller.apply_preview();
            }
        });
        if let Some(plan) = &self.controller.preview {
            ui.label(
                RichText::new(format!("待应用：{} → {}", plan.previous, plan.target))
                    .strong()
                    .color(accent(ui)),
            );
            ui.label(format!("范围：{}", plan.scope));
            evidence(ui, &plan.source.backend, &plan.source.path);
        } else {
            ui.label(
                RichText::new("先预览当前系统上的更改，再应用；修改目标或刷新会作废旧计划。")
                    .small()
                    .color(secondary(ui)),
            );
        }
    }

    fn power_source(&self, ui: &mut egui::Ui) {
        let Some(snapshot) = &self.controller.snapshot else {
            return;
        };
        section_title(ui, "供电与电池");
        let mut observed = false;
        for reading in snapshot.telemetry.iter().filter(|reading| {
            reading.kind == "external_power_online" || reading.kind.starts_with("battery_")
        }) {
            observed = true;
            let value = if reading.kind == "external_power_online"
                && reading.status == CapabilityStatus::Available
            {
                match reading.value {
                    Some(1.0) => "已连接".into(),
                    Some(0.0) => "未连接".into(),
                    _ => "未采集".into(),
                }
            } else {
                reading_text(reading)
            };
            ui.label(format!("{} · {value}", reading.label));
            evidence(ui, &reading.source.backend, &reading.source.path);
        }
        if !observed {
            ui.label("未采集到供电或电池读数。");
        }
        for diagnostic in snapshot
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "battery_status")
        {
            ui.label(&diagnostic.message);
        }
    }

    // @mark mechrevo.display_brightness role=presentation
    fn brightness_controls(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "屏幕亮度");
        ui.label("通过系统公开的背光接口调节。多显示设备需分别选择；应用前先预览。");
        let mut selected = self.controller.selected_brightness_device.clone();
        if let Some(snapshot) = &self.controller.snapshot {
            for device in &snapshot.brightness {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .selectable_label(
                            selected.as_deref() == Some(device.id.as_str()),
                            &device.label,
                        )
                        .clicked()
                    {
                        selected = Some(device.id.clone());
                    }
                    let actual = device
                        .current_percent
                        .filter(|value| value.is_finite())
                        .map(|value| format!("{value:.1}%"))
                        .unwrap_or_else(|| "未采集".into());
                    let requested = device
                        .control_value
                        .zip(device.maximum)
                        .filter(|(_, maximum)| *maximum > 0)
                        .map(|(value, maximum)| {
                            format!("{:.1}%", f64::from(value) * 100.0 / f64::from(maximum))
                        })
                        .unwrap_or_else(|| "未采集".into());
                    ui.label(format!("当前请求：{requested} · 驱动报告：{actual}"));
                });
                if self.preferences.display_mode == DisplayMode::Standard {
                    evidence(ui, &device.source.backend, &device.source.path);
                    ui.label(format!(
                        "请求值：{} · 驱动报告值：{} · 上限：{}",
                        device
                            .control_value
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "未知".into()),
                        device
                            .actual_value
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "未知".into()),
                        device
                            .maximum
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "未知".into())
                    ));
                }
            }
        }
        if let Some(device) = selected {
            self.controller.select_brightness_device(&device);
        }
        let device = self.controller.snapshot.as_ref().and_then(|snapshot| {
            snapshot.brightness.iter().find(|device| {
                Some(device.id.as_str()) == self.controller.selected_brightness_device.as_deref()
            })
        });
        let levels = device
            .map(|device| device.supported_percentages.clone())
            .unwrap_or_default();
        let mut percent = self.controller.requested_brightness;
        if levels.is_empty() {
            let label = ui.label(format!("目标亮度：{percent}%"));
            ui.scope(|ui| {
                ui.spacing_mut().slider_width = ui.available_width().max(1.0);
                ui.add_enabled(
                    device.is_some(),
                    egui::Slider::new(&mut percent, 0..=100).show_value(false),
                )
                .labelled_by(label.id);
            });
        } else {
            ui.label("设备支持的亮度档位");
            ui.horizontal_wrapped(|ui| {
                for level in &levels {
                    ui.selectable_value(&mut percent, *level, format!("{level}%"));
                }
            });
        }
        self.controller.set_brightness_target(percent);
        let reason = self
            .controller
            .brightness_unavailable_reason()
            .map(str::to_owned);
        if let Some(reason) = &reason {
            ui.label(format!("当前不可操作：{reason}"));
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    reason.is_none() && !self.controller.busy(),
                    egui::Button::new("预览亮度"),
                )
                .clicked()
            {
                let _ = self.controller.preview_brightness();
            }
            if ui
                .add_enabled(
                    reason.is_none()
                        && !self.controller.busy()
                        && self.controller.brightness_preview.is_some(),
                    egui::Button::new("应用亮度并回读"),
                )
                .clicked()
            {
                let _ = self.controller.apply_brightness_preview();
            }
        });
        if let Some(plan) = &self.controller.brightness_preview {
            ui.label(
                RichText::new(format!(
                    "待应用：{} · {}% · 原始值 {} → {} / {}",
                    plan.device_id,
                    plan.requested_percent,
                    plan.previous,
                    plan.target,
                    plan.maximum
                ))
                .strong()
                .color(accent(ui)),
            );
            evidence(ui, &plan.source.backend, &plan.source.path);
        }
    }

    fn capabilities(&self, ui: &mut egui::Ui) {
        section_title(ui, "硬件能力");
        let Some(snapshot) = &self.controller.snapshot else {
            ui.label("检测完成后显示接口与限制。");
            return;
        };
        for capability in snapshot
            .capabilities
            .iter()
            .filter(|cap| cap.id != "power_profile" && cap.id != "brightness")
        {
            let label = match capability.id.as_str() {
                "fan_control" => "风扇曲线 / 强冷",
                "oem_performance" => "OEM 性能模式",
                "gpu_control" => "GPU 调校",
                "mux_control" => "显卡切换",
                "battery_charge_limit" => "电池充电上限",
                _ => capability.label.as_str(),
            };
            ui.horizontal_wrapped(|ui| {
                if capability.id == "telemetry" {
                    ui.label("系统传感器（只读）");
                } else {
                    ui.add_enabled(false, egui::Button::new(label));
                }
                ui.label(match capability.status {
                    CapabilityStatus::Available => "已检测接口",
                    CapabilityStatus::Unsupported => "未支持",
                    CapabilityStatus::Unavailable => "不可用",
                });
            });
            ui.label(&capability.reason);
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled(false, egui::Button::new("外置水冷"));
            ui.label("未支持：尚无经过验证的设备协议与恢复流程。");
        });
    }

    fn sensors(&self, ui: &mut egui::Ui) {
        section_title(ui, "传感器与采集来源");
        let Some(snapshot) = &self.controller.snapshot else {
            ui.label("尚未采集，请刷新读数。");
            return;
        };
        if snapshot.telemetry.is_empty() {
            ui.label("没有可用传感器数据。未读取的数据不会显示为 0。");
        }
        for reading in &snapshot.telemetry {
            ui.push_id(&reading.id, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&reading.label).strong());
                    ui.label(RichText::new(reading_text(reading)).color(accent(ui)));
                });
                ui.label(format!("设备：{}", reading.device_id));
                if let Some(reason) = &reading.reason {
                    ui.label(format!("原因：{reason}"));
                }
                evidence(ui, &reading.source.backend, &reading.source.path);
                ui.separator();
            });
        }
        if !snapshot.diagnostics.is_empty() {
            section_title(ui, "检测信息");
            for diagnostic in &snapshot.diagnostics {
                ui.label(format!("{} · {}", diagnostic.code, diagnostic.message));
            }
        }
    }

    fn catalog(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "型号目录");
        ui.label("官方产品条目用于检索。目录收录不代表驱动或硬件功能已经兼容。");
        ui.add(
            egui::TextEdit::singleline(&mut self.catalog_query)
                .hint_text("搜索型号、系列或产品名称")
                .desired_width(f32::INFINITY),
        );
        let models = self.controller.matching_models(&self.catalog_query);
        ui.label(format!("找到 {} 个条目", models.len()));
        for model in models {
            ui.push_id(&model.id, |ui| {
                ui.label(RichText::new(&model.name).strong());
                ui.label(format!(
                    "{} · {}",
                    model.id,
                    model.series.as_deref().unwrap_or("未注明系列")
                ));
                ui.label("兼容状态：待验证");
                if self.preferences.display_mode == DisplayMode::Standard {
                    ui.label(&model.support_note);
                }
                ui.hyperlink_to("查看官方来源", &model.url);
                ui.separator();
            });
        }
    }
}

impl eframe::App for DesktopApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe calls logic while the native window is hidden as well. Closing
        // a minimized window must preserve the same write/readback close guard.
        self.tick(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "display_preferences", &self.preferences);
    }
}

fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.add_space(18.0);
    ui.heading(RichText::new(title).size(18.0));
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 3.0), egui::Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0, accent(ui).gamma_multiply(0.5)),
    );
    ui.add_space(4.0);
}

fn accent(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        ACCENT
    } else {
        Color32::from_rgb(9, 100, 107)
    }
}
fn secondary(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        SUBTLE
    } else {
        Color32::from_rgb(87, 78, 105)
    }
}
fn error_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        ERROR
    } else {
        Color32::from_rgb(162, 31, 29)
    }
}
fn evidence(ui: &mut egui::Ui, backend: &str, path: &str) {
    ui.label(
        RichText::new(format!("来源：{backend} · {path}"))
            .small()
            .color(secondary(ui)),
    );
}

struct Node {
    title: String,
    readings: Vec<String>,
}

fn topology_nodes(readings: &[SensorReading]) -> Vec<Node> {
    let mut devices: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for reading in readings {
        let identity = format!(
            "{} {} {}",
            reading.device_id, reading.label, reading.source.path
        )
        .to_lowercase();
        let category = if matches!(reading.kind.as_str(), "fan_speed" | "fan") {
            "FAN"
        } else if ["nvidia", "amdgpu", "gpu"]
            .iter()
            .any(|part| identity.contains(part))
        {
            "GPU"
        } else if ["cpu", "coretemp", "k10temp", "x86_pkg_temp"]
            .iter()
            .any(|part| identity.contains(part))
        {
            "CPU"
        } else {
            continue;
        };
        let key = format!("{category}:{}", reading.device_id);
        let device = devices
            .entry(key)
            .or_insert_with(|| (category.into(), vec![]));
        if device.1.len() < 3 {
            device
                .1
                .push(format!("{} {}", reading.label, reading_text(reading)));
        }
    }
    let mut nodes: Vec<Node> = devices
        .into_iter()
        .map(|(device, (_, readings))| Node {
            title: device,
            readings,
        })
        .collect();
    for category in ["CPU", "GPU", "FAN"] {
        if !nodes.iter().any(|node| node.title.starts_with(category)) {
            nodes.push(Node {
                title: category.into(),
                readings: vec!["未采集".into()],
            });
        }
    }
    nodes
}

fn topology(ui: &mut egui::Ui, readings: &[SensorReading]) {
    let nodes = topology_nodes(readings);
    let width = ui.available_width();
    let wide = width >= 620.0;
    let columns = if wide { 2 } else { 1 };
    let gap = if wide { 68.0 } else { 0.0 };
    let node_width = (width - gap) / columns as f32;
    let rows = nodes.len().div_ceil(columns);
    let layouts: Vec<_> = nodes
        .iter()
        .map(|node| {
            let lines: Vec<_> = std::iter::once(&node.title)
                .chain(node.readings.iter())
                .enumerate()
                .map(|(index, line)| {
                    let wrap_width = (node_width - 22.0).max(40.0);
                    let galley = ui.painter().layout(
                        line.clone(),
                        FontId::proportional(if index == 0 { 14.0 } else { 12.0 }),
                        if index == 0 {
                            accent(ui)
                        } else {
                            ui.visuals().text_color()
                        },
                        wrap_width,
                    );
                    debug_assert!(
                        galley.size().x <= wrap_width + 1.0,
                        "topology text exceeds its actual node width"
                    );
                    galley
                })
                .collect();
            let height = lines.iter().map(|line| line.size().y + 5.0).sum::<f32>() + 20.0;
            (lines, height.max(90.0))
        })
        .collect();
    let row_heights: Vec<_> = (0..rows)
        .map(|row| {
            layouts
                .iter()
                .skip(row * columns)
                .take(columns)
                .map(|(_, height)| *height)
                .fold(0.0, f32::max)
        })
        .collect();
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width, row_heights.iter().sum::<f32>() + rows as f32 * 14.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    if wide {
        let center = rect.center().x;
        painter.line_segment(
            [
                egui::pos2(center, rect.top() + 20.0),
                egui::pos2(center, rect.bottom() - 30.0),
            ],
            Stroke::new(1.0, accent(ui).gamma_multiply(0.5)),
        );
    }
    for (index, (lines, _)) in layouts.iter().enumerate() {
        let column = index % columns;
        let row = index / columns;
        let y_offset = row_heights.iter().take(row).sum::<f32>() + row as f32 * 14.0;
        let node_rect = Rect::from_min_size(
            rect.min + Vec2::new(column as f32 * (node_width + gap), y_offset),
            Vec2::new(node_width, row_heights[row]),
        );
        let stroke = Stroke::new(1.0, accent(ui).gamma_multiply(0.7));
        painter.rect_filled(
            node_rect,
            3.0,
            if ui.visuals().dark_mode {
                PANEL
            } else {
                Color32::from_rgb(236, 233, 243)
            },
        );
        // Open brackets carry the HUD; ordinary controls keep familiar affordances.
        for (corner, sign) in [
            (node_rect.left_top(), Vec2::new(1.0, 1.0)),
            (node_rect.right_top(), Vec2::new(-1.0, 1.0)),
            (node_rect.left_bottom(), Vec2::new(1.0, -1.0)),
            (node_rect.right_bottom(), Vec2::new(-1.0, -1.0)),
        ] {
            painter.line_segment([corner, corner + Vec2::new(sign.x * 16.0, 0.0)], stroke);
            painter.line_segment([corner, corner + Vec2::new(0.0, sign.y * 16.0)], stroke);
        }
        if wide {
            let edge = if column == 0 {
                node_rect.right_center()
            } else {
                node_rect.left_center()
            };
            painter.line_segment([edge, egui::pos2(rect.center().x, edge.y)], stroke);
        }
        let mut y = node_rect.top() + 10.0;
        for galley in lines {
            let height = galley.size().y;
            painter.galley(
                egui::pos2(node_rect.left() + 10.0, y),
                galley.clone(),
                ui.visuals().text_color(),
            );
            y += height + 5.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mechrevo_console_core::EvidenceSource;

    // @mark mechrevo.console_ui role=verification
    #[test]
    fn production_fan_speed_reading_reaches_fan_topology_without_a_fake_missing_node() {
        let reading = SensorReading {
            id: "hwmon0/fan1".into(),
            label: "CPU fan".into(),
            device_id: "hwmon0".into(),
            kind: "fan_speed".into(),
            value: Some(2345.0),
            unit: "RPM".into(),
            source: EvidenceSource {
                backend: "linux_sysfs".into(),
                path: "/sys/class/hwmon/hwmon0/fan1_input".into(),
                device_id: Some("hwmon0".into()),
            },
            status: CapabilityStatus::Available,
            reason: None,
        };
        let nodes = topology_nodes(&[reading]);
        let fans: Vec<_> = nodes
            .iter()
            .filter(|node| node.title.starts_with("FAN"))
            .collect();
        assert_eq!(fans.len(), 1);
        assert_eq!(fans[0].title, "FAN:hwmon0");
        assert!(fans[0].readings[0].contains("2345.0 RPM"));
        assert!(!fans[0].readings[0].contains("未采集"));
    }
}
