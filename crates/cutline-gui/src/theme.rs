//! Colours and spacing from the design: white surfaces, warm greys, one blue.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

pub const INK: Color32 = Color32::from_rgb(0x1f, 0x1e, 0x1b);
pub const MUTED: Color32 = Color32::from_rgb(0x5f, 0x5e, 0x5a);
pub const BORDER: Color32 = Color32::from_rgb(0xc9, 0xc5, 0xbb);
pub const HAIRLINE: Color32 = Color32::from_rgb(0xe4, 0xe1, 0xda);
pub const SURFACE: Color32 = Color32::WHITE;
pub const WELL: Color32 = Color32::from_rgb(0xf1, 0xef, 0xe9);
pub const ACCENT: Color32 = Color32::from_rgb(0x2f, 0x6f, 0xd6);
pub const ACCENT_BG: Color32 = Color32::from_rgb(0xe8, 0xf0, 0xfc);
pub const SHADE: Color32 = Color32::from_rgba_premultiplied(17, 16, 15, 140);
pub const EMPTY_BAR: Color32 = Color32::from_rgb(0xa9, 0xa4, 0x96);
pub const DANGER: Color32 = Color32::from_rgb(0xb4, 0x3c, 0x2e);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(18.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        ),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 32.0;
    let v = &mut style.visuals;
    v.panel_fill = SURFACE;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SURFACE;
    v.faint_bg_color = WELL;
    v.override_text_color = Some(INK);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_BG;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(40),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(30),
    };
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.corner_radius = CornerRadius::same(6);
        w.fg_stroke = Stroke::new(1.0, INK);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
    v.widgets.inactive.bg_fill = SURFACE;
    v.widgets.inactive.weak_bg_fill = SURFACE;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.bg_fill = WELL;
    v.widgets.hovered.weak_bg_fill = WELL;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, EMPTY_BAR);
    v.widgets.active.bg_fill = HAIRLINE;
    v.widgets.active.weak_bg_fill = HAIRLINE;
    v.widgets.active.bg_stroke = Stroke::new(1.0, INK);
    v.widgets.open.weak_bg_fill = WELL;
    ctx.set_global_style(style);
}

/// The bold-outlined primary button (Export, Save).
pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.to_string()).strong())
        .stroke(Stroke::new(2.0, INK))
        .fill(SURFACE)
        .corner_radius(8)
        .min_size(egui::vec2(96.0, 40.0))
}

/// A plain text button with no frame (Open, + Add part, Show in folder).
pub fn quiet(text: &str) -> egui::Button<'static> {
    egui::Button::new(text.to_string())
        .frame(false)
        .min_size(egui::vec2(0.0, 36.0))
}

/// A rounded toggle pill (Keep selected / Remove selected, One video / Separate).
pub fn pill(ui: &mut egui::Ui, on: bool, text: &str) -> egui::Response {
    let rt = if on {
        egui::RichText::new(text).strong().size(13.0)
    } else {
        egui::RichText::new(text).size(13.0).color(MUTED)
    };
    ui.add(
        egui::Button::new(rt)
            .fill(if on { ACCENT_BG } else { SURFACE })
            .stroke(if on {
                Stroke::new(2.0, ACCENT)
            } else {
                Stroke::new(1.0, BORDER)
            })
            .corner_radius(255)
            .min_size(egui::vec2(0.0, 36.0)),
    )
}

/// Four little bars for the quality meter.
pub fn quality_bars(ui: &mut egui::Ui, filled: usize) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(4.0 * 16.0 + 3.0 * 4.0, 8.0),
        egui::Sense::hover(),
    );
    for i in 0..4 {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(i as f32 * 20.0, 0.0),
            egui::vec2(16.0, 8.0),
        );
        if i < filled {
            ui.painter().rect_filled(r, 2, ACCENT);
        } else {
            ui.painter()
                .rect_stroke(r, 2, Stroke::new(1.0, EMPTY_BAR), egui::StrokeKind::Inside);
        }
    }
}

pub fn small(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(12.0).color(MUTED)
}
