//! A quiet, responsive desktop surface built around real device readings.
use crate::{
    appearance,
    fan_editor::{FanEditor, FanEditorUpdate},
    reading_text, Controller, DisplayMode, JobKind, Preferences, Theme,
};
use eframe::egui::{self, Color32, FontId, Rect, RichText, Stroke, Vec2};
use mechrevo_console_core::{
    CapabilityStatus, OemControlKind, OemDomain, OemValue, PowerProfile, SensorReading,
};
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

const BACKGROUND: Color32 = Color32::from_rgb(23, 24, 24);
const PANEL: Color32 = Color32::from_rgb(31, 32, 31);
const FOREGROUND: Color32 = Color32::from_rgb(246, 246, 243);
const SUBTLE: Color32 = Color32::from_rgb(164, 167, 169);
const GREEN: Color32 = Color32::from_rgb(167, 192, 140);
const AMBER: Color32 = Color32::from_rgb(221, 187, 124);
const ERROR: Color32 = Color32::from_rgb(237, 154, 140);
const HISTORY_LIMIT: usize = 120;

#[derive(Debug, Clone, Copy)]
pub struct RenderMetrics {
    pub content_size: Vec2,
    pub viewport: Rect,
    pub bottom_marker: Rect,
    pub scroll_offset: Vec2,
    pub heading_rect: Rect,
    pub thermal_panel_rect: Rect,
}
impl Default for RenderMetrics {
    fn default() -> Self {
        Self {
            content_size: Vec2::ZERO,
            viewport: Rect::NOTHING,
            bottom_marker: Rect::NOTHING,
            scroll_offset: Vec2::ZERO,
            heading_rect: Rect::NOTHING,
            thermal_panel_rect: Rect::NOTHING,
        }
    }
}
#[derive(Default, PartialEq, Eq, Clone, Copy)]
pub enum Section {
    #[default]
    Dashboard,
    Controls,
    Fans,
    Sensors,
    Catalog,
}

#[derive(Default)]
struct TemperatureHistory {
    machine: String,
    last_refresh: Option<Instant>,
    sampled: bool,
    series: BTreeMap<String, VecDeque<(Instant, Option<f64>)>>,
}
impl TemperatureHistory {
    fn observe(
        &mut self,
        snapshot: Option<&mechrevo_console_core::Snapshot>,
        refreshed: Option<Instant>,
    ) {
        self.observe_readings(
            snapshot,
            snapshot
                .map(|snapshot| snapshot.telemetry.as_slice())
                .unwrap_or(&[]),
            refreshed,
        );
    }

    fn observe_controller(&mut self, controller: &Controller) {
        if let Some(sample) = &controller.thermal_sample {
            if controller.thermal_error.is_some() {
                self.observe_readings(
                    controller.snapshot.as_ref(),
                    &[],
                    controller.thermal_last_attempt,
                );
            } else {
                self.observe_readings(
                    controller.snapshot.as_ref(),
                    &sample.readings,
                    Some(sample.started_at),
                );
            }
        } else if controller.thermal_error.is_some() {
            self.observe_readings(
                controller.snapshot.as_ref(),
                &[],
                controller.thermal_last_attempt,
            );
        } else {
            self.observe(controller.snapshot.as_ref(), controller.last_refresh);
        }
    }

    fn observe_readings(
        &mut self,
        snapshot: Option<&mechrevo_console_core::Snapshot>,
        readings: &[SensorReading],
        refreshed: Option<Instant>,
    ) {
        let machine = snapshot
            .map(|snapshot| {
                format!(
                    "{:?}|{:?}|{:?}|{:?}|{}",
                    snapshot.identity.manufacturer,
                    snapshot.identity.product_name,
                    snapshot.identity.board_name,
                    snapshot.identity.bios_version,
                    snapshot.platform
                )
            })
            .unwrap_or_else(|| "unidentified".into());
        if self.machine != machine {
            self.machine = machine;
            self.series.clear();
            self.sampled = false;
            self.last_refresh = None;
        }
        if self.sampled && self.last_refresh == refreshed {
            return;
        }
        let time = refreshed.unwrap_or_else(Instant::now);
        if let Some(previous) = self.last_refresh {
            if time.saturating_duration_since(previous) > crate::THERMAL_STALE_AFTER {
                let gap = previous + crate::THERMAL_STALE_AFTER;
                for samples in self.series.values_mut() {
                    samples.push_back((gap, None));
                }
            }
        }
        // Missing or unavailable sensors make an explicit gap, never a zero.
        for samples in self.series.values_mut() {
            samples.push_back((time, None));
        }
        for reading in readings.iter().filter(|r| is_temperature(r)) {
            let samples = self.series.entry(sensor_key(reading)).or_default();
            if samples.back().is_some_and(|sample| sample.0 == time) {
                samples.pop_back();
            }
            samples.push_back((time, measured(reading)));
        }
        for samples in self.series.values_mut() {
            while samples.len() > HISTORY_LIMIT {
                samples.pop_front();
            }
        }
        // Devices that disappeared cannot grow this map without bound.
        self.series
            .retain(|_, samples| samples.iter().any(|(_, value)| value.is_some()));
        self.last_refresh = refreshed;
        self.sampled = true;
    }
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
    display_settings_open: bool,
    motion_enabled: bool,
    source_details_expanded: bool,
    history: TemperatureHistory,
    fan_editor: FanEditor,
}
impl DesktopApp {
    pub fn new(creation: &eframe::CreationContext<'_>) -> Result<Self, std::io::Error> {
        let mut preferences: Preferences = creation
            .storage
            .and_then(|s| eframe::get_value(s, "display_preferences"))
            .unwrap_or_default();
        preferences.sanitize();
        let mut app =
            Self::with_controller(Controller::new(crate::NativeBackend::new())?, preferences);
        app.motion_enabled = creation
            .storage
            .and_then(|s| eframe::get_value(s, "interface_motion"))
            .unwrap_or(true);
        app.configure(&creation.egui_ctx);
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
            display_settings_open: false,
            motion_enabled: true,
            source_details_expanded: false,
            history: TemperatureHistory::default(),
            fan_editor: FanEditor::default(),
        }
    }
    pub fn scroll_to_bottom(&mut self) {
        self.bottom_requested = true;
    }
    pub fn select_section(&mut self, section: Section) {
        self.section = section;
    }
    pub fn fan_editor(&self) -> &FanEditor {
        &self.fan_editor
    }
    pub fn fan_editor_mut(&mut self) -> &mut FanEditor {
        &mut self.fan_editor
    }
    pub fn open_display_settings(&mut self) {
        self.display_settings_requested = true;
    }
    /// Uses the same state as the Sensors page's expand/collapse action.
    pub fn set_source_details_expanded(&mut self, expanded: bool) {
        self.source_details_expanded = expanded;
    }
    fn configure(&self, ctx: &egui::Context) {
        let dark = match self.preferences.theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => ctx.system_theme().unwrap_or(egui::Theme::Dark) == egui::Theme::Dark,
        };
        let mut v = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let text = if dark {
            FOREGROUND
        } else {
            Color32::from_rgb(37, 39, 35)
        };
        let bg = if dark {
            BACKGROUND
        } else {
            Color32::from_rgb(241, 240, 233)
        };
        let panel = if dark {
            PANEL
        } else {
            Color32::from_rgb(251, 250, 245)
        };
        let line = if dark {
            Color32::from_rgb(59, 61, 56)
        } else {
            Color32::from_rgb(211, 213, 201)
        };
        v.panel_fill = bg;
        v.window_fill = panel;
        v.extreme_bg_color = bg;
        v.faint_bg_color = panel;
        v.override_text_color = Some(text);
        v.hyperlink_color = text;
        v.selection.bg_fill = if dark {
            Color32::from_rgb(78, 84, 66)
        } else {
            Color32::from_rgb(211, 221, 192)
        };
        v.selection.stroke = Stroke::new(1.0, text);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, line);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);
        v.widgets.inactive.weak_bg_fill = panel;
        v.widgets.inactive.bg_fill = panel;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, line);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, text);
        v.widgets.hovered.bg_fill = if dark {
            Color32::from_rgb(52, 55, 48)
        } else {
            Color32::from_rgb(228, 231, 218)
        };
        v.widgets.hovered.weak_bg_fill = v.widgets.hovered.bg_fill;
        v.widgets.hovered.bg_stroke = Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(112, 120, 98)
            } else {
                Color32::from_rgb(130, 139, 114)
            },
        );
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, text);
        v.widgets.active = v.widgets.hovered;
        v.widgets.open = v.widgets.hovered;
        ctx.set_visuals(v);
        ctx.set_zoom_factor(self.preferences.text_scale);
        ctx.style_mut_of(
            if dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            },
            |style| {
                style.spacing.item_spacing = Vec2::new(10.0, 9.0);
                style.spacing.button_padding = Vec2::new(14.0, 9.0);
                style.spacing.interact_size.y = 36.0;
                style.spacing.indent = 14.0;
                style.animation_time = if self.motion_enabled { 0.18 } else { 0.0 };
                style.wrap_mode = Some(egui::TextWrapMode::Wrap);
                style
                    .text_styles
                    .insert(egui::TextStyle::Heading, FontId::proportional(22.0));
                style
                    .text_styles
                    .insert(egui::TextStyle::Body, FontId::proportional(14.0));
                style
                    .text_styles
                    .insert(egui::TextStyle::Button, FontId::proportional(14.0));
                style
                    .text_styles
                    .insert(egui::TextStyle::Small, FontId::proportional(13.0));
                for widget in [
                    &mut style.visuals.widgets.inactive,
                    &mut style.visuals.widgets.hovered,
                    &mut style.visuals.widgets.active,
                    &mut style.visuals.widgets.open,
                ] {
                    widget.corner_radius = egui::CornerRadius::same(8);
                }
            },
        );
    }
    fn tick(&mut self, ctx: &egui::Context) {
        // A native close must not terminate the process between write, readback
        // and rollback. Defer closing while that transaction owns the worker.
        if ctx.input(|input| input.viewport().close_requested()) {
            self.controller.request_close();
            if !self.controller.close_ready() {
                self.pending_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        self.controller.poll();
        if self.pending_close && self.controller.close_ready() {
            self.pending_close = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let narrow_refresh = self.controller.uses_periodic_thermal_refresh();
        if self.preferences.auto_refresh
            && !self.controller.busy()
            && (narrow_refresh || !self.controller.has_pending_preview())
            && self.controller.thermal_error.is_none()
            && self.controller.worker_available
            && !self.pending_close
            && self.controller.thermal_refresh_due()
        {
            if narrow_refresh {
                let _ = self.controller.refresh_thermal();
            } else {
                // With no fan session or pending preview, retain complete
                // battery/AC/policy updates on both native platforms.
                let _ = self.controller.refresh();
            }
        }
        if self.controller.busy() {
            if !self.controller.oem_write_state_unknown() {
                ctx.request_repaint_after(Duration::from_millis(80));
            }
        } else if self.controller.fan_session_held
            || !self.controller.fan_helper_reaped
            || !self.controller.fan_cleanup_complete
            || self.pending_close
        {
            // Lifecycle events arrive independently of sensor sampling. Keep
            // draining those cached worker events while a helper may live;
            // this requests a frame, never a snapshot or a driver read.
            ctx.request_repaint_after(Duration::from_millis(200));
        } else if self.preferences.auto_refresh {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else if let Some(sample) = &self.controller.thermal_sample {
            // One deadline marks a manual sample stale; no repeated sampling
            // or continuous idle animation is introduced.
            if let Some(remaining) =
                crate::THERMAL_STALE_AFTER.checked_sub(sample.started_at.elapsed())
            {
                ctx.request_repaint_after(remaining);
            }
        }
    }

    // @mark mechrevo.console_ui role=presentation
    pub fn render(&mut self, root: &mut egui::Ui) -> RenderMetrics {
        appearance::install_fonts(root.ctx());
        self.configure(root.ctx());
        self.tick(root.ctx());
        self.history.observe_controller(&self.controller);
        if std::mem::take(&mut self.display_settings_requested) {
            self.display_settings_open = true;
        }
        let backdrop_rect = root.max_rect();
        appearance::paint_backdrop(root, backdrop_rect);
        let outer = if root.available_width() >= 900.0 {
            40.0
        } else {
            16.0
        };
        let mut metrics = RenderMetrics::default();
        egui::Frame::new().inner_margin(outer).show(root, |ui| {
            let width = ui.available_width().clamp(1.0, 1000.0);
            let side = ((ui.available_width() - width) * 0.5).max(0.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(side);
                ui.allocate_ui_with_layout(
                    Vec2::new(width, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(width);
                        metrics = self.main_content(ui);
                    },
                );
            });
        });
        self.last_metrics = metrics;
        metrics
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (section, label) in [
            (Section::Dashboard, "概览"),
            (Section::Controls, "控制"),
            (Section::Fans, "风扇"),
            (Section::Sensors, "传感器"),
            (Section::Catalog, "型号目录"),
        ] {
            let selected = self.section == section;
            let t = ui.ctx().animate_bool_with_time(
                ui.scope_id().with(("navigation", label)),
                selected,
                if self.motion_enabled { 0.2 } else { 0.0 },
            );
            let fill = if ui.visuals().dark_mode {
                Color32::from_white_alpha((18.0 * t) as u8)
            } else {
                Color32::from_black_alpha((11.0 * t) as u8)
            };
            let response = ui.add(
                egui::Button::new(RichText::new(label).size(13.0).color(if selected {
                    foreground(ui)
                } else {
                    secondary(ui)
                }))
                .selected(selected)
                .fill(fill)
                .stroke(Stroke::NONE)
                .corner_radius(9.0),
            );
            if response.clicked() {
                self.section = section;
            }
        }
    }

    fn main_content(&mut self, ui: &mut egui::Ui) -> RenderMetrics {
        // The outer centering row has zero spacing; content widgets must not
        // inherit that structural setting.
        ui.spacing_mut().item_spacing.x = 10.0;
        let mut bottom_marker = Rect::NOTHING;
        let mut heading_rect = Rect::NOTHING;
        let mut thermal_panel_rect = Rect::NOTHING;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("console-content")
            .auto_shrink([false, false]);
        if self.bottom_requested {
            scroll = scroll.vertical_scroll_offset(self.last_metrics.content_size.y.max(0.0));
        }
        self.bottom_requested = false;
        let output = scroll.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            self.header(ui);
            if self.display_settings_open {
                self.display_settings(ui);
            }
            self.feedback(ui);
            ui.add_space(8.0);
            heading_rect = ui.scope(|ui| self.editorial_heading(ui)).response.rect;
            ui.add_space(8.0);
            match self.section {
                Section::Dashboard => thermal_panel_rect = self.dashboard(ui),
                Section::Controls => self.oem_controls(ui),
                Section::Fans => self.fan_controls(ui),
                Section::Sensors => self.sensors(ui),
                Section::Catalog => self.catalog(ui),
            }
            ui.add_space(30.0);
            ui.separator();
            ui.add_space(6.0);
            bottom_marker = ui
                .label(
                    RichText::new("开发预览 0.1  /  只读监测 · 系统控制需预览、应用并回读")
                        .size(13.0)
                        .color(secondary(ui)),
                )
                .rect;
            ui.add_space(6.0);
        });
        RenderMetrics {
            content_size: output.content_size,
            viewport: output.inner_rect,
            bottom_marker,
            scroll_offset: output.state.clamped_offset(),
            heading_rect,
            thermal_panel_rect,
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let wide = ui.available_width() >= 760.0;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::hover());
            icon(ui.painter(), rect, Icon::Brand, foreground(ui));
            ui.label(RichText::new("MECHREVO").font(appearance::display_font(19.0)));
            ui.add_space(if wide { 20.0 } else { 6.0 });
            self.navigation(ui);
            if wide {
                // The actions remain direct children of this row. A nested
                // wrapping row would inherit the remaining sliver of width.
                // In a wrapping row available_width() means the maximum width
                // after wrapping, not the remaining space on this line.
                ui.add_space((ui.available_size_before_wrap().x - 280.0).max(8.0));
                self.header_actions(ui);
            }
        });
        if !wide {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| self.header_actions(ui));
        }
        ui.add_space(6.0);
        ui.separator();
    }

    fn header_actions(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 10.0;
        let response = ui.add_enabled(
            !self.controller.busy() && self.controller.worker_available && !self.pending_close,
            egui::Button::new(RichText::new("刷新").size(13.0))
                .min_size(Vec2::new(80.0, 34.0))
                .frame(false),
        );
        let rect = Rect::from_center_size(
            egui::pos2(response.rect.left() + 15.0, response.rect.center().y),
            Vec2::splat(13.0),
        );
        let rotation = if self.controller.busy() && self.motion_enabled {
            ui.input(|i| i.time) as f32 * 3.0
        } else {
            0.0
        };
        refresh_icon(ui.painter(), rect, secondary(ui), rotation);
        let narrow =
            self.controller.requires_thermal_refresh() || self.controller.thermal_error.is_some();
        if response.clicked() {
            let result = if narrow {
                self.controller.refresh_thermal()
            } else {
                self.controller.refresh()
            };
            if let Err(error) = result {
                self.controller.error = Some(mechrevo_console_core::ControlError::new(
                    "refresh_not_started",
                    format!("刷新未启动：{error:?}。请等待当前操作结束。"),
                ));
            }
        }
        response.on_hover_text(if narrow {
            "只读更新温度与转速；设备控制信息保持上次完整采集的时间。"
        } else {
            "重新采集完整设备信息、控制状态与传感器。"
        });
        sampling_switch(ui, &mut self.preferences.auto_refresh, self.motion_enabled).on_hover_text(
            "每 3 秒只读采集；预览期间暂停完整采集，Linux 温度与转速仍更新。采样失败后点刷新重试。",
        );
        if ui
            .add(egui::Button::new(RichText::new("显示设置").size(13.0)).frame(false))
            .clicked()
        {
            self.display_settings_open = !self.display_settings_open;
        }
    }

    fn editorial_heading(&self, ui: &mut egui::Ui) {
        let wide = ui.available_width() >= 660.0;
        let title = match self.section {
            Section::Dashboard => "设备概览",
            Section::Controls => "设备控制",
            Section::Fans => "风扇曲线",
            Section::Sensors => "传感器",
            Section::Catalog => "型号目录",
        };
        if wide {
            let width = ui.available_width();
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 20.0;
                ui.allocate_ui_with_layout(
                    Vec2::new(width * 0.43 - 10.0, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(width * 0.43 - 10.0);
                        ui.label(RichText::new(title).size(34.0));
                    },
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(width * 0.57 - 10.0, 0.0),
                    egui::Layout::top_down(egui::Align::Max),
                    |ui| {
                        ui.set_width(width * 0.57 - 10.0);
                        ui.add_space(3.0);
                        self.device_caption(ui);
                    },
                );
            });
        } else {
            ui.label(RichText::new(title).size(34.0));
            ui.add_space(5.0);
            self.device_caption(ui);
        }
    }

    fn device_caption(&self, ui: &mut egui::Ui) {
        let snapshot = self.controller.snapshot.as_ref();
        let product = snapshot
            .and_then(|s| s.identity.product_name.as_deref())
            .unwrap_or("设备信息将在采集后显示");
        let platform = snapshot.map(|s| platform_label(&s.platform)).unwrap_or("—");
        let board = snapshot
            .and_then(|s| s.identity.board_name.as_deref())
            .unwrap_or("型号待识别");
        ui.add(
            egui::Label::new(RichText::new(product).size(14.0).color(label_color(ui))).truncate(),
        )
        .on_hover_text(format!("{product}\n{platform} · {board}"));
        let status = if matches!(self.controller.activity(), Some((JobKind::Refresh, _)))
            && !self.controller.thermal_sampling()
            && self.controller.fan_activity().is_none()
        {
            "正在采集".to_owned()
        } else if let Some(last) = self.controller.last_refresh {
            if !self.preferences.auto_refresh {
                "已采集 · 手动更新".into()
            } else {
                let seconds = last.elapsed().as_secs();
                if seconds < 2 {
                    "刚刚更新".into()
                } else {
                    format!("{seconds} 秒前更新")
                }
            }
        } else {
            "等待采集".into()
        };
        ui.add(
            egui::Label::new(
                RichText::new(format!("{platform}  ·  设备信息 {status}"))
                    .size(13.0)
                    .color(secondary(ui)),
            )
            .truncate(),
        )
        .on_hover_text(format!("{platform} · {board}"));
    }

    fn display_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        surface(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("显示设置").strong());
                if ui.small_button("收起").clicked() {
                    self.display_settings_open = false;
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("主题");
                ui.selectable_value(&mut self.preferences.theme, Theme::Dark, "石墨");
                ui.selectable_value(&mut self.preferences.theme, Theme::Light, "浅色");
                ui.selectable_value(&mut self.preferences.theme, Theme::System, "跟随系统");
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("信息密度");
                ui.selectable_value(
                    &mut self.preferences.display_mode,
                    DisplayMode::Standard,
                    "标准",
                );
                ui.selectable_value(
                    &mut self.preferences.display_mode,
                    DisplayMode::Compact,
                    "紧凑",
                );
            });
            let label = ui.label(format!(
                "文字大小  {:.0}%",
                self.preferences.text_scale * 100.0
            ));
            ui.scope(|ui| {
                slider_style(ui);
                ui.add(
                    egui::Slider::new(&mut self.preferences.text_scale, 0.8..=1.6)
                        .step_by(0.05)
                        .trailing_fill(true)
                        .handle_shape(egui::style::HandleShape::Circle)
                        .show_value(false),
                )
                .labelled_by(label.id);
            });
            ui.checkbox(&mut self.motion_enabled, "界面过渡动画");
        });
    }

    fn dashboard(&mut self, ui: &mut egui::Ui) -> Rect {
        let thermal_rect = self.thermal_panel(ui);
        ui.add_space(18.0);
        if ui.available_width() >= 660.0 {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                ui.columns(2, |columns| {
                    for column in columns.iter_mut() {
                        column.spacing_mut().item_spacing.x = 10.0;
                    }
                    surface(&mut columns[0], |ui| self.power_controls(ui));
                    surface(&mut columns[1], |ui| self.brightness_controls(ui));
                });
            });
        } else {
            surface(ui, |ui| self.power_controls(ui));
            ui.add_space(14.0);
            surface(ui, |ui| self.brightness_controls(ui));
        }
        ui.add_space(24.0);
        self.power_source(ui);
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(14.0);
        self.identity(ui);
        egui::CollapsingHeader::new("接口能力与限制")
            .id_salt("hardware-capabilities")
            .show(ui, |ui| self.capabilities(ui));
        thermal_rect
    }

    fn thermal_panel(&self, ui: &mut egui::Ui) -> Rect {
        let readings = self
            .controller
            .thermal_sample
            .as_ref()
            .map(|sample| sample.readings.as_slice())
            .or_else(|| {
                self.controller
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.telemetry.as_slice())
            })
            .unwrap_or(&[]);
        let cpu = primary_temperature(readings, "CPU");
        let gpu = primary_temperature(readings, "GPU");
        let fans = fan_readings(readings);
        let cpu_fans: Vec<_> = fans
            .iter()
            .copied()
            .filter(|r| fan_component(r) == Some("CPU"))
            .collect();
        let gpu_fans: Vec<_> = fans
            .iter()
            .copied()
            .filter(|r| fan_component(r) == Some("GPU"))
            .collect();
        let other_fans: Vec<_> = fans
            .iter()
            .copied()
            .filter(|r| fan_component(r).is_none())
            .collect();
        ui.scope(|ui| {
            surface(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                // Set the content minimum before advancing the cursor: egui
                // measures this height from the current position.
                if ui.available_width() >= 720.0 {
                    ui.set_min_height(222.0);
                }
                // A text row does not need horizontal_wrapped's button height.
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 0.0),
                    egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 16.0;
                        ui.label(
                            RichText::new("温度与散热")
                                .size(14.0)
                                .color(label_color(ui)),
                        );
                        self.thermal_status(ui);
                    },
                );
                ui.add_space(8.0);
                let width = ui.available_width();
                if width >= 720.0 {
                    // Standard desktop view is 270 px including the material's
                    // margins. This is a minimum, never a clip: large text and
                    // additional sensors are free to make the panel taller.
                    let left = width * 0.46 - 14.0;
                    let right = width - left - 28.0;
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 28.0;
                        ui.allocate_ui_with_layout(
                            Vec2::new(left, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(left);
                                ui.columns(2, |columns| {
                                    let cpu_color = green(&columns[0]);
                                    let gpu_color = amber(&columns[1]);
                                    temperature_metric(&mut columns[0], "CPU", cpu, cpu_color);
                                    temperature_metric(&mut columns[1], "GPU", gpu, gpu_color);
                                    for fan in cpu_fans.iter().take(3) {
                                        component_fan_metric(&mut columns[0], fan);
                                    }
                                    for fan in gpu_fans.iter().take(3) {
                                        component_fan_metric(&mut columns[1], fan);
                                    }
                                });
                            },
                        );
                        ui.allocate_ui_with_layout(
                            Vec2::new(right, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(right);
                                self.temperature_chart(ui, cpu, gpu);
                            },
                        );
                    });
                } else {
                    if width >= 280.0 {
                        ui.columns(2, |columns| {
                            let cpu_color = green(&columns[0]);
                            let gpu_color = amber(&columns[1]);
                            temperature_metric(&mut columns[0], "CPU", cpu, cpu_color);
                            temperature_metric(&mut columns[1], "GPU", gpu, gpu_color);
                            for fan in cpu_fans.iter().take(3) {
                                component_fan_metric(&mut columns[0], fan);
                            }
                            for fan in gpu_fans.iter().take(3) {
                                component_fan_metric(&mut columns[1], fan);
                            }
                        });
                    } else {
                        temperature_metric(ui, "CPU", cpu, green(ui));
                        for fan in cpu_fans.iter().take(3) {
                            component_fan_metric(ui, fan);
                        }
                        ui.add_space(10.0);
                        temperature_metric(ui, "GPU", gpu, amber(ui));
                        for fan in gpu_fans.iter().take(3) {
                            component_fan_metric(ui, fan);
                        }
                    }
                    ui.add_space(8.0);
                    self.temperature_chart(ui, cpu, gpu);
                }
                if fans.is_empty() {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        small_icon(ui, Icon::Fan);
                        ui.label(
                            RichText::new("风扇转速未采集")
                                .size(13.0)
                                .color(secondary(ui)),
                        );
                    });
                } else if !other_fans.is_empty() {
                    // A generic fan1/fan2 index does not establish CPU/GPU
                    // ownership. Keep those readings together and unassigned.
                    ui.add_space(8.0);
                    ui.separator();
                    if ui.available_width() >= 520.0 {
                        let count = other_fans.len().min(3);
                        ui.columns(count, |columns| {
                            for (column, fan) in columns.iter_mut().zip(&other_fans) {
                                fan_metric(column, fan);
                            }
                        });
                    } else {
                        for fan in other_fans.iter().take(3) {
                            fan_metric(ui, fan);
                        }
                    }
                }
                let omitted = cpu_fans.len().saturating_sub(3)
                    + gpu_fans.len().saturating_sub(3)
                    + other_fans.len().saturating_sub(3);
                if omitted > 0 {
                    ui.label(
                        RichText::new(format!("另有 {omitted} 个风扇读数，见传感器页"))
                            .size(13.0)
                            .color(secondary(ui)),
                    );
                }
                if self.controller.thermal_error.is_some()
                    || self
                        .controller
                        .thermal_sample
                        .as_ref()
                        .is_some_and(|sample| !sample.diagnostics.is_empty())
                {
                    egui::CollapsingHeader::new("采样详情").show(ui, |ui| {
                        if let Some(error) = &self.controller.thermal_error {
                            ui.add(
                                egui::Label::new(format!("{} · {}", error.code, error.message))
                                    .wrap(),
                            );
                            for detail in &error.details {
                                ui.add(egui::Label::new(detail).wrap());
                            }
                            if let Some((started, finished)) = self
                                .controller
                                .thermal_last_attempt
                                .zip(self.controller.thermal_last_finished)
                            {
                                ui.label(format!(
                                    "失败采集耗时 {} ms",
                                    finished.saturating_duration_since(started).as_millis()
                                ));
                            }
                            ui.label("保留上次成功读数；点击刷新可显式重试。");
                        }
                        if let Some(sample) = &self.controller.thermal_sample {
                            for diagnostic in &sample.diagnostics {
                                ui.add(
                                    egui::Label::new(format!(
                                        "{} · {}",
                                        diagnostic.code, diagnostic.message
                                    ))
                                    .wrap(),
                                );
                            }
                        }
                    });
                }
            })
        })
        .response
        .rect
    }

    fn thermal_status(&self, ui: &mut egui::Ui) {
        let sample = self.controller.thermal_sample.as_ref();
        let stale =
            sample.is_some_and(|sample| sample.started_at.elapsed() > crate::THERMAL_STALE_AFTER);
        let failed = self.controller.thermal_error.is_some();
        let state = if self.controller.thermal_sampling() {
            if failed {
                "正在重试温度与转速采样"
            } else {
                "正在采集温度与转速"
            }
        } else if failed {
            if self.preferences.auto_refresh {
                "采样失败 · 自动采样已暂停"
            } else {
                "采样失败 · 手动采样"
            }
        } else if self.preferences.auto_refresh
            && self.controller.has_pending_preview()
            && !self.controller.uses_periodic_thermal_refresh()
        {
            "自动采样已暂停"
        } else if !self.preferences.auto_refresh {
            "手动采样"
        } else if sample.is_none() && self.controller.last_refresh.is_none() {
            "等待采样"
        } else {
            "自动采样"
        };
        let mut text = state.to_owned();
        if stale {
            text.push_str(" · 读数已陈旧");
        }
        if self.controller.thermal_sampling()
            && self
                .controller
                .thermal_last_attempt
                .is_some_and(|started| started.elapsed() > crate::THERMAL_STALE_AFTER)
        {
            text.push_str(" · 本次采集已超过 6 秒");
        }
        if let Some(sample) = sample {
            // Manual mode does not repaint a ticking counter. A stable age
            // band remains truthful when the window is idle; one deadline
            // invalidates the fresh band without starting another read.
            let age = if self.preferences.auto_refresh {
                format!("{} 秒前", sample.started_at.elapsed().as_secs())
            } else if stale {
                "≥6 秒前".into()
            } else {
                "6 秒内".into()
            };
            text.push_str(&format!(
                " · 采集起点 {age} · 采集耗时 {} ms",
                sample
                    .finished_at
                    .saturating_duration_since(sample.started_at)
                    .as_millis()
            ));
        }
        ui.label(RichText::new(text).size(13.0).color(if failed || stale {
            amber(ui)
        } else {
            secondary(ui)
        }));
    }

    fn temperature_chart(
        &self,
        ui: &mut egui::Ui,
        cpu: Option<&SensorReading>,
        gpu: Option<&SensorReading>,
    ) {
        let series = [(cpu, green(ui)), (gpu, amber(ui))];
        let data: Vec<_> = series
            .iter()
            .filter_map(|(r, c)| {
                r.and_then(|r| self.history.series.get(&sensor_key(r)))
                    .map(|samples| (samples, *c))
            })
            .collect();
        let count = data
            .iter()
            .map(|(s, _)| s.iter().filter(|(_, v)| v.is_some()).count())
            .max()
            .unwrap_or(0);
        // These labels show the current readout, not the last non-missing
        // history value. A suspended/disappeared GPU must not look live.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for (label, reading, color) in [("CPU", cpu, green(ui)), ("GPU", gpu, amber(ui))] {
                let endpoint = reading
                    .and_then(measured)
                    .map(|value| format!("{label} {value:.0}°C"))
                    .unwrap_or_else(|| format!("{label} —"));
                ui.label(RichText::new(endpoint).size(13.0).color(color));
            }
        });
        let width = ui.available_width().max(1.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 108.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let plot = Rect::from_min_max(
            rect.left_top() + Vec2::new(2.0, 6.0),
            rect.right_bottom() - Vec2::new(46.0, 6.0),
        );
        let times: Vec<_> = data
            .iter()
            .flat_map(|(samples, _)| samples.iter().map(|(time, _)| *time))
            .collect();
        let range = times
            .iter()
            .min()
            .zip(times.iter().max())
            .map(|(first, last)| (*first, *last));
        if count < 3 {
            // Keep this a semantic label, so the collection state is also
            // available to screen readers and keyboard-only users.
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new("正在积累趋势")
                            .size(13.0)
                            .color(secondary(ui)),
                    );
                });
            });
            painter.line_segment(
                [plot.left_bottom(), plot.right_bottom()],
                Stroke::new(1.0, divider(ui)),
            );
        } else if let Some((first, last)) = range {
            let duration = last.duration_since(first).as_secs_f32().max(0.001);
            let values: Vec<_> = data
                .iter()
                .flat_map(|(s, _)| s.iter().filter_map(|(_, v)| *v))
                .collect();
            let low =
                (values.iter().copied().fold(f64::INFINITY, f64::min) / 10.0).floor() * 10.0 - 5.0;
            let high = (values.iter().copied().fold(f64::NEG_INFINITY, f64::max) / 10.0).ceil()
                * 10.0
                + 5.0;
            for portion in [0.0, 0.5, 1.0] {
                let y = egui::lerp(plot.bottom()..=plot.top(), portion);
                painter.line_segment(
                    [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                    Stroke::new(1.0, divider(ui)),
                );
                painter.text(
                    egui::pos2(
                        rect.right() - 2.0,
                        if portion == 1.0 { y + 3.0 } else { y - 3.0 },
                    ),
                    if portion == 1.0 {
                        egui::Align2::RIGHT_TOP
                    } else {
                        egui::Align2::RIGHT_BOTTOM
                    },
                    format!("{:.0}°C", low + (high - low) * f64::from(portion)),
                    FontId::proportional(12.0),
                    secondary(ui),
                );
            }
            for (samples, color) in &data {
                if samples.iter().filter(|(_, value)| value.is_some()).count() < 3 {
                    continue;
                }
                let mut previous = None;
                for (time, value) in *samples {
                    let Some(value) = value else {
                        previous = None;
                        continue;
                    };
                    let point = egui::pos2(
                        plot.left()
                            + plot.width() * time.duration_since(first).as_secs_f32() / duration,
                        plot.bottom() - plot.height() * ((*value - low) / (high - low)) as f32,
                    );
                    if let Some(previous) = previous {
                        painter.line_segment([previous, point], Stroke::new(2.0, *color));
                    }
                    previous = Some(point);
                }
                if let Some(last) = previous {
                    painter.circle_filled(last, 3.0, *color);
                }
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(if count < 3 {
                    "采集至少 3 次后显示趋势".into()
                } else if let Some((first, last)) = range {
                    let seconds = last.duration_since(first).as_secs();
                    if seconds >= 60 {
                        format!("最近 {} 分 {:02} 秒", seconds / 60, seconds % 60)
                    } else {
                        format!("最近 {seconds} 秒")
                    }
                } else {
                    "正在积累趋势".into()
                })
                .size(13.0)
                .color(secondary(ui)),
            );
            if data
                .iter()
                .any(|(samples, _)| samples.iter().any(|(_, value)| value.is_none()))
            {
                ui.label(
                    RichText::new("存在采样间隔，空窗不连线")
                        .size(13.0)
                        .color(secondary(ui)),
                );
            }
        });
    }

    fn feedback(&self, ui: &mut egui::Ui) {
        if self.pending_close {
            ui.label(RichText::new("正在完成操作、恢复控制并等待后台结束。").color(amber(ui)));
        }
        if let Some((kind, elapsed)) = self.controller.activity() {
            if kind != JobKind::Refresh || elapsed > Duration::from_secs(10) {
                ui.label(
                    RichText::new(format!(
                        "{} · {:.1} 秒",
                        match kind {
                            JobKind::Refresh => "设备响应较慢，请等待",
                            JobKind::Preview => "正在准备操作预览",
                            JobKind::Apply => "正在应用并回读",
                        },
                        elapsed.as_secs_f32()
                    ))
                    .color(amber(ui)),
                );
            }
        }
        if let Some(error) = &self.controller.error {
            ui.add_space(12.0);
            surface(ui, |ui| {
                ui.label(
                    RichText::new(format!("操作未确认 · {}", error.message)).color(error_color(ui)),
                );
                egui::CollapsingHeader::new("错误详情").show(ui, |ui| {
                    for detail in &error.details {
                        ui.label(detail);
                    }
                    ui.label(format!(
                        "{} · 刷新核对；后台退出时请重新打开控制台。",
                        error.code
                    ));
                });
            });
        }
        if let Some(applied) = &self.controller.applied {
            ui.label(
                RichText::new(format!(
                    "已应用并回读：{} → {}",
                    applied.previous, applied.confirmed
                ))
                .color(green(ui)),
            );
        }
        if let Some(applied) = &self.controller.brightness_applied {
            ui.label(
                RichText::new(format!(
                    "亮度请求已回读：{} → {}",
                    applied.previous, applied.confirmed
                ))
                .color(green(ui)),
            );
        }
        if let Some(applied) = &self.controller.oem_applied {
            let label = self
                .controller
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot
                        .oem_controls
                        .iter()
                        .find(|control| control.id == applied.control_id)
                })
                .map(|control| control.label.as_str())
                .unwrap_or(&applied.control_id);
            ui.label(
                RichText::new(format!(
                    "{label} 请求已回读：{} → {}",
                    oem_value_text(&applied.previous),
                    oem_value_text(&applied.confirmed)
                ))
                .color(green(ui)),
            );
            ui.label(
                RichText::new(&applied.readback_scope)
                    .size(13.0)
                    .color(secondary(ui)),
            );
        }
    }

    fn oem_controls(&mut self, ui: &mut egui::Ui) {
        let controls = self
            .controller
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.oem_controls.clone())
            .unwrap_or_default();
        if controls.is_empty() {
            empty_state(ui, "未发现 OEM 控制接口", "本页只显示这台设备实际提供的接口。刷新后仍为空，表示当前平台尚未提供可用的型号控制。");
            return;
        }
        if self.controller.selected_oem_control.is_none() {
            self.controller.select_oem_control(&controls[0].id);
        }
        surface(ui, |ui| {
            ui.label(
                RichText::new("选择要调节的设备功能")
                    .size(14.0)
                    .color(label_color(ui)),
            );
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for control in &controls {
                    let selected = self.controller.selected_oem_control.as_deref()
                        == Some(control.id.as_str());
                    if ui
                        .add_enabled(
                            !self.controller.write_in_progress(),
                            egui::Button::selectable(selected, &control.label),
                        )
                        .clicked()
                    {
                        self.controller.select_oem_control(&control.id);
                    }
                }
            });
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(12.0);
            let Some(control) = self.controller.selected_oem().cloned() else {
                ui.label("所选设备接口已离线，请刷新并重新选择。");
                return;
            };
            ui.label(RichText::new(&control.label).size(18.0));
            ui.label(
                RichText::new(format!(
                    "当前  {}",
                    control
                        .current
                        .as_ref()
                        .map(oem_value_text)
                        .unwrap_or_else(|| "未采集".into())
                ))
                .color(secondary(ui)),
            );
            let adjustable =
                control.status == CapabilityStatus::Available && control.current.is_some();
            if adjustable {
                let mut target = self.controller.requested_oem_value.clone();
                ui.add_space(8.0);
                ui.add_enabled_ui(!self.controller.write_in_progress(), |ui| {
                    match (&control.domain, &mut target) {
                        (OemDomain::Boolean, Some(OemValue::Boolean(value))) => {
                            ui.checkbox(value, "目标状态");
                        }
                        (OemDomain::Choices(choices), Some(OemValue::Choice(value))) => {
                            ui.label(RichText::new("目标模式").size(13.0).color(secondary(ui)));
                            ui.horizontal_wrapped(|ui| {
                                for choice in choices {
                                    ui.selectable_value(
                                        value,
                                        choice.clone(),
                                        oem_choice_label(choice),
                                    )
                                    .on_hover_text(choice);
                                }
                            });
                        }
                        (OemDomain::Level { max }, Some(OemValue::Level(value))) => {
                            let label = ui.label(format!("目标级别  {value} / {max}"));
                            ui.scope(|ui| {
                                slider_style(ui);
                                ui.add(egui::Slider::new(value, 0..=*max).show_value(false))
                                    .labelled_by(label.id);
                            });
                        }
                        (OemDomain::Rgb, Some(OemValue::Rgb(rgb))) => {
                            ui.label(RichText::new("目标颜色").size(13.0).color(secondary(ui)));
                            ui.horizontal_wrapped(|ui| {
                                for (label, channel) in
                                    ["红", "绿", "蓝"].into_iter().zip(rgb.iter_mut())
                                {
                                    let label = ui.label(label);
                                    ui.add(egui::DragValue::new(channel).range(0..=255).speed(1.0))
                                        .labelled_by(label.id);
                                }
                                let (rect, _) = ui.allocate_exact_size(
                                    Vec2::new(30.0, 22.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    6.0,
                                    Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                                );
                            });
                        }
                        _ => {
                            ui.label("接口公布的值与范围不一致，请刷新后重新选择。");
                        }
                    }
                });
                if let Some(target) = target {
                    self.controller.set_oem_target(target);
                }
            }
            if control.kind == OemControlKind::ChargingProfile {
                ui.label(
                    RichText::new("这里选择充电模式，不代表固定的 60% 或 80% 充电上限。")
                        .size(13.0)
                        .color(secondary(ui)),
                );
            }
            if matches!(
                control.kind,
                OemControlKind::KeyboardBrightness | OemControlKind::KeyboardColor
            ) {
                ui.label(
                    RichText::new("键盘灯读数为请求 / 驱动状态，未测量实际发光效果。")
                        .size(13.0)
                        .color(secondary(ui)),
                );
            }
            ui.label(
                RichText::new(&control.readback_scope)
                    .size(13.0)
                    .color(secondary(ui)),
            );
            let reason = self.controller.oem_unavailable_reason().map(str::to_owned);
            if let Some(reason) = &reason {
                unavailable(ui, reason);
            }
            if let Some(plan) = &self.controller.oem_preview {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "{} → {}",
                        oem_value_text(&plan.previous),
                        oem_value_text(&plan.target)
                    ))
                    .color(green(ui)),
                );
                if let Some(brightness) = plan.companion_brightness {
                    ui.label(format!("伴随键盘亮度  {brightness}"));
                }
                if plan.requires_privilege {
                    ui.label(
                        RichText::new("应用时请求管理员授权")
                            .size(13.0)
                            .color(secondary(ui)),
                    );
                }
                ui.label(
                    RichText::new(&plan.readback_scope)
                        .size(13.0)
                        .color(secondary(ui)),
                );
            }
            egui::CollapsingHeader::new("接口详情")
                .id_salt(("oem-interface-details", &control.id))
                .show(ui, |ui| {
                    ui.label(&control.id);
                    evidence(ui, &control.source.backend, &control.source.path);
                    ui.label(
                        RichText::new(&control.binding)
                            .size(13.0)
                            .color(secondary(ui)),
                    );
                });
            if reason.is_none() || self.controller.oem_preview.is_some() {
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            reason.is_none() && !self.controller.busy(),
                            egui::Button::new("预览更改"),
                        )
                        .clicked()
                    {
                        let _ = self.controller.preview_oem();
                    }
                    if self.controller.oem_preview.is_some()
                        && ui
                            .add_enabled(
                                reason.is_none() && !self.controller.busy(),
                                primary_button(ui, "应用并回读"),
                            )
                            .clicked()
                    {
                        if let Err(error) = self.controller.apply_oem_preview() {
                            self.controller.error = Some(mechrevo_console_core::ControlError::new(
                                "oem_preview_invalidated",
                                format!("设备请求或接口已变化，请重新预览：{error:?}"),
                            ));
                        }
                    }
                });
            }
        });
    }

    fn publish_fan_edit(&mut self) {
        match self.fan_editor.take_update() {
            Some(FanEditorUpdate::Import(input)) => {
                let _ = self.controller.import_fan_curve(&input);
            }
            Some(FanEditorUpdate::Edit(curve)) => {
                let _ = self.controller.set_fan_draft(curve);
            }
            None => {}
        }
    }

    fn fan_controls(&mut self, ui: &mut egui::Ui) {
        use mechrevo_console_core::fan_curve::{broker::BrokerPhase, RawFanTable, TABLE_BYTES};
        self.publish_fan_edit();
        if self.fan_editor.curve().is_none() {
            if let Some(draft) = &self.controller.fan_draft {
                self.fan_editor.adopt_draft(draft);
            }
        }
        surface(ui, |ui| {
            let phase = self.controller.fan_state.as_ref().map(|state| state.phase);
            let state_unconfirmed = self
                .controller
                .fan_state
                .as_ref()
                .is_some_and(|state| !state.state_is_current || state.live_error != 0);
            let status = if state_unconfirmed {
                "当前风扇状态未确认，请刷新或恢复"
            } else {
                match phase {
                    Some(BrokerPhase::Idle) => "风扇接口已就绪",
                    Some(BrokerPhase::Acquired) if self.controller.fan_session_held => {
                        "已取得控制，尚未应用曲线"
                    }
                    Some(BrokerPhase::Acquired) => "风扇接口正由其他会话占用",
                    Some(BrokerPhase::Active) if self.controller.fan_session_held => {
                        "本会话曲线控制正在运行"
                    }
                    Some(BrokerPhase::Active) => "其他会话正在控制风扇",
                    Some(BrokerPhase::RecoveryRequired) => "需要恢复原控制",
                    None if self.controller.fan_error.is_some() => "当前风扇接口不可用",
                    None => "尚未检查风扇接口",
                }
            };
            ui.label(RichText::new(status).size(18.0).color(
                if state_unconfirmed || phase == Some(BrokerPhase::RecoveryRequired) {
                    error_color(ui)
                } else {
                    foreground(ui)
                },
            ));
            if self.controller.fan_session_held {
                ui.label(
                    RichText::new("本会话保持风扇控制权；恢复旧表后才会释放。")
                        .size(13.0)
                        .color(secondary(ui)),
                );
            } else if !state_unconfirmed
                && phase == Some(BrokerPhase::Idle)
                && self.controller.fan_helper_reaped
                && self.controller.fan_cleanup_complete
            {
                ui.label(
                    RichText::new("控制权已释放，辅助程序已退出。")
                        .size(13.0)
                        .color(secondary(ui)),
                );
            }
            if self.controller.fan_restoration_confirmed {
                ui.label(
                    RichText::new("本次恢复事务曾回读确认旧硬件表与控制状态。")
                        .size(13.0)
                        .color(amber(ui)),
                );
            }
            if self.controller.fan_state.is_none() {
                ui.label(
                    RichText::new(
                        "检查只读取驱动状态。没有此版本接口时仍可编辑草稿，无法应用到设备。",
                    )
                    .size(13.0)
                    .color(secondary(ui)),
                );
            }
            if let Some(error) = &self.controller.fan_error {
                ui.label(
                    RichText::new(&error.message)
                        .size(14.0)
                        .color(error_color(ui)),
                );
                egui::CollapsingHeader::new("风扇错误详情")
                    .id_salt("fan-error-details")
                    .show(ui, |ui| {
                        ui.label(RichText::new(&error.code).size(13.0).color(secondary(ui)));
                        for detail in &error.details {
                            ui.label(detail);
                        }
                    });
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let label = if self.controller.fan_state.is_some() {
                    "刷新风扇状态"
                } else {
                    "检查风扇接口"
                };
                if ui
                    .add_enabled(!self.controller.busy(), egui::Button::new(label))
                    .clicked()
                {
                    let result = if self.controller.fan_session_held {
                        self.controller.refresh_fan()
                    } else {
                        self.controller.probe_fan()
                    };
                    if let Err(error) = result {
                        self.controller.fan_error = Some(mechrevo_console_core::ControlError::new(
                            "fan_request_unavailable",
                            format!("暂不能检查风扇状态：{error:?}"),
                        ));
                    }
                }
                let needs_recovery = self.controller.fan_session_held
                    || !self.controller.fan_helper_reaped
                    || !self.controller.fan_cleanup_complete
                    || phase == Some(BrokerPhase::RecoveryRequired);
                if needs_recovery
                    && ui
                        .add_enabled(!self.controller.busy(), egui::Button::new("恢复旧表并释放"))
                        .clicked()
                {
                    if let Err(error) = self.controller.restore_fan() {
                        self.controller.fan_error = Some(mechrevo_console_core::ControlError::new(
                            "fan_restore_unavailable",
                            format!("暂不能提交恢复请求：{error:?}"),
                        ));
                    }
                }
            });
        });
        ui.add_space(16.0);
        surface(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("曲线草稿").size(18.0));
                let status = match (&self.controller.fan_draft, &self.controller.fan_applied) {
                    (Some(draft), Some(applied)) if draft == applied => "与已应用曲线一致",
                    (Some(_), Some(_)) => "有尚未应用的更改",
                    (Some(_), None) => "尚未应用",
                    _ => "等待导入",
                };
                ui.label(RichText::new(status).size(13.0).color(amber(ui)));
            });
            self.fan_editor.show(
                ui,
                !self.controller.write_in_progress() && !self.pending_close,
                self.controller.fan_applied.as_ref(),
            );
        });
        // An edit is published before handling any preview/apply action from
        // this frame, so it cannot reuse the preceding curve's confirmation.
        self.publish_fan_edit();
        ui.add_space(16.0);
        surface(ui, |ui| {
            ui.label(RichText::new("预览与应用").size(18.0));
            if let Some(preview) = &self.controller.fan_preview {
                ui.label(RichText::new("草稿格式有效，尚未写入设备。").size(14.0));
                ui.label(format!(
                    "目标模式  {}",
                    if preview.curve().respective {
                        "独立控制"
                    } else {
                        "同步控制"
                    }
                ));
                egui::CollapsingHeader::new("查看草稿差异").id_salt("fan-draft-diff").show(ui, |ui| {
                    if let Some(state) = self.controller.fan_state.as_ref().filter(|state| state.state_is_current && state.live_error == 0) {
                        if let Some(raw) = state.live_table.as_ref().and_then(|bytes| <[u8; TABLE_BYTES]>::try_from(bytes.as_slice()).ok()) {
                            let original = RawFanTable::from_bytes(raw);
                            let target = original.with_curve(preview);
                            let changed = original.as_bytes().iter().zip(target.as_bytes()).filter(|(left, right)| left != right).count();
                            ui.label(format!("当前硬件表中将改动 {changed} 个曲线字节；恢复始终使用完整原表。"));
                        }
                    }
                    if let Some(applied) = &self.controller.fan_applied {
                        for (name, previous, next) in [("CPU", &applied.curve().cpu, &preview.curve().cpu), ("GPU", &applied.curve().gpu, &preview.curve().gpu)] {
                            for stage in 0..16 {
                                if previous.duty_raw[stage] != next.duty_raw[stage] {
                                    ui.label(format!("{name} 第 {} 档占空比  {:.1}% → {:.1}%", stage + 1, f32::from(previous.duty_raw[stage]) / 2.0, f32::from(next.duty_raw[stage]) / 2.0));
                                }
                                if stage < 15 && (previous.rise_celsius[stage] != next.rise_celsius[stage] || previous.fall_celsius[stage] != next.fall_celsius[stage]) {
                                    ui.label(format!("{name} 第 {} 档温界  升温 {} → {} °C，降温 {} → {} °C", stage + 1, previous.rise_celsius[stage], next.rise_celsius[stage], previous.fall_celsius[stage], next.fall_celsius[stage]));
                                }
                            }
                        }
                    } else {
                        ui.label("尚无已核验的应用曲线可做逐档对比。当前原表只作为完整恢复材料保留，不转换为编辑输入。");
                    }
                });
            } else {
                ui.label(
                    RichText::new("先预览当前草稿，再申请应用。预览不需要管理员授权。")
                        .size(14.0)
                        .color(secondary(ui)),
                );
            }
            let valid = self.fan_editor.is_valid() && self.controller.fan_draft.is_some();
            let unavailable = self
                .controller
                .fan_apply_unavailable_reason()
                .map(str::to_owned);
            let available = unavailable.is_none();
            if let Some(reason) = unavailable {
                ui.label(RichText::new(reason).size(13.0).color(secondary(ui)));
            }
            ui.label(
                RichText::new("点击应用时才请求管理员授权。硬件原表逐字节保存，恢复不依赖草稿。")
                    .size(13.0)
                    .color(secondary(ui)),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        valid && !self.controller.busy() && !self.pending_close,
                        egui::Button::new("预览曲线"),
                    )
                    .clicked()
                {
                    let _ = self.controller.preview_fan();
                }
                if self.controller.fan_preview.is_some()
                    && ui
                        .add_enabled(
                            valid && available && !self.controller.busy() && !self.pending_close,
                            primary_button(ui, "应用曲线"),
                        )
                        .clicked()
                {
                    if let Err(error) = self.controller.apply_fan() {
                        self.controller.fan_error = Some(mechrevo_console_core::ControlError::new(
                            "fan_apply_unavailable",
                            format!("暂不能应用草稿，请重新预览：{error:?}"),
                        ));
                    }
                }
            });
        });
    }

    fn power_controls(&mut self, ui: &mut egui::Ui) {
        heading_with_icon(ui, "系统电源策略", Icon::Power);
        let current = self
            .controller
            .snapshot
            .as_ref()
            .and_then(|s| s.power_policy.current.as_deref())
            .unwrap_or("未采集");
        ui.label(RichText::new(format!("当前  {}", profile_label(current))).color(secondary(ui)));
        let mut selected = self.controller.selected_profile;
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            for (profile, label) in [
                (PowerProfile::PowerSaver, "节能"),
                (PowerProfile::Balanced, "平衡"),
                (PowerProfile::Performance, "性能"),
            ] {
                let supported = self.controller.snapshot.as_ref().is_some_and(|s| {
                    s.capabilities.iter().any(|cap| {
                        cap.id == "power_profile" && cap.status == CapabilityStatus::Available
                    }) && mechrevo_console_core::profile_available(s, profile)
                });
                if supported
                    && ui
                        .add(egui::Button::selectable(selected == profile, label))
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
            unavailable(ui, reason);
        }
        if let Some(plan) = &self.controller.preview {
            ui.label(
                RichText::new(format!(
                    "{} → {}",
                    profile_label(&plan.previous),
                    profile_label(&plan.target)
                ))
                .color(green(ui)),
            );
        }
        if reason.is_none() || self.controller.preview.is_some() {
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
                if self.controller.preview.is_some()
                    && ui
                        .add_enabled(
                            reason.is_none() && !self.controller.busy(),
                            primary_button(ui, "应用并回读"),
                        )
                        .clicked()
                {
                    let _ = self.controller.apply_preview();
                }
            });
        }
        if let Some(plan) = &self.controller.preview {
            egui::CollapsingHeader::new("操作范围").show(ui, |ui| {
                ui.label(&plan.scope);
                evidence(ui, &plan.source.backend, &plan.source.path);
            });
        }
    }

    // @mark mechrevo.display_brightness role=presentation
    fn brightness_controls(&mut self, ui: &mut egui::Ui) {
        heading_with_icon(ui, "屏幕亮度", Icon::Sun);
        let mut selected = self.controller.selected_brightness_device.clone();
        if let Some(snapshot) = &self.controller.snapshot {
            for device in &snapshot.brightness {
                if snapshot.brightness.len() > 1 {
                    if ui
                        .selectable_label(
                            selected.as_deref() == Some(device.id.as_str()),
                            &device.label,
                        )
                        .clicked()
                    {
                        selected = Some(device.id.clone());
                    }
                } else {
                    ui.label(RichText::new(&device.label).small().color(secondary(ui)));
                }
            }
        }
        if let Some(device) = selected {
            self.controller.select_brightness_device(&device);
        }
        let device = self.controller.snapshot.as_ref().and_then(|s| {
            s.brightness.iter().find(|d| {
                Some(d.id.as_str()) == self.controller.selected_brightness_device.as_deref()
            })
        });
        let has_device = device.is_some();
        let levels = device
            .map(|d| d.supported_percentages.clone())
            .unwrap_or_default();
        if let Some(device) = device {
            let requested = device
                .control_value
                .zip(device.maximum)
                .filter(|(_, m)| *m > 0)
                .map(|(v, m)| format!("{:.0}%", f64::from(v) * 100.0 / f64::from(m)))
                .unwrap_or_else(|| "未采集".into());
            let actual = device
                .current_percent
                .filter(|v| v.is_finite())
                .map(|v| format!("{v:.0}%"))
                .unwrap_or_else(|| "未采集".into());
            ui.label(
                RichText::new(format!("请求 {requested}  ·  驱动报告 {actual}"))
                    .small()
                    .color(secondary(ui)),
            );
        }
        let mut percent = self.controller.requested_brightness;
        let adjustable = device.is_some_and(|device| {
            device.status == CapabilityStatus::Available && device.readable && device.writable
        });
        if adjustable {
            if levels.is_empty() {
                let label = ui.label(format!("目标亮度  {percent}%"));
                ui.scope(|ui| {
                    slider_style(ui);
                    ui.add_enabled(
                        adjustable,
                        egui::Slider::new(&mut percent, 0..=100)
                            .trailing_fill(true)
                            .handle_shape(egui::style::HandleShape::Circle)
                            .show_value(false),
                    )
                    .labelled_by(label.id);
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    for level in &levels {
                        ui.selectable_value(&mut percent, *level, format!("{level}%"));
                    }
                });
            }
        }
        self.controller.set_brightness_target(percent);
        let reason = self
            .controller
            .brightness_unavailable_reason()
            .map(str::to_owned);
        if let Some(reason) = &reason {
            unavailable(ui, reason);
        }
        if let Some(plan) = &self.controller.brightness_preview {
            ui.label(
                RichText::new(format!(
                    "待应用 {}% · 原始值 {} → {}",
                    plan.requested_percent, plan.previous, plan.target
                ))
                .color(green(ui)),
            );
        }
        if reason.is_none() || self.controller.brightness_preview.is_some() {
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
                if self.controller.brightness_preview.is_some()
                    && ui
                        .add_enabled(
                            reason.is_none() && !self.controller.busy(),
                            primary_button(ui, "应用并回读"),
                        )
                        .clicked()
                {
                    let _ = self.controller.apply_brightness_preview();
                }
            });
        }
        if has_device || self.controller.brightness_applied.is_some() {
            egui::CollapsingHeader::new("背光设备详情")
                .id_salt("brightness-details")
                .show(ui, |ui| {
                    if let Some(snapshot) = &self.controller.snapshot {
                        for device in &snapshot.brightness {
                            ui.label(&device.id);
                            ui.label(format!(
                                "请求值 {:?} · 驱动报告值 {:?} · 上限 {:?}",
                                device.control_value, device.actual_value, device.maximum
                            ));
                            evidence(ui, &device.source.backend, &device.source.path);
                        }
                    }
                    if let Some(applied) = &self.controller.brightness_applied {
                        ui.label(format!(
                            "驱动报告 actual_brightness：{:?}；可能延迟，并非实测亮度。",
                            applied.actual_value
                        ));
                    }
                });
        }
    }

    fn power_source(&self, ui: &mut egui::Ui) {
        heading_with_icon(ui, "供电与电池", Icon::Battery);
        let readings = self
            .controller
            .snapshot
            .as_ref()
            .map(|s| s.telemetry.as_slice())
            .unwrap_or(&[]);
        let mut any = false;
        for reading in readings
            .iter()
            .filter(|r| r.kind == "external_power_online" || r.kind.starts_with("battery_"))
        {
            any = true;
            let value = if reading.kind == "external_power_online"
                && reading.status == CapabilityStatus::Available
            {
                match reading.value {
                    Some(1.0) => "电源已连接".into(),
                    Some(0.0) => "电源未连接".into(),
                    _ => "未采集".into(),
                }
            } else {
                reading_text(reading)
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(&reading.label);
                ui.label(RichText::new(value).strong());
            })
            .response
            .on_hover_text(format!(
                "{} · {}",
                reading.source.backend, reading.source.path
            ));
        }
        if !any {
            ui.label(RichText::new("未采集到供电或电池读数").color(secondary(ui)));
        }
        if let Some(s) = &self.controller.snapshot {
            for d in s.diagnostics.iter().filter(|d| d.code == "battery_status") {
                ui.label(RichText::new(&d.message).small().color(secondary(ui)));
            }
        }
    }

    fn identity(&self, ui: &mut egui::Ui) {
        heading_with_icon(ui, "这台电脑", Icon::Laptop);
        let Some(s) = &self.controller.snapshot else {
            ui.label("刷新后显示型号与系统信息。");
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(s.identity.board_name.as_deref().unwrap_or("主板型号未知"));
            ui.label(RichText::new(platform_label(&s.platform)).color(secondary(ui)));
            if let Some(bios) = &s.identity.bios_version {
                ui.label(RichText::new(format!("BIOS {bios}")).color(secondary(ui)));
            }
        });
        let identification = mechrevo_console_core::registry::identify(&s.identity);
        ui.label(
            RichText::new(match identification.status.as_str() {
                "hardware_partial" => "型号已匹配 · 部分功能有实机记录",
                "identified" => "型号已匹配 · 当前 BIOS 待验证",
                "ambiguous" => "型号存在多个匹配 · 专有控制未启用",
                _ => "型号待核验 · 专有控制未启用",
            })
            .small()
            .color(secondary(ui)),
        );
        egui::CollapsingHeader::new("设备信息与识别依据")
            .id_salt("identity-evidence")
            .show(ui, |ui| {
                ui.label(format!(
                    "厂商：{}",
                    s.identity.manufacturer.as_deref().unwrap_or("未知")
                ));
                ui.label(s.identity.product_name.as_deref().unwrap_or("产品名称未知"));
                ui.label(&identification.reason);
                for matched in &identification.matches {
                    ui.label(format!("{} · {}", matched.model_id, matched.title));
                    for source in &matched.sources {
                        if source.starts_with("https://") {
                            ui.hyperlink(source);
                        } else {
                            ui.label(source);
                        }
                    }
                }
                ui.label(&s.identity.support_note);
            });
    }

    fn capabilities(&self, ui: &mut egui::Ui) {
        let Some(s) = &self.controller.snapshot else {
            ui.label("检测后显示系统接口能力。");
            return;
        };
        for cap in &s.capabilities {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&cap.label).strong());
                ui.label(
                    RichText::new(match cap.status {
                        CapabilityStatus::Available => "已检测接口",
                        CapabilityStatus::Unsupported => "未支持",
                        CapabilityStatus::Unavailable => "不可用",
                    })
                    .small()
                    .color(secondary(ui)),
                );
            });
            ui.label(&cap.reason);
            ui.add_space(8.0);
        }
        ui.label("外置水冷尚无经过验证的控制协议与恢复流程。");
    }

    fn sensors(&mut self, ui: &mut egui::Ui) {
        let readings = self
            .controller
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.telemetry.as_slice())
            .or_else(|| {
                self.controller
                    .thermal_sample
                    .as_ref()
                    .map(|sample| sample.readings.as_slice())
            });
        let Some(readings) = readings else {
            empty_state(ui, "还没有设备读数", "点击刷新，开始读取系统公开的传感器。");
            return;
        };
        self.thermal_status(ui);
        if readings.is_empty() {
            empty_state(
                ui,
                "没有可用的传感器",
                "可尝试刷新。未采集的数据会保持未知，不显示为零。",
            );
        } else if ui
            .button(if self.source_details_expanded {
                "收起来源"
            } else {
                "展开来源"
            })
            .clicked()
        {
            self.source_details_expanded = !self.source_details_expanded;
        }
        for reading in readings {
            ui.push_id((&reading.device_id, &reading.id), |ui| {
                ui.add_space(8.0);
                ui.add(egui::Label::new(RichText::new(&reading.label).strong()).wrap());
                ui.label(RichText::new(reading_text(reading)).size(20.0).color(
                    if measured(reading).is_some() {
                        foreground(ui)
                    } else {
                        secondary(ui)
                    },
                ));
                if let Some(reason) = &reading.reason {
                    ui.label(RichText::new(reason).small().color(secondary(ui)));
                }
                if self.source_details_expanded {
                    ui.indent("source-details", |ui| {
                        ui.add(egui::Label::new(format!("设备：{}", reading.device_id)).wrap());
                        evidence(ui, &reading.source.backend, &reading.source.path);
                    });
                }
                ui.add_space(6.0);
                ui.separator();
            });
        }
        let diagnostics = self
            .controller
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.diagnostics.as_slice())
            .unwrap_or(&[]);
        if !diagnostics.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new("检测信息").strong());
            for d in diagnostics {
                ui.label(format!("{} · {}", d.code, d.message));
            }
        }
        if let Some(sample) = &self.controller.thermal_sample {
            for diagnostic in &sample.diagnostics {
                ui.add(
                    egui::Label::new(format!("{} · {}", diagnostic.code, diagnostic.message))
                        .wrap(),
                );
            }
        }
        if let Some(error) = &self.controller.thermal_error {
            ui.add(egui::Label::new(format!("采样失败：{}", error.message)).wrap());
        }
    }

    fn catalog(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.catalog_query)
                .hint_text("搜索型号、系列或名称")
                .desired_width(f32::INFINITY)
                .margin(12.0),
        );
        let models = self.controller.matching_models(&self.catalog_query);
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!("{} 个产品条目", models.len()))
                .small()
                .color(secondary(ui)),
        );
        for model in models {
            ui.push_id(&model.id, |ui| {
                ui.add_space(12.0);
                ui.label(RichText::new(&model.name).size(16.0).strong());
                ui.label(
                    RichText::new(format!(
                        "{} · {} · 兼容性待验证",
                        model.id,
                        model.series.as_deref().unwrap_or("系列未注明")
                    ))
                    .small()
                    .color(secondary(ui)),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to("官方产品页", &model.url);
                });
                if self.preferences.display_mode == DisplayMode::Standard {
                    egui::CollapsingHeader::new("收录说明").show(ui, |ui| {
                        ui.label(&model.support_note);
                    });
                }
                ui.add_space(10.0);
                ui.separator();
            });
        }
    }
}

impl eframe::App for DesktopApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "display_preferences", &self.preferences);
        eframe::set_value(storage, "interface_motion", &self.motion_enabled);
    }
}

fn foreground(ui: &egui::Ui) -> Color32 {
    ui.visuals().text_color()
}
fn secondary(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        SUBTLE
    } else {
        Color32::from_rgb(103, 106, 108)
    }
}
fn label_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(209, 211, 212)
    } else {
        Color32::from_rgb(68, 72, 75)
    }
}

/// One focusable click target retains egui's pointer, Space/Enter and
/// accessibility handling; its visual track is deliberately smaller.
fn sampling_switch(ui: &mut egui::Ui, enabled: &mut bool, motion: bool) -> egui::Response {
    let text = ui.painter().layout_no_wrap(
        "自动采样".into(),
        FontId::proportional(13.0),
        label_color(ui),
    );
    let (rect, mut response) = ui.allocate_exact_size(
        Vec2::new(30.0 + 8.0 + text.size().x, 34.0),
        egui::Sense::click(),
    );
    if response.clicked() {
        *enabled = !*enabled;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *enabled, "自动采样")
    });
    if ui.is_rect_visible(rect) {
        let track = Rect::from_center_size(
            egui::pos2(rect.left() + 15.0, rect.center().y),
            Vec2::new(30.0, 17.0),
        );
        let fill = if *enabled {
            if ui.visuals().dark_mode {
                Color32::from_rgb(201, 207, 197)
            } else {
                Color32::from_rgb(89, 101, 81)
            }
        } else if ui.visuals().dark_mode {
            Color32::from_rgb(68, 71, 73)
        } else {
            Color32::from_rgb(199, 202, 201)
        };
        ui.painter().rect_filled(track, 9.0, fill);
        let t =
            ui.ctx()
                .animate_bool_with_time(response.id, *enabled, if motion { 0.16 } else { 0.0 });
        ui.painter().circle_filled(
            egui::pos2(
                egui::lerp(track.left() + 8.5..=track.right() - 8.5, t),
                track.center().y,
            ),
            6.0,
            if *enabled && ui.visuals().dark_mode {
                BACKGROUND
            } else {
                Color32::WHITE
            },
        );
        ui.painter().galley(
            egui::pos2(track.right() + 8.0, rect.center().y - text.size().y * 0.5),
            text,
            label_color(ui),
        );
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                8.0,
                Stroke::new(1.0, label_color(ui)),
                egui::StrokeKind::Inside,
            );
        }
    }
    response
}
fn green(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        GREEN
    } else {
        Color32::from_rgb(69, 105, 44)
    }
}
fn amber(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        AMBER
    } else {
        Color32::from_rgb(137, 100, 31)
    }
}
fn error_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        ERROR
    } else {
        Color32::from_rgb(154, 46, 34)
    }
}
fn divider(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(51, 54, 49)
    } else {
        Color32::from_rgb(215, 217, 205)
    }
}
fn surface<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    appearance::glass(ui, contents)
}
fn slider_style(ui: &mut egui::Ui) {
    let dark = ui.visuals().dark_mode;
    ui.spacing_mut().slider_width = ui.available_width().max(1.0);
    ui.spacing_mut().slider_rail_height = 6.0;
    let rail = if dark {
        Color32::from_rgb(117, 124, 104)
    } else {
        Color32::from_rgb(133, 141, 118)
    };
    let fill = green(ui);
    let foreground = foreground(ui);
    let visuals = ui.visuals_mut();
    visuals.disabled_alpha = 0.75;
    visuals.widgets.inactive.bg_fill = rail;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.5, foreground);
    visuals.selection.bg_fill = fill;
    visuals.widgets.hovered.bg_fill = foreground;
    visuals.widgets.hovered.fg_stroke = Stroke::new(2.0, fill);
    visuals.widgets.active = visuals.widgets.hovered;
}
fn primary_button(ui: &egui::Ui, label: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label).color(if ui.visuals().dark_mode {
        BACKGROUND
    } else {
        Color32::WHITE
    }))
    .fill(if ui.visuals().dark_mode {
        FOREGROUND
    } else {
        Color32::from_rgb(50, 57, 43)
    })
    .stroke(Stroke::NONE)
}
fn unavailable(ui: &mut egui::Ui, reason: &str) {
    let concise = reason.split(['。', '；', '\n']).next().unwrap_or(reason);
    let summary: String = concise.chars().take(38).collect();
    let summary = if concise.chars().count() > 38 {
        format!("{summary}…")
    } else {
        summary
    };
    ui.label(RichText::new(summary).size(13.0).color(secondary(ui)));
    egui::CollapsingHeader::new(RichText::new("查看原因").size(13.0).color(secondary(ui)))
        .id_salt(reason)
        .show(ui, |ui| {
            ui.label(RichText::new(reason).small().color(secondary(ui)));
        });
}
fn evidence(ui: &mut egui::Ui, backend: &str, path: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(format!("{backend} · {path}"))
                .small()
                .color(secondary(ui)),
        )
        .wrap(),
    );
}
fn heading_with_icon(ui: &mut egui::Ui, title: &str, glyph: Icon) {
    ui.horizontal_wrapped(|ui| {
        small_icon(ui, glyph);
        ui.label(RichText::new(title).size(16.0).strong());
    });
}
fn small_icon(ui: &mut egui::Ui, glyph: Icon) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
    icon(ui.painter(), rect, glyph, secondary(ui));
}
fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.add_space(28.0);
    ui.label(RichText::new(title).size(20.0).strong());
    ui.label(RichText::new(detail).color(secondary(ui)));
    ui.add_space(28.0);
}
fn platform_label(platform: &str) -> &str {
    match platform {
        "linux" => "Linux",
        "windows" => "Windows",
        _ => platform,
    }
}
fn profile_label(profile: &str) -> &str {
    match profile {
        "balanced" => "平衡",
        "performance" => "性能",
        "power-saver" | "low-power" => "节能",
        _ => profile,
    }
}
fn oem_value_text(value: &OemValue) -> String {
    match value {
        OemValue::Boolean(true) => "开启".into(),
        OemValue::Boolean(false) => "关闭".into(),
        OemValue::Choice(choice) => oem_choice_label(choice).into(),
        OemValue::Level(value) => value.to_string(),
        OemValue::Rgb([red, green, blue]) => format!("#{red:02X}{green:02X}{blue:02X}"),
    }
}
fn oem_choice_label(choice: &str) -> &str {
    match choice {
        "high_capacity" => "高容量",
        "balanced" => "均衡",
        "stationary" => "长期接电",
        _ => choice,
    }
}
fn is_temperature(r: &SensorReading) -> bool {
    matches!(r.kind.as_str(), "temperature" | "temp")
        && matches!(r.unit.as_str(), "°C" | "C" | "celsius")
}
fn measured(r: &SensorReading) -> Option<f64> {
    if r.status == CapabilityStatus::Available {
        r.value.filter(|v| v.is_finite())
    } else {
        None
    }
}
fn sensor_key(r: &SensorReading) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        r.device_id, r.id, r.source.backend, r.source.path, r.unit
    )
}
fn sensor_category(r: &SensorReading) -> &'static str {
    let identity = format!("{} {} {}", r.device_id, r.label, r.source.path).to_lowercase();
    if matches!(r.kind.as_str(), "fan_speed" | "fan") {
        "FAN"
    } else if ["nvidia", "amdgpu", "gpu"]
        .iter()
        .any(|p| identity.contains(p))
    {
        "GPU"
    } else if ["cpu", "coretemp", "k10temp", "x86_pkg_temp"]
        .iter()
        .any(|p| identity.contains(p))
    {
        "CPU"
    } else {
        "OTHER"
    }
}
fn fan_readings(readings: &[SensorReading]) -> Vec<&SensorReading> {
    readings
        .iter()
        .filter(|r| matches!(r.kind.as_str(), "fan_speed" | "fan"))
        .collect()
}
fn fan_component(reading: &SensorReading) -> Option<&'static str> {
    // Only the reported label can explicitly identify the cooled component.
    // Generic fan indexes, device order and a shared hwmon path do not.
    let has = |component: &str| {
        reading
            .label
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word.eq_ignore_ascii_case(component))
    };
    match (has("CPU"), has("GPU")) {
        (true, false) => Some("CPU"),
        (false, true) => Some("GPU"),
        _ => None,
    }
}
fn primary_temperature<'a>(
    readings: &'a [SensorReading],
    category: &str,
) -> Option<&'a SensorReading> {
    let mut matching: Vec<_> = readings
        .iter()
        .filter(|r| is_temperature(r) && sensor_category(r) == category)
        .collect();
    matching.sort_by(|a, b| {
        measured(b)
            .partial_cmp(&measured(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matching.first().copied()
}
fn temperature_metric(
    ui: &mut egui::Ui,
    label: &str,
    reading: Option<&SensorReading>,
    color: Color32,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 2.5, color);
        ui.label(RichText::new(label).size(14.0).color(label_color(ui)));
    });
    let value = reading
        .and_then(measured)
        .map(|v| format!("{v:.0}"))
        .unwrap_or_else(|| "—".into());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(
            RichText::new(value)
                .font(appearance::tabular_font(52.0))
                .color(foreground(ui)),
        )
        .on_hover_text(
            reading
                .map(|r| format!("{} · {} · {}", r.label, r.device_id, r.source.path))
                .unwrap_or_else(|| "尚无温度读数".into()),
        );
        if reading.and_then(measured).is_some() {
            ui.label(
                RichText::new("°C")
                    .font(appearance::number_font(18.0))
                    .color(secondary(ui)),
            );
        }
    });
    if reading.and_then(measured).is_none() {
        ui.label(RichText::new("未采集").size(13.0).color(secondary(ui)));
    }
}
fn component_fan_metric(ui: &mut egui::Ui, reading: &SensorReading) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        small_icon(ui, Icon::Fan);
        ui.label(RichText::new("风扇").size(13.0).color(label_color(ui)))
            .on_hover_text(format!("{} · {}", reading.label, reading.source.path));
        let value = measured(reading)
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "—".into());
        ui.label(RichText::new(value).font(appearance::tabular_font(18.0)));
        ui.label(RichText::new(&reading.unit).size(13.0).color(secondary(ui)));
    });
}
fn fan_metric(ui: &mut egui::Ui, reading: &SensorReading) {
    let visible_label = reading
        .label
        .strip_prefix("mechrevo_ec:")
        .map(str::trim)
        .unwrap_or(&reading.label);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        small_icon(ui, Icon::Fan);
        ui.add(
            egui::Label::new(
                RichText::new(visible_label)
                    .size(13.0)
                    .color(label_color(ui)),
            )
            .truncate(),
        )
        .on_hover_text(&reading.label);
        let value = measured(reading)
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "—".into());
        ui.label(RichText::new(value).font(appearance::tabular_font(18.0)))
            .on_hover_text(format!("{} · {}", reading.device_id, reading.source.path));
        ui.label(RichText::new(&reading.unit).size(13.0).color(secondary(ui)));
    });
}
#[derive(Clone, Copy)]
enum Icon {
    Brand,
    Fan,
    Power,
    Sun,
    Battery,
    Laptop,
}
fn icon(p: &egui::Painter, rect: Rect, glyph: Icon, color: Color32) {
    let r = rect.shrink(1.0);
    let c = r.center();
    let w = r.width();
    let h = r.height();
    let s = Stroke::new(1.5, color);
    let point = |x: f32, y: f32| egui::pos2(r.left() + w * x, r.top() + h * y);
    match glyph {
        Icon::Brand => {
            for (a, b) in [
                ((0.1, 0.85), (0.1, 0.15)),
                ((0.1, 0.15), (0.5, 0.57)),
                ((0.5, 0.57), (0.9, 0.15)),
                ((0.9, 0.15), (0.9, 0.85)),
            ] {
                p.line_segment([point(a.0, a.1), point(b.0, b.1)], Stroke::new(2.5, color));
            }
        }
        Icon::Fan => {
            p.circle_stroke(c, w * 0.42, s);
            p.circle_filled(c, w * 0.09, color);
            for angle in [0.0_f32, 2.094, 4.189] {
                let a = egui::vec2(angle.cos(), angle.sin());
                let b = egui::vec2((angle + 0.7).cos(), (angle + 0.7).sin());
                p.line(vec![c + a * w * 0.1, c + a * w * 0.3, c + b * w * 0.22], s);
            }
        }
        Icon::Power => {
            let points: Vec<_> = (0..=22)
                .map(|i| {
                    let angle = 0.7 + i as f32 / 22.0 * (std::f32::consts::TAU - 1.4);
                    c + egui::vec2(angle.sin(), -angle.cos()) * w * 0.39
                })
                .collect();
            p.line(points, s);
            p.line_segment([point(0.5, 0.02), point(0.5, 0.47)], s);
        }
        Icon::Sun => {
            p.circle_stroke(c, w * 0.22, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d = egui::vec2(a.cos(), a.sin());
                p.line_segment([c + d * w * 0.34, c + d * w * 0.47], s);
            }
        }
        Icon::Battery => {
            p.rect_stroke(
                Rect::from_min_max(point(0.02, 0.25), point(0.85, 0.75)),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
            p.line_segment([point(0.98, 0.4), point(0.98, 0.6)], s);
            p.line_segment([point(0.23, 0.5), point(0.61, 0.5)], s);
        }
        Icon::Laptop => {
            p.rect_stroke(
                Rect::from_min_max(point(0.12, 0.12), point(0.88, 0.72)),
                2.0,
                s,
                egui::StrokeKind::Inside,
            );
            p.line_segment([point(0.0, 0.9), point(1.0, 0.9)], s);
        }
    }
}
fn refresh_icon(p: &egui::Painter, rect: Rect, color: Color32, rotation: f32) {
    let c = rect.center();
    let radius = rect.width() * 0.39;
    let points: Vec<_> = (0..=22)
        .map(|i| {
            let a = rotation + i as f32 / 22.0 * 5.1;
            c + egui::vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    p.line(points, Stroke::new(1.5, color));
    let a = rotation + 5.1;
    let tip = c + egui::vec2(a.cos(), a.sin()) * radius;
    p.line_segment(
        [tip, tip + egui::vec2(-a.sin(), a.cos()) * 4.0],
        Stroke::new(1.5, color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mechrevo_console_core::{EvidenceSource, Identity, PowerPolicy, Snapshot};
    fn reading(device: &str, value: Option<f64>) -> SensorReading {
        SensorReading {
            id: "temp1".into(),
            label: "CPU package".into(),
            device_id: device.into(),
            kind: "temperature".into(),
            value,
            unit: "°C".into(),
            source: EvidenceSource {
                backend: "fixture".into(),
                path: format!("/{device}/temp1"),
                device_id: Some(device.into()),
            },
            status: if value.is_some() {
                CapabilityStatus::Available
            } else {
                CapabilityStatus::Unavailable
            },
            reason: None,
        }
    }
    fn snapshot(readings: Vec<SensorReading>) -> Snapshot {
        Snapshot {
            platform: "fixture".into(),
            identity: Identity::default(),
            telemetry: readings,
            capabilities: vec![],
            power_policy: PowerPolicy::default(),
            diagnostics: vec![],
            brightness: vec![],
            oem_controls: vec![],
        }
    }
    #[test]
    fn history_deduplicates_frames_and_keeps_unknown_gaps() {
        let r = reading("cpu0", Some(51.0));
        let key = sensor_key(&r);
        let mut h = TemperatureHistory::default();
        let now = Instant::now();
        let s = snapshot(vec![r]);
        h.observe(Some(&s), Some(now));
        h.observe(Some(&s), Some(now));
        assert_eq!(h.series[&key].len(), 1);
        h.observe(
            Some(&snapshot(vec![reading("cpu0", None)])),
            Some(now + Duration::from_secs(3)),
        );
        assert_eq!(h.series[&key].len(), 2);
        assert_eq!(h.series[&key][1].1, None);
    }
    #[test]
    fn history_isolates_devices_and_bounds_retention() {
        let a = reading("cpu0", Some(51.0));
        let b = reading("cpu1", Some(65.0));
        let mut h = TemperatureHistory::default();
        let now = Instant::now();
        let s = snapshot(vec![a.clone(), b.clone()]);
        for i in 0..HISTORY_LIMIT + 7 {
            h.observe(Some(&s), Some(now + Duration::from_secs(i as u64)));
        }
        assert_eq!(h.series.len(), 2);
        assert_eq!(h.series[&sensor_key(&a)].len(), HISTORY_LIMIT);
        assert!(h.series[&sensor_key(&a)]
            .iter()
            .all(|(_, v)| *v == Some(51.0)));
        assert!(h.series[&sensor_key(&b)]
            .iter()
            .all(|(_, v)| *v == Some(65.0)));
        let mut other = s;
        other.identity.board_name = Some("other-machine".into());
        h.observe(Some(&other), Some(now + Duration::from_secs(200)));
        assert!(h.series.values().all(|v| v.len() == 1));
    }
    #[test]
    fn production_fan_speed_and_multiple_gpus_remain_individual() {
        let mut a = reading("fan-device", Some(2345.0));
        a.kind = "fan_speed".into();
        a.unit = "RPM".into();
        assert_eq!(sensor_category(&a), "FAN");
        assert!(!is_temperature(&a));
        let fans = [a];
        let visible_fans = fan_readings(&fans);
        assert_eq!(visible_fans.len(), 1);
        assert_eq!(visible_fans[0].device_id, "fan-device");
        assert_eq!(reading_text(visible_fans[0]), "2345.0 RPM");
        let mut gpu0 = reading("gpu0", Some(47.0));
        gpu0.label = "GPU 0".into();
        let mut gpu1 = reading("gpu1", Some(71.0));
        gpu1.label = "GPU 1".into();
        let readings = [gpu0, gpu1];
        let selected = primary_temperature(&readings, "GPU").unwrap();
        assert_eq!(selected.device_id, "gpu1");
        assert_eq!(selected.value, Some(71.0));
        assert_ne!(sensor_key(&readings[0]), sensor_key(&readings[1]));
    }
    #[test]
    fn fan_component_requires_an_unambiguous_reported_label() {
        let mut fan = reading("cpu-device", Some(2400.0));
        fan.kind = "fan_speed".into();
        fan.unit = "RPM".into();
        fan.source.path = "/hwmon/cpu/fan1_input".into();
        for label in ["fan1", "mechrevo_ec:fan2", "CPU / GPU fan", "scpu fan"] {
            fan.label = label.into();
            assert_eq!(fan_component(&fan), None, "{label}");
        }
        for label in ["CPU fan", "mechrevo_ec:cpu_fan1", "CPU 风扇"] {
            fan.label = label.into();
            assert_eq!(fan_component(&fan), Some("CPU"), "{label}");
        }
        fan.label = "mechrevo_ec:GPU fan".into();
        assert_eq!(fan_component(&fan), Some("GPU"));
    }

    #[test]
    fn native_close_waits_for_oem_apply_and_stays_blocked_for_unknown_helper() {
        for failure in [None, Some("oem_helper_liveness_unknown")] {
            let (mut controller, ready, release) = crate::tests::gated_oem_controller(failure);
            crate::tests::prepare_oem(&mut controller);
            controller.preview_oem().unwrap();
            crate::tests::finish(&mut controller);
            controller.apply_oem_preview().unwrap();
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            let mut app = DesktopApp::with_controller(
                controller,
                Preferences {
                    auto_refresh: false,
                    ..Default::default()
                },
            );
            let ctx = egui::Context::default();
            let mut close = egui::RawInput::default();
            close
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .events
                .push(egui::ViewportEvent::Close);
            let output = ctx.run_pass(close, |ctx| app.tick(ctx));
            let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
            assert!(commands.contains(&egui::ViewportCommand::CancelClose));
            assert!(!commands.contains(&egui::ViewportCommand::Close));
            // This close-event harness has no renderer or texture consumer.
            // Explicitly discard only its unrendered texture deltas.
            output.drop_without_applying_deltas();
            assert!(app.pending_close);
            release.send(()).unwrap();
            if failure.is_some() {
                let deadline = Instant::now() + Duration::from_secs(2);
                while app.controller.error.is_none() && Instant::now() < deadline {
                    app.controller.poll();
                    std::thread::yield_now();
                }
                assert!(app.controller.write_in_progress());
                let output = ctx.run_pass(egui::RawInput::default(), |ctx| app.tick(ctx));
                assert!(app.pending_close);
                assert!(!output.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .contains(&egui::ViewportCommand::Close));
                output.drop_without_applying_deltas();
            } else {
                crate::tests::finish(&mut app.controller);
                let output = ctx.run_pass(egui::RawInput::default(), |ctx| app.tick(ctx));
                assert!(!app.pending_close);
                assert!(output.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .contains(&egui::ViewportCommand::Close));
                output.drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn fan_lifecycle_events_wake_idle_frames_without_sensor_sampling() {
        struct NoHardware;
        impl crate::Backend for NoHardware {
            fn snapshot(&mut self) -> Snapshot {
                panic!("idle lifecycle polling must not sample hardware")
            }
            fn catalog(&mut self) -> Vec<mechrevo_console_core::CatalogModel> {
                unreachable!()
            }
            fn plan(
                &mut self,
                _: PowerProfile,
            ) -> Result<mechrevo_console_core::ProfilePlan, mechrevo_console_core::ControlError>
            {
                unreachable!()
            }
            fn apply(
                &mut self,
                _: &mechrevo_console_core::ProfilePlan,
            ) -> Result<mechrevo_console_core::ApplyResult, mechrevo_console_core::ControlError>
            {
                unreachable!()
            }
        }
        fn frame(ctx: &egui::Context, app: &mut DesktopApp, time: f64) -> Duration {
            let output = ctx.run_pass(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| app.tick(ctx),
            );
            let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            output.drop_without_applying_deltas();
            delay
        }
        let mut app = DesktopApp::with_controller(
            Controller::new(NoHardware).unwrap(),
            Preferences {
                auto_refresh: false,
                ..Default::default()
            },
        );
        let ctx = egui::Context::default();
        let mut time = 0.0;
        // Drain the context's startup repaint requests before observing idle.
        for _ in 0..5 {
            frame(&ctx, &mut app, time);
            time += 0.25;
        }
        assert!(
            frame(&ctx, &mut app, time) > Duration::from_secs(1),
            "a clean manual-sampling console must stay idle"
        );
        time += 0.25;
        app.controller.fan_session_held = true;
        app.controller.fan_helper_reaped = false;
        app.controller.fan_cleanup_complete = false;
        assert!(!app.controller.busy());
        assert!(
            frame(&ctx, &mut app, time) <= Duration::from_millis(200),
            "a retained helper needs a future frame even when sampling is off"
        );
        time += 0.25;
        let (events, incoming) = std::sync::mpsc::sync_channel(1);
        app.controller.fan_events = incoming;
        events
            .send(crate::FanEvent {
                sequence: 1,
                update: mechrevo_console_core::fan_curve::client::FanClientUpdate {
                    error: Some(mechrevo_console_core::ControlError::new(
                        "fixture_helper_exited",
                        "后台退出，恢复尚未确认",
                    )),
                    session_held: false,
                    helper_reaped: true,
                    cleanup_complete: false,
                    ..Default::default()
                },
            })
            .unwrap();
        assert!(frame(&ctx, &mut app, time) <= Duration::from_millis(200));
        assert_eq!(app.controller.fan_error.as_ref().unwrap().code, "fixture_helper_exited", "the next scheduled frame must publish the background failure without pointer/key input");
        assert!(!app.controller.busy());
        assert!(!app.controller.fan_cleanup_complete);
        time += 0.25;
        events
            .send(crate::FanEvent {
                sequence: 2,
                update: Default::default(),
            })
            .unwrap();
        for _ in 0..5 {
            frame(&ctx, &mut app, time);
            time += 0.25;
        }
        assert!(
            app.controller.fan_helper_reaped
                && app.controller.fan_cleanup_complete
                && !app.controller.fan_session_held
        );
        assert!(
            frame(&ctx, &mut app, time) > Duration::from_secs(1),
            "confirmed cleanup must return a manual-sampling console to idle"
        );
    }
}
