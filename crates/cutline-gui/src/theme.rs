//! The look: a dark, quiet editing suite. Neutral greys so the picture is
//! the brightest thing on screen, one blue for actions, amber for the parts
//! you're trimming, a red playhead.

use crate::icons::Icon;
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Response, Sense, Stroke, TextStyle, Ui, Widget,
};
use std::sync::Arc;

// Surfaces, darkest to lightest.
pub const VIEWER: Color32 = Color32::from_rgb(0x07, 0x08, 0x0a);
pub const BG: Color32 = Color32::from_rgb(0x0f, 0x10, 0x13);
pub const PANEL: Color32 = Color32::from_rgb(0x16, 0x17, 0x1b);
pub const RAISED: Color32 = Color32::from_rgb(0x1e, 0x20, 0x25);
pub const HOVER: Color32 = Color32::from_rgb(0x27, 0x29, 0x30);
pub const PRESSED: Color32 = Color32::from_rgb(0x30, 0x33, 0x3b);
pub const HAIRLINE: Color32 = Color32::from_rgb(0x25, 0x27, 0x2d);
pub const BORDER: Color32 = Color32::from_rgb(0x32, 0x35, 0x3d);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x45, 0x48, 0x52);

// Text.
pub const TEXT: Color32 = Color32::from_rgb(0xeb, 0xec, 0xf0);
pub const MUTED: Color32 = Color32::from_rgb(0x9c, 0xa0, 0xab);
pub const FAINT: Color32 = Color32::from_rgb(0x6a, 0x6e, 0x79);

// Accents.
pub const ACCENT: Color32 = Color32::from_rgb(0x4b, 0x8b, 0xff);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x67, 0x9d, 0xff);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(0x3a, 0x76, 0xe6);
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_premultiplied(12, 22, 41, 41);
pub const TRIM: Color32 = Color32::from_rgb(0xf5, 0xb8, 0x3d);
pub const TRIM_SOFT: Color32 = Color32::from_rgba_premultiplied(25, 18, 6, 26);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xff, 0x4f, 0x5e);
pub const DANGER: Color32 = Color32::from_rgb(0xf0, 0x63, 0x5f);
pub const SUCCESS: Color32 = Color32::from_rgb(0x3f, 0xcf, 0x8e);
pub const SHADE: Color32 = Color32::from_rgba_premultiplied(5, 6, 8, 185);

/// Font family for headings and buttons: the system's semibold face when
/// there is one, otherwise the regular one.
pub fn semibold_family() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Ask fontconfig for the desktop's UI font (Inter, Cantarell, Noto Sans …).
fn system_font(pattern: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    // A .ttc needs a face index we don't know; skip it rather than guess.
    if path.is_empty() || !(path.ends_with(".ttf") || path.ends_with(".otf")) {
        return None;
    }
    std::fs::read(path).ok()
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let regular = system_font("sans-serif:style=Regular");
    let semibold =
        system_font("sans-serif:weight=semibold").or_else(|| system_font("sans-serif:style=Bold"));
    let mut proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut strong = proportional.clone();
    if let Some(bytes) = regular {
        fonts
            .font_data
            .insert("ui".into(), Arc::new(egui::FontData::from_owned(bytes)));
        proportional.insert(0, "ui".into());
        strong.insert(0, "ui".into());
    }
    if let Some(bytes) = semibold {
        fonts.font_data.insert(
            "ui-semibold".into(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        strong.insert(0, "ui-semibold".into());
    }
    // Last resort for symbols: of egui's bundled fonts only Hack has arrows
    // (the ← → hints), and desktop UI fonts such as Noto Sans may not.
    for family in [&mut proportional, &mut strong] {
        if !family.iter().any(|f| f == "Hack") {
            family.push("Hack".into());
        }
    }
    fonts
        .families
        .insert(FontFamily::Proportional, proportional);
    fonts.families.insert(semibold_family(), strong);
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(17.0, semibold_family())),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        ),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.window_margin = egui::Margin::same(16);
    let v = &mut style.visuals;
    v.dark_mode = true;
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = RAISED;
    v.code_bg_color = BG;
    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(MUTED);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_SOFT;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 16],
        blur: 48,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_fill = RAISED;
    v.widgets.inactive.weak_bg_fill = RAISED;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.bg_fill = HOVER;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.bg_fill = PRESSED;
    v.widgets.active.weak_bg_fill = PRESSED;
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.open.bg_fill = HOVER;
    v.widgets.open.weak_bg_fill = HOVER;
    v.widgets.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.corner_radius = CornerRadius::same(6);
        w.expansion = 0.0;
    }
    ctx.set_global_style(style);
}

// ---------- buttons ----------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Filled blue: the one thing to do next (Export, Save).
    Primary,
    /// Raised grey with a border.
    Secondary,
    /// No frame until hovered (toolbar actions).
    Ghost,
    /// Ghost, red text (Delete).
    Danger,
}

/// A button painted by hand so every state (hover, press, disabled) looks
/// deliberate, with an optional icon in front of the text.
pub struct Btn {
    text: String,
    icon: Option<Icon>,
    kind: Kind,
    height: f32,
    min_width: f32,
}

impl Btn {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        Btn {
            text: text.into(),
            icon: None,
            kind,
            height: 32.0,
            min_width: 0.0,
        }
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
}

pub fn primary(text: &str) -> Btn {
    Btn::new(Kind::Primary, text)
}
pub fn secondary(text: &str) -> Btn {
    Btn::new(Kind::Secondary, text)
}
pub fn ghost(text: &str) -> Btn {
    Btn::new(Kind::Ghost, text)
}

impl Widget for Btn {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = if self.kind == Kind::Primary {
            FontId::new(13.5, semibold_family())
        } else {
            FontId::new(13.5, FontFamily::Proportional)
        };
        let galley = (!self.text.is_empty())
            .then(|| ui.painter().layout_no_wrap(self.text.clone(), font, TEXT));
        let icon_w = if self.icon.is_some() { 16.0 } else { 0.0 };
        let gap = if self.icon.is_some() && galley.is_some() {
            7.0
        } else {
            0.0
        };
        let text_w = galley.as_ref().map(|g| g.size().x).unwrap_or(0.0);
        let pad = if galley.is_some() { 14.0 } else { 8.0 };
        let w = (pad * 2.0 + icon_w + gap + text_w)
            .max(self.min_width)
            .max(self.height);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, self.height), Sense::click());
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let enabled = ui.is_enabled();
        let hovered = enabled && resp.hovered();
        let pressed = enabled && resp.is_pointer_button_down_on();
        let (fill, stroke, fg) = match self.kind {
            Kind::Primary => (
                if !enabled {
                    RAISED
                } else if pressed {
                    ACCENT_PRESSED
                } else if hovered {
                    ACCENT_HOVER
                } else {
                    ACCENT
                },
                Stroke::NONE,
                if enabled { Color32::WHITE } else { FAINT },
            ),
            Kind::Secondary => (
                if pressed {
                    PRESSED
                } else if hovered {
                    HOVER
                } else {
                    RAISED
                },
                Stroke::new(1.0, if hovered { BORDER_STRONG } else { BORDER }),
                if enabled { TEXT } else { FAINT },
            ),
            Kind::Ghost | Kind::Danger => (
                if pressed {
                    PRESSED
                } else if hovered {
                    HOVER
                } else {
                    Color32::TRANSPARENT
                },
                Stroke::NONE,
                if !enabled {
                    FAINT
                } else if self.kind == Kind::Danger && hovered {
                    DANGER
                } else if hovered {
                    TEXT
                } else {
                    MUTED
                },
            ),
        };
        let p = ui.painter();
        p.rect(rect, 7, fill, stroke, egui::StrokeKind::Inside);
        let content_w = icon_w + gap + text_w;
        let mut x = rect.center().x - content_w / 2.0;
        if let Some(icon) = self.icon {
            icon.paint(
                p,
                egui::Rect::from_center_size(
                    egui::pos2(x + icon_w / 2.0, rect.center().y),
                    egui::vec2(16.0, 16.0),
                ),
                fg,
            );
            x += icon_w + gap;
        }
        if let Some(g) = galley {
            p.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
        }
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp
    }
}

/// A square icon-only button (undo, redo, transport).
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, tip: &str) -> Response {
    Btn::new(Kind::Ghost, "")
        .icon(icon)
        .height(size)
        .min_width(size)
        .ui(ui)
        .on_hover_text(tip)
}

/// A segmented control: a row of options in one rounded track.
/// Returns the index clicked this frame, if any.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: usize) -> Option<usize> {
    let font = FontId::new(13.0, FontFamily::Proportional);
    let font_on = FontId::new(13.0, semibold_family());
    let galleys: Vec<_> = options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let f = if i == selected {
                font_on.clone()
            } else {
                font.clone()
            };
            ui.painter().layout_no_wrap(o.to_string(), f, TEXT)
        })
        .collect();
    let seg_w: Vec<f32> = galleys.iter().map(|g| g.size().x + 28.0).collect();
    let h = 32.0;
    let total: f32 = seg_w.iter().sum::<f32>() + 6.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, h), Sense::hover());
    let p = ui.painter().clone();
    p.rect(
        rect,
        8,
        BG,
        Stroke::new(1.0, HAIRLINE),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.left() + 3.0;
    let mut clicked = None;
    for (i, g) in galleys.into_iter().enumerate() {
        let r = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + 3.0),
            egui::vec2(seg_w[i], h - 6.0),
        );
        let resp = ui.interact(r, ui.scope_id().with(("seg", i)), Sense::click());
        let on = i == selected;
        if on {
            p.rect(
                r,
                6,
                HOVER,
                Stroke::new(1.0, BORDER_STRONG),
                egui::StrokeKind::Inside,
            );
        } else if resp.hovered() {
            p.rect_filled(r, 6, RAISED);
        }
        let fg = if !ui.is_enabled() {
            FAINT
        } else if on || resp.hovered() {
            TEXT
        } else {
            MUTED
        };
        p.galley(r.center() - g.size() / 2.0, g, fg);
        if resp.clicked() {
            clicked = Some(i);
        }
        x += seg_w[i];
    }
    clicked
}

/// A rounded toggle pill, kept for compact on/off choices (One video / Separate).
pub fn pill(ui: &mut Ui, on: bool, text: &str) -> Response {
    let rt = if on {
        egui::RichText::new(text)
            .size(13.0)
            .family(semibold_family())
            .color(TEXT)
    } else {
        egui::RichText::new(text).size(13.0).color(MUTED)
    };
    ui.add(
        egui::Button::new(rt)
            .fill(if on { ACCENT_SOFT } else { RAISED })
            .stroke(if on {
                Stroke::new(1.0, ACCENT)
            } else {
                Stroke::new(1.0, BORDER)
            })
            .corner_radius(255)
            .min_size(egui::vec2(0.0, 30.0)),
    )
}

/// Four little bars for the quality meter.
pub fn quality_bars(ui: &mut Ui, filled: usize) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(4.0 * 14.0 + 3.0 * 3.0, 6.0), Sense::hover());
    for i in 0..4 {
        let r = egui::Rect::from_min_size(
            rect.min + egui::vec2(i as f32 * 17.0, 0.0),
            egui::vec2(14.0, 6.0),
        );
        ui.painter()
            .rect_filled(r, 3, if i < filled { SUCCESS } else { HOVER });
    }
}

pub fn small(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(11.5).color(MUTED)
}

pub fn strong(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .family(semibold_family())
        .color(TEXT)
}

/// Uppercase caption above a group of controls.
pub fn caption(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .size(10.5)
        .family(semibold_family())
        .color(FAINT)
        .extra_letter_spacing(0.8)
}

/// A vertical divider between toolbar groups.
pub fn vsep(ui: &mut Ui, h: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, h), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(4.0),
        Stroke::new(1.0, BORDER),
    );
}

/// Frame for floating dialogs (Export).
pub fn dialog_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(14)
        .inner_margin(0)
        .shadow(egui::epaint::Shadow {
            offset: [0, 24],
            blur: 64,
            spread: 0,
            color: Color32::from_black_alpha(170),
        })
}
