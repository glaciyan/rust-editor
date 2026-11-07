//! The look: a warm, quiet dark editing suite. Near-black surfaces so the
//! picture is the brightest thing on screen, one blue for actions and the
//! selected part, an orange playhead.
//!
//! Every hand-painted widget here reports what it is to AccessKit, so screen
//! readers (and the UI tests) can find it by name.

use crate::icons::Icon;
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Response, Role, Sense, Stroke, TextStyle, Ui,
    Widget, WidgetInfo,
};
use std::sync::Arc;

// Surfaces, darkest to lightest.
pub const VIEWER: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
pub const BG: Color32 = Color32::from_rgb(0x13, 0x12, 0x10);
pub const SECTION: Color32 = Color32::from_rgb(0x19, 0x17, 0x14);
pub const PANEL: Color32 = Color32::from_rgb(0x1d, 0x1b, 0x18);
pub const RAISED: Color32 = Color32::from_rgb(0x23, 0x21, 0x1d);
pub const RAISED_HOVER: Color32 = Color32::from_rgb(0x28, 0x26, 0x21);
pub const HOVER: Color32 = Color32::from_rgb(0x2f, 0x2c, 0x26);
pub const SEGMENT_ON: Color32 = Color32::from_rgb(0x32, 0x2f, 0x29);
pub const HAIRLINE: Color32 = Color32::from_rgb(0x30, 0x2c, 0x26);
pub const BORDER: Color32 = Color32::from_rgb(0x3b, 0x37, 0x30);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x4d, 0x48, 0x40);

// Text.
pub const TEXT: Color32 = Color32::from_rgb(0xec, 0xe8, 0xe1);
pub const MUTED: Color32 = Color32::from_rgb(0xa9, 0xa0, 0x96);
/// Disabled controls only; too dim for text that has to be read.
pub const FAINT: Color32 = Color32::from_rgb(0x5c, 0x55, 0x4d);

// Accents.
pub const ACCENT: Color32 = Color32::from_rgb(0x4f, 0x9b, 0xf7);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x3d, 0x8b, 0xef);
pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(0x33, 0x7c, 0xdc);
/// Text and icons on an accent fill.
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x13, 0x12, 0x10);
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_premultiplied(13, 25, 40, 41);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xf9, 0x80, 0x12);
pub const DANGER: Color32 = Color32::from_rgb(0xf5, 0x80, 0x6f);
pub const SUCCESS: Color32 = Color32::from_rgb(0x47, 0xb1, 0x81);
/// Border of a focused text field.
pub const FIELD_FOCUS: Color32 = Color32::from_rgb(0xaf, 0x7e, 0x55);
pub const FIELD_GLOW: Color32 = Color32::from_rgba_premultiplied(42, 30, 20, 61);
/// Outline of a part that isn't selected.
pub const PART_OUTLINE: Color32 = Color32::from_rgba_premultiplied(130, 128, 124, 140);
pub const BACKDROP: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 140);

pub const MONO: fn(f32) -> FontId = FontId::monospace;

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

/// Fonts: the desktop's own when `system` (the app), egui's bundled ones
/// otherwise (the tests, so they don't depend on the machine).
fn install_fonts(ctx: &egui::Context, system: bool) {
    let mut fonts = egui::FontDefinitions::default();
    let regular = system
        .then(|| system_font("sans-serif:style=Regular"))
        .flatten();
    let semibold = system
        .then(|| {
            system_font("sans-serif:weight=semibold")
                .or_else(|| system_font("sans-serif:style=Bold"))
        })
        .flatten();
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
    install_fonts(ctx, true);
    apply_style(ctx);
}

/// The look without the desktop's fonts.
#[cfg(test)]
pub fn apply_for_tests(ctx: &egui::Context) {
    install_fonts(ctx, false);
    apply_style(ctx);
}

fn apply_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(15.5, semibold_family())),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
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
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.interact_size.y = 32.0;
    style.spacing.combo_height = 320.0;
    style.spacing.menu_margin = egui::Margin::same(4);
    style.spacing.window_margin = egui::Margin::same(16);
    let v = &mut style.visuals;
    v.dark_mode = true;
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.text_edit_bg_color = Some(BG);
    v.faint_bg_color = RAISED;
    v.code_bg_color = BG;
    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(MUTED);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0x4f, 0x9b, 0xf7, 70);
    v.selection.stroke = Stroke::new(1.0, FIELD_FOCUS);
    v.text_cursor.stroke = Stroke::new(2.0, TEXT);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_fill = RAISED;
    v.widgets.inactive.weak_bg_fill = RAISED;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.bg_fill = RAISED_HOVER;
    v.widgets.hovered.weak_bg_fill = RAISED_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.bg_fill = HOVER;
    v.widgets.active.weak_bg_fill = HOVER;
    v.widgets.active.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.open.bg_fill = RAISED_HOVER;
    v.widgets.open.weak_bg_fill = RAISED_HOVER;
    v.widgets.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.corner_radius = CornerRadius::same(8);
        w.expansion = 0.0;
    }
    ctx.set_global_style(style);
}

// ---------- buttons ----------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Filled blue: the one thing to do next (Export, Save).
    Primary,
    /// Raised with a border.
    Secondary,
    /// No frame until hovered (toolbar actions).
    Ghost,
    /// Ghost, red text on hover (Delete).
    Danger,
}

/// A button painted by hand so every state (hover, press, disabled) looks
/// deliberate, with an optional icon in front of the text and an optional
/// keycap after it.
pub struct Btn {
    text: String,
    label: Option<String>,
    icon: Option<Icon>,
    kbd: Option<String>,
    kind: Kind,
    height: f32,
    min_width: f32,
}

impl Btn {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        Btn {
            text: text.into(),
            label: None,
            icon: None,
            kbd: None,
            kind,
            height: 32.0,
            min_width: 0.0,
        }
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    /// A keycap after the text, naming the shortcut.
    pub fn kbd(mut self, key: &str) -> Self {
        self.kbd = Some(key.to_string());
        self
    }
    /// The accessible name, for buttons that only show an icon.
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_string());
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
            FontId::new(13.0, semibold_family())
        } else {
            FontId::new(13.0, FontFamily::Proportional)
        };
        let galley = (!self.text.is_empty()).then(|| {
            ui.painter()
                .layout_no_wrap(self.text.clone(), font, Color32::PLACEHOLDER)
        });
        let kbd = self.kbd.as_ref().map(|k| {
            ui.painter()
                .layout_no_wrap(k.clone(), MONO(11.0), Color32::PLACEHOLDER)
        });
        let icon_w = if self.icon.is_some() { 15.0 } else { 0.0 };
        let gap = 7.0;
        let text_w = galley.as_ref().map(|g| g.size().x).unwrap_or(0.0);
        let kbd_w = kbd.as_ref().map(|g| g.size().x + 10.0).unwrap_or(0.0);
        let parts = [icon_w, text_w, kbd_w];
        let n = parts.iter().filter(|w| **w > 0.0).count();
        let content_w = parts.iter().sum::<f32>() + gap * n.saturating_sub(1) as f32;
        let (pad_l, pad_r) = match (galley.is_some(), kbd.is_some(), self.icon.is_some()) {
            (false, _, _) => (0.0, 0.0),
            (true, true, _) => (10.0, 8.0),
            (true, false, true) => (10.0, 12.0),
            (true, false, false) => (14.0, 14.0),
        };
        let w = (pad_l + content_w + pad_r)
            .max(self.min_width)
            .max(self.height);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, self.height), Sense::click());
        let enabled = ui.is_enabled();
        let name = self.label.clone().unwrap_or_else(|| self.text.clone());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &name));
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let hovered = enabled && resp.hovered();
        let pressed = enabled && resp.is_pointer_button_down_on();
        let (fill, stroke, fg, icon_fg) = match self.kind {
            Kind::Primary => {
                let fill = if !enabled {
                    RAISED
                } else if pressed {
                    ACCENT_PRESSED
                } else if hovered {
                    ACCENT_HOVER
                } else {
                    ACCENT
                };
                let fg = if enabled { ON_ACCENT } else { FAINT };
                (fill, Stroke::NONE, fg, fg)
            }
            Kind::Secondary => (
                if pressed {
                    HOVER
                } else if hovered {
                    RAISED_HOVER
                } else {
                    RAISED
                },
                Stroke::new(1.0, if hovered { BORDER_STRONG } else { BORDER }),
                if enabled { TEXT } else { FAINT },
                if enabled { MUTED } else { FAINT },
            ),
            Kind::Ghost | Kind::Danger => {
                let fill = if pressed || hovered {
                    HOVER
                } else {
                    Color32::TRANSPARENT
                };
                let danger = self.kind == Kind::Danger && hovered;
                let fg = if !enabled {
                    FAINT
                } else if danger {
                    DANGER
                } else if galley.is_none() && !hovered {
                    // Icon-only buttons are quieter until hovered.
                    MUTED
                } else {
                    TEXT
                };
                let icon_fg = if !enabled {
                    FAINT
                } else if danger {
                    DANGER
                } else if hovered {
                    TEXT
                } else {
                    MUTED
                };
                (fill, Stroke::NONE, fg, icon_fg)
            }
        };
        let radius = if self.kind == Kind::Ghost || self.kind == Kind::Danger {
            6
        } else {
            8
        };
        let p = ui.painter();
        p.rect(rect, radius, fill, stroke, egui::StrokeKind::Inside);
        let mut x = if galley.is_some() && self.min_width <= pad_l + content_w + pad_r {
            rect.left() + pad_l
        } else {
            rect.center().x - content_w / 2.0
        };
        let cy = rect.center().y;
        if let Some(icon) = self.icon {
            icon.paint(
                p,
                egui::Rect::from_center_size(
                    egui::pos2(x + icon_w / 2.0, cy),
                    egui::vec2(15.0, 15.0),
                ),
                icon_fg,
            );
            x += icon_w + gap;
        }
        if let Some(g) = galley {
            let gw = g.size().x;
            p.galley(egui::pos2(x, cy - g.size().y / 2.0), g, fg);
            x += gw + gap;
        }
        if let Some(g) = kbd {
            keycap_at(p, egui::pos2(x, cy), g, enabled);
        }
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp
    }
}

/// A keycap whose left edge sits at `left_center`.
fn keycap_at(p: &egui::Painter, left_center: egui::Pos2, g: Arc<egui::Galley>, enabled: bool) {
    let r = egui::Rect::from_min_size(
        egui::pos2(left_center.x, left_center.y - 9.0),
        egui::vec2(g.size().x + 10.0, 18.0),
    );
    p.rect_stroke(
        r,
        4,
        Stroke::new(1.0, BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    p.galley(
        r.center() - g.size() / 2.0,
        g,
        if enabled { MUTED } else { FAINT },
    );
}

/// A keycap in a row of text: `Set start and end with [I] [O]`.
pub fn keycap(ui: &mut Ui, key: &str) {
    let g = ui
        .painter()
        .layout_no_wrap(key.to_string(), MONO(11.0), Color32::PLACEHOLDER);
    let (r, _) = ui.allocate_exact_size(egui::vec2(g.size().x + 10.0, 18.0), Sense::hover());
    keycap_at(ui.painter(), r.left_center(), g, true);
}

/// A square icon-only button. `label` names it for screen readers;
/// `tip` (usually the label plus its shortcut) shows on hover.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: egui::Vec2, label: &str, tip: &str) -> Response {
    Btn::new(Kind::Ghost, "")
        .icon(icon)
        .label(label)
        .height(size.y)
        .min_width(size.x)
        .ui(ui)
        .on_hover_text(tip)
}

/// A segmented control: a row of options in one rounded track. With `width`
/// the segments share it equally; otherwise each fits its text.
/// Returns the index clicked this frame, if any.
pub fn segmented(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    options: &[&str],
    selected: usize,
    width: Option<f32>,
) -> Option<usize> {
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
            ui.painter()
                .layout_no_wrap(o.to_string(), f, Color32::PLACEHOLDER)
        })
        .collect();
    let h = 32.0;
    let pad = 3.0;
    let natural: Vec<f32> = galleys.iter().map(|g| g.size().x + 24.0).collect();
    let seg_w: Vec<f32> = match width {
        Some(w) => vec![
            (w - pad * 2.0 - (options.len() as f32 - 1.0) * 2.0) / options.len() as f32;
            options.len()
        ],
        None => natural,
    };
    let total = seg_w.iter().sum::<f32>() + pad * 2.0 + (options.len() as f32 - 1.0) * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, h), Sense::hover());
    let p = ui.painter().clone();
    p.rect(
        rect,
        8,
        BG,
        Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    let enabled = ui.is_enabled();
    let base = ui.scope_id().with(id_salt);
    let mut x = rect.left() + pad;
    let mut clicked = None;
    for (i, g) in galleys.into_iter().enumerate() {
        let r = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + pad),
            egui::vec2(seg_w[i], h - pad * 2.0),
        );
        let resp = ui.interact(r, base.with(i), Sense::click());
        let on = i == selected;
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, enabled, on, options[i]));
        if on {
            p.rect_filled(r, 6, SEGMENT_ON);
        } else if resp.hovered() && enabled {
            p.rect_filled(r, 6, RAISED);
        }
        let fg = if !enabled {
            FAINT
        } else if on || resp.hovered() {
            TEXT
        } else {
            MUTED
        };
        p.galley(r.center() - g.size() / 2.0, g, fg);
        if resp.clicked() && enabled {
            clicked = Some(i);
        }
        x += seg_w[i] + 2.0;
    }
    clicked
}

/// A one-line text field in the dialog style: dark well, border that turns
/// warm when focused, optional unit (`MB`, `.mp4`) inside on the right.
pub struct Field<'a> {
    text: &'a mut String,
    id_salt: egui::Id,
    label: String,
    width: f32,
    hint: String,
    suffix: Option<String>,
    right_align: bool,
    focus: bool,
    height: f32,
}

impl<'a> Field<'a> {
    pub fn new(text: &'a mut String, label: &str) -> Self {
        Field {
            text,
            id_salt: egui::Id::unique(("field", label.to_string())),
            label: label.to_string(),
            width: f32::INFINITY,
            hint: String::new(),
            suffix: None,
            right_align: false,
            focus: false,
            height: 32.0,
        }
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    pub fn hint(mut self, h: &str) -> Self {
        self.hint = h.to_string();
        self
    }
    pub fn suffix(mut self, s: &str) -> Self {
        self.suffix = Some(s.to_string());
        self
    }
    pub fn right_align(mut self) -> Self {
        self.right_align = true;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    /// Take keyboard focus this frame.
    pub fn focus(mut self, focus: bool) -> Self {
        self.focus = focus;
        self
    }
}

impl Widget for Field<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let w = if self.width.is_finite() {
            self.width
        } else {
            ui.available_width()
        }
        .max(40.0);
        let h = self.height;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), Sense::hover());
        let id = ui.scope_id().with(self.id_salt);
        let focused = ui.memory(|m| m.has_focus(id));
        let suffix = self.suffix.as_ref().map(|s| {
            ui.painter()
                .layout_no_wrap(s.clone(), FontId::proportional(12.5), MUTED)
        });
        let suffix_w = suffix.as_ref().map(|g| g.size().x + 8.0).unwrap_or(0.0);
        let p = ui.painter().clone();
        if focused {
            p.rect_stroke(
                rect.expand(1.5),
                9,
                Stroke::new(3.0, FIELD_GLOW),
                egui::StrokeKind::Middle,
            );
        }
        p.rect(
            rect,
            8,
            BG,
            Stroke::new(1.0, if focused { FIELD_FOCUS } else { BORDER }),
            egui::StrokeKind::Inside,
        );
        let inner = egui::Rect::from_min_max(
            rect.min + egui::vec2(10.0, 0.0),
            rect.max - egui::vec2(10.0 + suffix_w, 0.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let mut edit = egui::TextEdit::singleline(self.text)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(inner.width())
            .min_size(inner.size())
            .font(FontId::proportional(13.0))
            .align(egui::Align2::LEFT_CENTER)
            .hint_text(egui::RichText::new(self.hint.clone()).color(MUTED));
        if self.right_align {
            edit = edit.align(egui::Align2::RIGHT_CENTER);
        }
        let resp = child.add(edit);
        if self.focus {
            resp.request_focus();
        }
        if let Some(g) = suffix {
            p.galley(
                egui::pos2(
                    rect.right() - 10.0 - g.size().x,
                    rect.center().y - g.size().y / 2.0,
                ),
                g,
                MUTED,
            );
        }
        let label = self.label.clone();
        ui.ctx()
            .accesskit_node_builder(resp.id, |node| node.set_label(label));
        resp
    }
}

/// Small muted text.
pub fn small(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(12.0).color(MUTED)
}

pub fn strong(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .family(semibold_family())
        .color(TEXT)
}

/// A field label in a dialog form.
pub fn form_label(text: &str) -> egui::RichText {
    egui::RichText::new(text).size(12.0).color(MUTED)
}

/// A vertical divider between toolbar groups.
pub fn vsep(ui: &mut Ui, h: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(17.0, h), Sense::hover());
    ui.painter()
        .vline(rect.center().x, rect.y_range(), Stroke::new(1.0, BORDER));
}

/// Frame for dialogs.
pub fn dialog_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(12)
        .inner_margin(0)
        .shadow(egui::epaint::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
}

/// A thin progress bar.
pub fn progress_bar(ui: &mut Ui, fraction: f32, width: f32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(width, 4.0), Sense::hover());
    ui.painter().rect_filled(r, 2, HOVER);
    let mut f = r;
    f.set_width(width * fraction.clamp(0.0, 1.0));
    ui.painter().rect_filled(f, 2, ACCENT);
}
