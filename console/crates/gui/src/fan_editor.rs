//! Local fan-curve editing. No device access, defaults or raw-table decoding.
use eframe::egui::{self, Color32, FontId, Rect, RichText, Stroke, Vec2};
use mechrevo_console_core::{
    fan_curve::{FanCurve, FanCurveChannel, ValidatedFanCurve, MAX_INPUT_BYTES},
    ControlError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FanChannel {
    #[default]
    Cpu,
    Gpu,
}

pub enum FanEditorUpdate {
    Import(Vec<u8>),
    Edit(FanCurve),
}

/// Drafts are created only from supplied, validated JSON. An invalid edit stays
/// visible locally, but is never promoted into an applicable validated draft.
#[derive(Default)]
// @mark mechrevo.fan_curve role=ui contract=explicit_validated_draft
pub struct FanEditor {
    working: Option<FanCurve>,
    channel: FanChannel,
    stage: usize,
    import_open: bool,
    import_text: String,
    import_error: Option<ControlError>,
    update: Option<FanEditorUpdate>,
}

impl FanEditor {
    pub fn load_json(&mut self, input: &[u8]) -> Result<(), ControlError> {
        self.update = Some(FanEditorUpdate::Import(
            input.iter().take(MAX_INPUT_BYTES + 1).copied().collect(),
        ));
        match ValidatedFanCurve::from_json(input) {
            Ok(curve) => {
                self.working = Some(curve.curve().clone());
                self.import_error = None;
                self.import_open = false;
                Ok(())
            }
            Err(error) => {
                self.import_error = Some(error.clone());
                Err(error)
            }
        }
    }

    pub fn curve(&self) -> Option<&FanCurve> {
        self.working.as_ref()
    }
    pub fn channel(&self) -> FanChannel {
        self.channel
    }
    pub fn selected_stage(&self) -> usize {
        self.stage
    }
    pub fn is_valid(&self) -> bool {
        self.import_error.is_none()
            && self
                .working
                .as_ref()
                .is_some_and(|curve| curve.clone().validate().is_ok())
    }
    pub fn take_update(&mut self) -> Option<FanEditorUpdate> {
        self.update.take()
    }

    /// Adopt a typed user draft, never bytes obtained from a hardware table.
    pub fn adopt_draft(&mut self, draft: &ValidatedFanCurve) {
        self.working = Some(draft.curve().clone());
        self.import_error = None;
        self.update = None;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, editable: bool, applied: Option<&ValidatedFanCurve>) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(editable, egui::Button::new("导入曲线"))
                .clicked()
            {
                self.import_open = !self.import_open;
            }
            if let Some(curve) = &self.working {
                if ui.button("复制草稿").clicked() {
                    if let Ok(json) = serde_json::to_string_pretty(curve) {
                        ui.ctx().copy_text(json);
                    }
                }
            }
        });
        if self.import_open {
            ui.add_space(8.0);
            ui.label(RichText::new("粘贴已有曲线的 JSON，载入后再编辑与预览。").size(13.0));
            let label = ui.label(RichText::new("曲线 JSON").size(13.0).color(secondary(ui)));
            ui.add_enabled(
                editable,
                egui::TextEdit::multiline(&mut self.import_text)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_INPUT_BYTES)
                    .hint_text("曲线 JSON"),
            )
            .labelled_by(label.id);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(editable, egui::Button::new("载入草稿"))
                    .clicked()
                {
                    let input = self.import_text.as_bytes().to_vec();
                    let _ = self.load_json(&input);
                }
                ui.label(
                    RichText::new("最多 8 KiB · 不生成默认曲线")
                        .size(13.0)
                        .color(secondary(ui)),
                );
            });
        }
        if let Some(error) = &self.import_error {
            ui.label(
                RichText::new(format!("导入未通过：{}", error.message))
                    .size(13.0)
                    .color(error_color(ui)),
            );
            if self.working.is_some() && ui.button("继续编辑原草稿").clicked() {
                self.import_error = None;
                self.update = self.working.clone().map(FanEditorUpdate::Edit);
            }
        }
        let Some(mut curve) = self.working.clone() else {
            ui.add_space(18.0);
            ui.label(RichText::new("还没有曲线草稿").size(20.0));
            ui.label(RichText::new("导入一份已知曲线后，才会显示 CPU / GPU 的温界和占空比。原硬件表不会被转换成新草稿。").size(14.0).color(secondary(ui)));
            return;
        };
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.channel, FanChannel::Cpu, "CPU");
            ui.selectable_value(&mut self.channel, FanChannel::Gpu, "GPU");
            ui.add_space(10.0);
            ui.label(RichText::new("编辑通道").size(13.0).color(secondary(ui)));
        });
        let before = curve.clone();
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut curve.respective, true, "独立控制");
                ui.selectable_value(&mut curve.respective, false, "同步控制");
            });
        });
        ui.label(
            RichText::new("切换模式保留两路曲线的原值。")
                .size(13.0)
                .color(secondary(ui)),
        );
        let channel = match self.channel {
            FanChannel::Cpu => &mut curve.cpu,
            FanChannel::Gpu => &mut curve.gpu,
        };
        let live = applied.map(|curve| match self.channel {
            FanChannel::Cpu => &curve.curve().cpu,
            FanChannel::Gpu => &curve.curve().gpu,
        });
        ui.add_space(8.0);
        curve_plot(ui, channel, live, &mut self.stage);
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            let label = ui.label("编辑档位");
            ui.add(
                egui::Slider::new(&mut self.stage, 0..=15)
                    .integer()
                    .custom_formatter(|value, _| format!("第 {} 档", value as usize + 1)),
            )
            .labelled_by(label.id);
        });
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("fan-stage-values")
                .num_columns(2)
                .spacing(Vec2::new(18.0, 10.0))
                .show(ui, |ui| {
                    if self.stage < 15 {
                        let label = ui.label("升温温界");
                        ui.add(
                            egui::DragValue::new(&mut channel.rise_celsius[self.stage])
                                .speed(1.0)
                                .range(0..=255)
                                .suffix(" °C"),
                        )
                        .labelled_by(label.id);
                        ui.end_row();
                        let label = ui.label("降温温界");
                        ui.add(
                            egui::DragValue::new(&mut channel.fall_celsius[self.stage])
                                .speed(1.0)
                                .range(0..=255)
                                .suffix(" °C"),
                        )
                        .labelled_by(label.id);
                        ui.end_row();
                    } else {
                        ui.label("末档温界");
                        ui.label(
                            RichText::new("只有前 15 档有温界")
                                .size(13.0)
                                .color(secondary(ui)),
                        );
                        ui.end_row();
                    }
                    let label = ui.label("占空比");
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut channel.duty_raw[self.stage])
                                .speed(1.0)
                                .range(0..=200),
                        )
                        .labelled_by(label.id);
                        ui.label(format!(
                            "= {:.1}%",
                            f32::from(channel.duty_raw[self.stage]) / 2.0
                        ));
                    });
                    ui.end_row();
                });
        });
        ui.label(
            RichText::new(
                "占空比每 1 个原始单位为 0.5%。温界 255 按协议原值保留，不外推温度曲线。",
            )
            .size(13.0)
            .color(secondary(ui)),
        );
        if curve != before {
            self.import_error = None;
            self.working = Some(curve.clone());
            self.update = Some(FanEditorUpdate::Edit(curve.clone()));
        }
        if let Err(error) = curve.validate() {
            ui.label(
                RichText::new(format!("草稿需要修正：{}", error.message))
                    .size(13.0)
                    .color(error_color(ui)),
            );
        }
    }
}

fn curve_plot(
    ui: &mut egui::Ui,
    channel: &FanCurveChannel,
    applied: Option<&FanCurveChannel>,
    stage: &mut usize,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("占空比").size(14.0));
        ui.label(RichText::new("草稿").size(13.0).color(amber(ui)));
        if applied.is_some() {
            ui.label(RichText::new("已应用").size(13.0).color(secondary(ui)));
        }
    });
    let width = ui.available_width().max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, 158.0), egui::Sense::click_and_drag());
    let plot = Rect::from_min_max(
        rect.left_top() + Vec2::new(34.0, 10.0),
        rect.right_bottom() - Vec2::new(8.0, 23.0),
    );
    let painter = ui.painter_at(rect);
    grid(&painter, plot, ui, &[0.0, 50.0, 100.0], 100.0, "%", 16);
    if let Some(applied) = applied {
        duty_steps(&painter, plot, &applied.duty_raw, secondary(ui), 1.0);
    }
    duty_steps(&painter, plot, &channel.duty_raw, amber(ui), 2.0);
    let x = stage_x(plot, *stage, 16);
    painter.line_segment(
        [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
        Stroke::new(1.0, secondary(ui).gamma_multiply(0.5)),
    );
    if response.is_pointer_button_down_on() || response.dragged() || response.clicked() {
        if let Some(point) = response.interact_pointer_pos() {
            *stage = (((point.x - plot.left()) / plot.width()) * 16.0)
                .floor()
                .clamp(0.0, 15.0) as usize;
        }
    }
    ui.label(
        RichText::new("横轴为档位 1–16，阶梯表示各档输出，不是温度之间的平滑插值。")
            .size(13.0)
            .color(secondary(ui)),
    );
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("双温界").size(14.0));
        ui.label(RichText::new("升温").size(13.0).color(amber(ui)));
        ui.label(RichText::new("降温").size(13.0).color(secondary(ui)));
    });
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 138.0), egui::Sense::hover());
    let plot = Rect::from_min_max(
        rect.left_top() + Vec2::new(34.0, 8.0),
        rect.right_bottom() - Vec2::new(8.0, 23.0),
    );
    let painter = ui.painter_at(rect);
    grid(&painter, plot, ui, &[0.0, 128.0, 255.0], 255.0, "", 15);
    for (values, color) in [
        (&channel.rise_celsius, amber(ui)),
        (&channel.fall_celsius, secondary(ui)),
    ] {
        for (index, value) in values.iter().enumerate() {
            let point = egui::pos2(
                stage_x(plot, index, 15),
                plot.bottom() - plot.height() * f32::from(*value) / 255.0,
            );
            painter.circle_filled(point, if index == *stage { 3.5 } else { 2.0 }, color);
        }
    }
    ui.label(
        RichText::new("15 个原始温界 · °C；255 原值单独保留")
            .size(13.0)
            .color(secondary(ui)),
    );
}

fn stage_x(plot: Rect, index: usize, count: usize) -> f32 {
    plot.left() + plot.width() * (index as f32 + 0.5) / count as f32
}
fn duty_steps(
    painter: &egui::Painter,
    plot: Rect,
    values: &[u8; 16],
    color: Color32,
    thickness: f32,
) {
    for segment in duty_segments(plot, values) {
        painter.line_segment(segment, Stroke::new(thickness, color));
    }
}
fn duty_segments(plot: Rect, values: &[u8; 16]) -> Vec<[egui::Pos2; 2]> {
    let mut segments = Vec::with_capacity(31);
    for index in 0..16 {
        let x = plot.left() + plot.width() * index as f32 / 16.0;
        let next_x = plot.left() + plot.width() * (index + 1) as f32 / 16.0;
        let y = plot.bottom() - plot.height() * f32::from(values[index]) / 200.0;
        segments.push([egui::pos2(x, y), egui::pos2(next_x, y)]);
        if index < 15 {
            let next_y = plot.bottom() - plot.height() * f32::from(values[index + 1]) / 200.0;
            segments.push([egui::pos2(next_x, y), egui::pos2(next_x, next_y)]);
        }
    }
    segments
}
fn grid(
    painter: &egui::Painter,
    plot: Rect,
    ui: &egui::Ui,
    values: &[f32],
    maximum: f32,
    suffix: &str,
    stages: usize,
) {
    for value in values {
        let y = plot.bottom() - plot.height() * value / maximum;
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(0.7, secondary(ui).gamma_multiply(0.18)),
        );
        painter.text(
            egui::pos2(plot.left() - 5.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{value:.0}{suffix}"),
            FontId::proportional(12.0),
            secondary(ui),
        );
    }
    for stage in [0, 3, 7, 11, stages - 1] {
        painter.text(
            egui::pos2(stage_x(plot, stage, stages), plot.bottom() + 6.0),
            egui::Align2::CENTER_TOP,
            format!("{}", stage + 1),
            FontId::proportional(12.0),
            secondary(ui),
        );
    }
}
fn secondary(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(164, 167, 169)
    } else {
        Color32::from_rgb(103, 106, 108)
    }
}
fn amber(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(221, 187, 124)
    } else {
        Color32::from_rgb(137, 100, 31)
    }
}
fn error_color(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(237, 154, 140)
    } else {
        Color32::from_rgb(154, 46, 34)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Vec<u8> {
        serde_json::to_vec(&FanCurve {
            schema_version: 1,
            layout: mechrevo_console_core::fan_curve::FanCurveLayout::UniwillRamFan1p5,
            respective: true,
            cpu: FanCurveChannel {
                rise_celsius: [45; 15],
                fall_celsius: [40; 15],
                duty_raw: [99; 16],
            },
            gpu: FanCurveChannel {
                rise_celsius: [255; 15],
                fall_celsius: [255; 15],
                duty_raw: [101; 16],
            },
        })
        .unwrap()
    }
    #[test]
    fn editor_starts_empty_and_only_accepts_supplied_typed_input() {
        let mut editor = FanEditor::default();
        assert!(editor.curve().is_none());
        assert!(!editor.is_valid());
        editor.load_json(&input()).unwrap();
        assert_eq!(editor.curve().unwrap().cpu.duty_raw[0], 99);
        assert_eq!(editor.curve().unwrap().gpu.rise_celsius, [255; 15]);
        assert!(editor.is_valid());
        assert!(matches!(
            editor.take_update(),
            Some(FanEditorUpdate::Import(_))
        ));
    }
    #[test]
    fn invalid_import_keeps_local_work_but_cannot_be_previewed() {
        let mut editor = FanEditor::default();
        editor.load_json(&input()).unwrap();
        let original = editor.curve().unwrap().clone();
        assert!(editor.load_json(br#"{"schema_version":1}"#).is_err());
        assert_eq!(editor.curve(), Some(&original));
        assert!(!editor.is_valid());
        assert!(matches!(
            editor.take_update(),
            Some(FanEditorUpdate::Import(_))
        ));
    }
    #[test]
    fn all_sixteen_duty_stages_own_a_visible_interval_including_half_percent_last_stage() {
        let plot = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(320.0, 200.0));
        let mut values = [99; 16];
        values[15] = 101;
        let segments = duty_segments(plot, &values);
        assert_eq!(segments.len(), 31);
        assert_eq!(
            segments
                .iter()
                .filter(|[left, right]| right.x > left.x)
                .count(),
            16
        );
        let last = segments.last().unwrap();
        assert_eq!(last[0].x, 300.0);
        assert_eq!(last[1].x, 320.0);
        assert!((last[0].y - 99.0).abs() < 0.001);
        assert_eq!(last[0].y, last[1].y);
        assert_eq!(stage_x(plot, 15, 16), 310.0);
    }
}
