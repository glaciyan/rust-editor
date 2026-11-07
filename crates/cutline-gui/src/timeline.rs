//! The timeline: every video end to end as thumbnails, cut-out stretches
//! striped dark, parts outlined (the selected one blue, with a handle at each
//! end), part labels underneath, and the playhead.

use crate::app::CutlineApp;
use crate::theme::{self, ACCENT, MUTED, ON_ACCENT};
use cutline_core::{format, Handle, PartMode, PartRef, Timestamp};
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke};

const RULER_H: f32 = 20.0;
const STRIP_H: f32 = 68.0;
const LABELS_GAP: f32 = 6.0;
const LABELS_H: f32 = 18.0;
const HANDLE_W: f32 = 10.0;
const GRAB: f32 = 10.0;
/// How far a part's outline reaches above and below the film strip.
const OUTLINE_OVERHANG: f32 = 3.0;

#[derive(Default)]
pub struct TimelineView {
    drag: Option<Drag>,
    /// Where the last right-click landed: (clip, clip-local time).
    menu_at: Option<(u64, Timestamp)>,
}

#[derive(Clone, Copy)]
enum Drag {
    Handle(PartRef, Handle),
    Scrub,
}

pub enum TimelineAction {
    None,
    Seek(u64, Timestamp),
    DragStart(u64, Timestamp),
    Dragging(u64, Timestamp),
    /// Clip, time, and whether it was a start handle.
    DragEnd(u64, Timestamp, bool),
    RemoveClip(u64),
    MoveClip(u64, isize),
    AddPart(u64, Timestamp),
    RemovePart(PartRef),
    SetMode(PartMode),
}

struct Geometry {
    ruler: Rect,
    strip: Rect,
    total: f64,
}

impl Geometry {
    fn x(&self, t: f64) -> f32 {
        self.strip.left() + (t / self.total.max(1e-6)) as f32 * self.strip.width()
    }
    fn t(&self, x: f32) -> f64 {
        (((x - self.strip.left()) / self.strip.width()).clamp(0.0, 1.0) as f64) * self.total
    }
}

/// Outline colour of a part: blue when selected; in Remove mode the parts
/// are what's cut, so they're drawn red.
fn part_color(mode: PartMode, selected: bool) -> Color32 {
    match (mode, selected) {
        (PartMode::Keep, true) => ACCENT,
        (PartMode::Keep, false) => theme::PART_OUTLINE,
        (PartMode::Remove, true) => theme::DANGER,
        (PartMode::Remove, false) => theme::DANGER.linear_multiply(0.6),
    }
}

pub fn show(ui: &mut egui::Ui, app: &mut CutlineApp) -> TimelineAction {
    let mut action = TimelineAction::None;
    ui.spacing_mut().item_spacing.y = 0.0;
    let (outer, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), RULER_H + STRIP_H + OUTLINE_OVERHANG),
        Sense::click_and_drag(),
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Timeline"));
    let ruler = Rect::from_min_size(outer.min, egui::vec2(outer.width(), RULER_H));
    let strip = Rect::from_min_size(
        egui::pos2(outer.left(), ruler.bottom()),
        egui::vec2(outer.width(), STRIP_H),
    );
    let geo = Geometry {
        ruler,
        strip,
        total: app.project.total_duration().secs(),
    };
    if app.project.is_empty() || geo.total <= 0.0 {
        let p = ui.painter();
        p.rect(
            strip,
            8,
            theme::BG,
            Stroke::new(1.0, theme::HAIRLINE),
            egui::StrokeKind::Inside,
        );
        p.text(
            strip.center(),
            egui::Align2::CENTER_CENTER,
            "Your videos appear here",
            egui::FontId::proportional(12.5),
            MUTED,
        );
        ui.allocate_exact_size(
            egui::vec2(strip.width(), LABELS_GAP + LABELS_H),
            Sense::hover(),
        );
        return action;
    }
    draw_ruler(ui, &geo);
    draw(ui, app, &geo);
    action = interact(ui, app, &geo, &resp);
    hover_line(ui, app, &geo, &resp);
    labels(ui, app, &geo);
    action
}

/// Pick a tick step so labels sit at least `min_px` apart.
fn tick_step(total: f64, width: f32, min_px: f32) -> f64 {
    const STEPS: [f64; 17] = [
        0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0,
        3600.0, 7200.0,
    ];
    let px_per_s = width as f64 / total.max(1e-6);
    STEPS
        .iter()
        .copied()
        .find(|s| s * px_per_s >= min_px as f64)
        .unwrap_or(7200.0)
}

/// Time labels only, no tick marks: the first sits flush left, the last
/// flush right, the rest centred on their time.
fn draw_ruler(ui: &egui::Ui, geo: &Geometry) {
    let p = ui.painter();
    let r = geo.ruler;
    let step = tick_step(geo.total, r.width(), 120.0);
    let n = (geo.total / step + 1e-9).floor() as usize;
    let mut last_right = f32::MIN;
    for i in 0..=n {
        let t = i as f64 * step;
        let text = if step < 1.0 {
            format::clock_precise(t)
        } else {
            format::clock(t)
        };
        let g = p.layout_no_wrap(text, theme::MONO(11.0), MUTED);
        let w = g.size().x;
        let x = geo.x(t);
        let left = if i == 0 {
            x
        } else {
            (x - w / 2.0).min(r.right() - w)
        };
        if left < last_right + 8.0 {
            continue;
        }
        last_right = left + w;
        p.galley(egui::pos2(left, r.top()), g, MUTED);
    }
}

fn draw(ui: &mut egui::Ui, app: &mut CutlineApp, geo: &Geometry) {
    let painter = ui.painter().clone();
    let ppp = ui.ctx().pixels_per_point();
    let strip = geo.strip;
    let multi = app.project.clips.len() > 1;
    let clips: Vec<_> = app
        .project
        .clips
        .iter()
        .map(|c| {
            (
                c.id,
                app.project.clip_offset(c.id).secs(),
                c.duration.secs(),
                c.name(),
            )
        })
        .collect();

    // Thumbnails, one rounded film strip per video.
    for (id, off, dur, name) in &clips {
        let x0 = geo.x(*off) + if multi { 1.5 } else { 0.0 };
        let x1 = geo.x(off + dur) - if multi { 1.5 } else { 0.0 };
        let rect = Rect::from_x_y_ranges(x0..=x1.max(x0 + 1.0), strip.y_range());
        let aspect = app
            .source_for(*id)
            .and_then(|m| m.video.as_ref())
            .map(|v| {
                let (w, h) = v.display_size();
                w as f32 / h.max(1) as f32
            })
            .unwrap_or(16.0 / 9.0);
        let tile_w = STRIP_H * aspect;
        let count = (((rect.width() / tile_w).ceil() as usize).div_ceil(4) * 4).clamp(4, 200);
        app.want_thumbs(*id, count, (STRIP_H * ppp) as usize);
        let w = rect.width() / count as f32;
        let clip_painter = painter.with_clip_rect(rect);
        clip_painter.rect_filled(rect, 8, theme::RAISED);
        for j in 0..count {
            let r = Rect::from_x_y_ranges(
                rect.left() + j as f32 * w..=rect.left() + (j + 1) as f32 * w,
                strip.y_range(),
            );
            if let Some(tex) = app.thumb_tex.get(&(*id, count, j)) {
                // Crop the picture to the tile's shape instead of squashing it.
                let ta = tex.aspect_ratio();
                let ra = r.width() / r.height();
                let uv = if ra < ta {
                    let k = ra / ta;
                    Rect::from_min_max(
                        egui::pos2(0.5 - k / 2.0, 0.0),
                        egui::pos2(0.5 + k / 2.0, 1.0),
                    )
                } else {
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
                };
                clip_painter.image(tex.id(), r, uv, Color32::WHITE);
            } else {
                clip_painter.vline(r.right(), r.y_range(), Stroke::new(1.0, theme::HAIRLINE));
            }
        }
        // Round the strip's corners by painting the panel over them.
        painter.rect_stroke(
            rect.expand(3.0),
            11,
            Stroke::new(6.0, theme::PANEL),
            egui::StrokeKind::Inside,
        );
        if multi && rect.width() > 60.0 {
            let galley = painter.layout_no_wrap(
                name.clone(),
                egui::FontId::new(11.0, theme::semibold_family()),
                Color32::WHITE,
            );
            let w = galley.size().x.min(rect.width() - 16.0);
            let bg = Rect::from_min_size(
                rect.left_top() + egui::vec2(6.0, 6.0),
                egui::vec2(w + 10.0, 17.0),
            );
            painter
                .with_clip_rect(rect)
                .rect_filled(bg, 4, Color32::from_black_alpha(170));
            painter.with_clip_rect(bg.shrink(1.0)).galley(
                bg.min + egui::vec2(5.0, 1.5),
                galley,
                Color32::WHITE,
            );
        }
    }

    // Darken and stripe what will be cut out.
    let mode = app.project.mode;
    for c in &app.project.clips {
        let off = app.project.clip_offset(c.id).secs();
        let mut cur = 0.0;
        let shade = |a: f64, b: f64| {
            if b - a > 1e-6 {
                let r = Rect::from_x_y_ranges(geo.x(off + a)..=geo.x(off + b), strip.y_range());
                painter.rect_filled(r, 0, Color32::from_black_alpha(168));
                let hp = painter.with_clip_rect(r.intersect(painter.clip_rect()));
                // 45° bands, 6 px wide and 12 px apart.
                let period = 17.0;
                let mut x = (r.left() / period).floor() * period - r.height();
                while x < r.right() + r.height() {
                    hp.line_segment(
                        [
                            egui::pos2(x, r.bottom()),
                            egui::pos2(x + r.height(), r.top()),
                        ],
                        Stroke::new(6.0, Color32::from_black_alpha(90)),
                    );
                    x += period;
                }
            }
        };
        for (a, b) in c.kept_ranges(mode) {
            shade(cur, a.secs());
            cur = b.secs();
        }
        shade(cur, c.duration.secs());
    }

    // Outline the parts; the selected one gets a handle at each end.
    let selected = app.project.selected;
    for c in &app.project.clips {
        let off = app.project.clip_offset(c.id).secs();
        for p in &c.parts {
            let xa = geo.x(off + p.start.secs());
            let xb = geo.x(off + p.end.secs());
            let is_sel = selected
                == Some(PartRef {
                    clip: c.id,
                    part: p.id,
                });
            let col = part_color(mode, is_sel);
            let outline = Rect::from_x_y_ranges(
                xa..=xb,
                strip.top() - OUTLINE_OVERHANG..=strip.bottom() + OUTLINE_OVERHANG,
            );
            painter.rect_stroke(outline, 7, Stroke::new(2.0, col), egui::StrokeKind::Inside);
            if !is_sel || outline.width() < HANDLE_W * 2.0 + 4.0 {
                continue;
            }
            for start in [true, false] {
                let h = if start {
                    Rect::from_x_y_ranges(xa..=xa + HANDLE_W, outline.y_range())
                } else {
                    Rect::from_x_y_ranges(xb - HANDLE_W..=xb, outline.y_range())
                };
                let cr = if start {
                    egui::CornerRadius {
                        nw: 6,
                        sw: 6,
                        ne: 0,
                        se: 0,
                    }
                } else {
                    egui::CornerRadius {
                        nw: 0,
                        sw: 0,
                        ne: 6,
                        se: 6,
                    }
                };
                painter.rect_filled(h, cr, col);
                painter.rect_filled(
                    Rect::from_center_size(h.center(), egui::vec2(2.0, 18.0)),
                    1,
                    ON_ACCENT.gamma_multiply(0.6),
                );
            }
        }
    }

    // Playhead.
    if let Some((clip, t)) = app.playhead {
        let x = geo.x(app.project.clip_offset(clip).secs() + t.secs());
        let top = geo.ruler.top() + 14.0;
        painter.vline(
            x,
            top..=strip.bottom() + 6.0,
            Stroke::new(2.0, theme::PLAYHEAD),
        );
        let head = Rect::from_min_size(egui::pos2(x - 6.0, top - 2.0), egui::vec2(12.0, 10.0));
        painter.rect_filled(
            head,
            egui::CornerRadius {
                nw: 3,
                ne: 3,
                sw: 6,
                se: 6,
            },
            theme::PLAYHEAD,
        );
    }
}

/// A faint line under the pointer, so you can see where a click will land.
fn hover_line(ui: &egui::Ui, app: &CutlineApp, geo: &Geometry, resp: &egui::Response) {
    if app.timeline.drag.is_some() || !resp.hovered() {
        return;
    }
    let Some(pos) = ui.ctx().pointer_hover_pos() else {
        return;
    };
    if handle_at(app, geo, pos).is_some() {
        return;
    }
    let x = pos.x.clamp(geo.strip.left(), geo.strip.right());
    ui.painter().vline(
        x,
        geo.ruler.bottom()..=geo.strip.bottom(),
        Stroke::new(1.0, Color32::from_white_alpha(70)),
    );
}

/// Handle nearest to the pointer, if close enough to grab.
fn handle_at(app: &CutlineApp, geo: &Geometry, pos: Pos2) -> Option<(PartRef, Handle)> {
    if pos.y < geo.strip.top() - 10.0 || pos.y > geo.strip.bottom() + 10.0 {
        return None;
    }
    let mut best: Option<(f32, PartRef, Handle)> = None;
    for c in &app.project.clips {
        let off = app.project.clip_offset(c.id).secs();
        for p in &c.parts {
            let r = PartRef {
                clip: c.id,
                part: p.id,
            };
            for (h, t) in [(Handle::Start, p.start), (Handle::End, p.end)] {
                // Handles sit inside the part, so aim at their middle.
                let inward = if h == Handle::Start {
                    HANDLE_W / 2.0
                } else {
                    -HANDLE_W / 2.0
                };
                let d = (geo.x(off + t.secs()) + inward - pos.x).abs();
                // Prefer the selected part's handles when two sit together.
                let sel_bonus = if app.project.selected == Some(r) {
                    -0.5
                } else {
                    0.0
                };
                if d <= GRAB && best.map(|b| d + sel_bonus < b.0).unwrap_or(true) {
                    best = Some((d + sel_bonus, r, h));
                }
            }
        }
    }
    best.map(|(_, r, h)| (r, h))
}

fn part_at(app: &CutlineApp, clip: u64, t: Timestamp) -> Option<PartRef> {
    app.project
        .clip(clip)?
        .parts
        .iter()
        .find(|p| p.contains(t))
        .map(|p| PartRef { clip, part: p.id })
}

fn interact(
    ui: &mut egui::Ui,
    app: &mut CutlineApp,
    geo: &Geometry,
    resp: &egui::Response,
) -> TimelineAction {
    let pointer = ui.ctx().pointer_latest_pos();
    let hover_handle = pointer.and_then(|p| {
        if resp.hovered() || resp.contains_pointer() {
            handle_at(app, geo, p)
        } else {
            None
        }
    });
    if hover_handle.is_some() || matches!(app.timeline.drag, Some(Drag::Handle(..))) {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    let locate = |app: &CutlineApp, x: f32| app.project.locate(Timestamp::from_secs(geo.t(x)));

    if resp.drag_started() {
        // Hit-test where the press began, not where the pointer is once the drag registers.
        let origin = ui
            .input(|i| i.pointer.press_origin())
            .or(resp.interact_pointer_pos());
        if let Some(p) = origin {
            match handle_at(app, geo, p) {
                Some((r, h)) => {
                    app.timeline.drag = Some(Drag::Handle(r, h));
                    app.project.selected = Some(r);
                    let t = app
                        .project
                        .part(r)
                        .map(|p| if h == Handle::Start { p.start } else { p.end })
                        .unwrap_or_default();
                    return TimelineAction::DragStart(r.clip, t);
                }
                None => {
                    app.timeline.drag = Some(Drag::Scrub);
                    if let Some((c, t)) = locate(app, p.x) {
                        return TimelineAction::Seek(c, t);
                    }
                }
            }
        }
    }
    if resp.dragged() {
        if let (Some(d), Some(p)) = (app.timeline.drag, resp.interact_pointer_pos()) {
            match d {
                Drag::Handle(r, h) => {
                    // Time inside the handle's own clip, even if the pointer leaves it.
                    let off = app.project.clip_offset(r.clip).secs();
                    let t = Timestamp::from_secs(geo.t(p.x) - off);
                    if let Some(landed) = app.project.move_handle(r, h, t) {
                        tooltip(ui, geo, p.x, &format::clock_precise(landed.secs()));
                        return TimelineAction::Dragging(r.clip, landed);
                    }
                }
                Drag::Scrub => {
                    if let Some((c, t)) = locate(app, p.x) {
                        return TimelineAction::Seek(c, t);
                    }
                }
            }
        }
    }
    if resp.drag_stopped() {
        if let Some(Drag::Handle(r, h)) = app.timeline.drag.take() {
            let t = app
                .project
                .part(r)
                .map(|p| if h == Handle::Start { p.start } else { p.end })
                .unwrap_or_default();
            return TimelineAction::DragEnd(r.clip, t, h == Handle::Start);
        }
        app.timeline.drag = None;
    }
    if resp.double_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some((c, t)) = locate(app, p.x) {
                if part_at(app, c, t).is_none() {
                    return TimelineAction::AddPart(c, t);
                }
            }
        }
    }
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some((c, t)) = locate(app, p.x) {
                if let Some(r) = part_at(app, c, t) {
                    app.project.selected = Some(r);
                }
                return TimelineAction::Seek(c, t);
            }
        }
    }
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            app.timeline.menu_at = locate(app, p.x);
        }
    }
    let mut action = TimelineAction::None;
    resp.context_menu(|ui| {
        let Some((c, t)) = app.timeline.menu_at else {
            return;
        };
        let part = part_at(app, c, t);
        let multi = app.project.clips.len() > 1;
        if part.is_none() && ui.button("Add part here").clicked() {
            action = TimelineAction::AddPart(c, t);
            ui.close();
        }
        if let Some(r) = part {
            if ui.button("Remove this part").clicked() {
                action = TimelineAction::RemovePart(r);
                ui.close();
            }
        }
        ui.separator();
        let (text, other) = match app.project.mode {
            PartMode::Keep => ("Cut out the marked parts instead", PartMode::Remove),
            PartMode::Remove => ("Keep only the marked parts", PartMode::Keep),
        };
        if ui.button(text).clicked() {
            action = TimelineAction::SetMode(other);
            ui.close();
        }
        if multi {
            ui.separator();
            let idx = app.project.clip_index(c).unwrap_or(0);
            if ui
                .add_enabled(idx > 0, egui::Button::new("Move video earlier"))
                .clicked()
            {
                action = TimelineAction::MoveClip(c, -1);
                ui.close();
            }
            if ui
                .add_enabled(
                    idx + 1 < app.project.clips.len(),
                    egui::Button::new("Move video later"),
                )
                .clicked()
            {
                action = TimelineAction::MoveClip(c, 1);
                ui.close();
            }
        }
        if ui.button("Remove video from timeline").clicked() {
            action = TimelineAction::RemoveClip(c);
            ui.close();
        }
    });
    action
}

fn tooltip(ui: &egui::Ui, geo: &Geometry, x: f32, text: &str) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("handle-time"),
    ));
    let galley = painter.layout_no_wrap(text.to_string(), theme::MONO(12.0), ON_ACCENT);
    let size = galley.size() + egui::vec2(14.0, 8.0);
    let r = Rect::from_center_size(egui::pos2(x, geo.ruler.top() - 10.0), size);
    painter.rect_filled(r, 5, ACCENT);
    painter.galley(r.min + egui::vec2(7.0, 4.0), galley, ON_ACCENT);
}

/// `Part 1 · 3.2 s` under each part, the selected one blue.
fn labels(ui: &mut egui::Ui, app: &CutlineApp, geo: &Geometry) {
    ui.add_space(LABELS_GAP);
    let (row, _) = ui.allocate_exact_size(egui::vec2(geo.strip.width(), LABELS_H), Sense::hover());
    let painter = ui.painter();
    let mut last_right = f32::MIN;
    let mut n = 0;
    for c in &app.project.clips {
        let off = app.project.clip_offset(c.id).secs();
        for p in &c.parts {
            n += 1;
            let is_sel = app.project.selected
                == Some(PartRef {
                    clip: c.id,
                    part: p.id,
                });
            let mid = geo.x(off + (p.start.secs() + p.end.secs()) / 2.0);
            let text = match app.project.mode {
                PartMode::Keep => format!("Part {n} · {}", format::length(p.len().secs())),
                PartMode::Remove => format!("Cut {n} · {}", format::length(p.len().secs())),
            };
            let (font, color) = if is_sel {
                (
                    egui::FontId::new(12.0, theme::semibold_family()),
                    part_color(app.project.mode, true),
                )
            } else {
                (egui::FontId::proportional(12.0), MUTED)
            };
            let galley = painter.layout_no_wrap(text, font, color);
            let w = galley.size().x;
            let left = (mid - w / 2.0).clamp(row.left(), (row.right() - w).max(row.left()));
            if left < last_right + 8.0 {
                continue;
            }
            painter.galley(egui::pos2(left, row.top()), galley, color);
            last_right = left + w;
        }
    }
}
