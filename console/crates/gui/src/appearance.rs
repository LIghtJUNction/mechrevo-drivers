//! Application-owned frosted material. This never captures or blurs the desktop.
use eframe::egui::{self, Color32, FontId, Rect, TextureHandle, Vec2};

/// Install once per context, keeping the platform's Chinese fallback provider.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let key = egui::Id::unique("mechrevo-bundled-fonts-v1");
    if ctx.data(|data| data.get_temp::<bool>(key).unwrap_or(false)) {
        return;
    }
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    for (name, bytes, families) in [
        (
            "Cormorant Garamond Italic",
            include_bytes!("../assets/fonts/CormorantGaramond-Italic.ttf").as_slice(),
            vec![egui::FontFamily::Name("Display".into())],
        ),
        (
            "Manrope",
            include_bytes!("../assets/fonts/Manrope.ttf").as_slice(),
            vec![
                egui::FontFamily::Name("Numbers".into()),
                egui::FontFamily::Proportional,
            ],
        ),
    ] {
        let data = egui::FontData::from_static(bytes).tweak(egui::epaint::text::FontTweak {
            coords: egui::epaint::text::VariationCoords::new([(
                b"wght",
                if name == "Manrope" { 450.0 } else { 400.0 },
            )]),
            ..Default::default()
        });
        ctx.add_font(FontInsert::new(
            name,
            data,
            families
                .into_iter()
                .map(|family| InsertFontFamily {
                    family,
                    priority: FontPriority::Highest,
                })
                .collect(),
        ));
    }
    ctx.data_mut(|data| data.insert_temp(key, true));
}

pub(crate) fn display_font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("Display".into()))
}

pub(crate) fn number_font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("Numbers".into()))
}

/// The bundled proportional face does not promise tabular figures. Readouts
/// use egui's actual monospace family so equal-length values keep their width.
pub(crate) fn tabular_font(size: f32) -> FontId {
    FontId::monospace(size)
}

/// A cached, deterministic diffuse field with fixed grain. No per-frame noise,
/// animated wallpaper, network request, compositor dependency or sensor input.
pub(crate) fn paint_backdrop(ui: &egui::Ui, rect: Rect) {
    let dark = ui.visuals().dark_mode;
    let key = egui::Id::unique(("mechrevo-diffuse-field-v2", dark));
    let texture = ui
        .ctx()
        .data(|data| data.get_temp::<TextureHandle>(key))
        .unwrap_or_else(|| {
            let texture = ui.ctx().load_texture(
                if dark {
                    "graphite-field"
                } else {
                    "ivory-field"
                },
                diffuse_field(dark),
                egui::TextureOptions::LINEAR,
            );
            ui.ctx()
                .data_mut(|data| data.insert_temp(key, texture.clone()));
            texture
        });
    ui.painter().image(
        texture.id(),
        rect,
        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

fn diffuse_field(dark: bool) -> egui::ColorImage {
    let size = [640, 480];
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    let mut seed = 0x77ab_213du32;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let x = x as f32 / size[0] as f32;
            let y = y as f32 / size[1] as f32;
            let warm = (-((x - 0.91).powi(2) / 0.12 + (y - 0.08).powi(2) / 0.12)).exp();
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let grain = (seed & 255) as f32 / 255.0 - 0.5;
            let channel =
                |base: f32, a: f32| (base + a * warm + grain * 0.55).clamp(0.0, 255.0) as u8;
            pixels.push(if dark {
                Color32::from_rgb(channel(11.0, 4.0), channel(13.0, 4.0), channel(15.0, 5.0))
            } else {
                Color32::from_rgb(
                    channel(239.0, 9.0),
                    channel(239.0, 6.0),
                    channel(235.0, 2.0),
                )
            });
        }
    }
    egui::ColorImage::new(size, pixels)
}

pub(crate) fn glass<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let dark = ui.visuals().dark_mode;
    let edge = if dark {
        Color32::from_white_alpha(20)
    } else {
        Color32::from_black_alpha(18)
    };
    let response = egui::Frame::new()
        .fill(if dark {
            Color32::from_rgb(24, 28, 32)
        } else {
            Color32::from_rgb(248, 249, 246)
        })
        .stroke(egui::Stroke::new(1.0, edge))
        .corner_radius(12.0)
        .shadow(egui::epaint::Shadow {
            offset: [0, 4],
            blur: 14,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 28 } else { 12 }),
        })
        .inner_margin(if ui.available_width() < 380.0 {
            16.0
        } else {
            20.0
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui)
        });
    let rect = response.response.rect;
    // An inset reflection makes the upper edge read as a thin material surface.
    let painter = ui.painter_at(rect);
    painter.line_segment(
        [
            rect.left_top() + Vec2::new(24.0, 0.7),
            rect.right_top() + Vec2::new(-24.0, 0.7),
        ],
        egui::Stroke::new(0.6, Color32::from_white_alpha(if dark { 19 } else { 100 })),
    );
    response.inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grain_is_stable_and_themes_are_distinct() {
        let dark = diffuse_field(true);
        assert_eq!(dark, diffuse_field(true));
        let light = diffuse_field(false);
        assert_eq!(dark.size, light.size);
        assert!(dark
            .pixels
            .iter()
            .all(|p| p.r() < 70 && p.g() < 70 && p.b() < 70));
        assert!(light
            .pixels
            .iter()
            .all(|p| p.r() > 210 && p.g() > 210 && p.b() > 210));
    }
}
