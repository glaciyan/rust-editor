//! The timeline: every video end to end as thumbnails, cut-out parts shaded,
//! marked parts outlined with a handle at each end, and the playhead.

use crate::app::CutlineApp;
use crate::theme::{self, FAINT, MUTED, TEXT, TRIM};
use cutline_core::{format, Handle, PartMode, PartRef, Timestamp};
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke};

const RULER_H: f32 = 20.0;
const STRIP_H: f32 = 56.0;
const LABELS_H: f32 = 18.0;
const HANDLE_W: f32 = 10.0;
const GRAB: f32 = 10.0;

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
    Checkpoint,
    RemoveClip(u64),
    MoveClip(u64, isize),
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

/// Colour of the part outlines: amber for parts you keep, red for parts you cut.
fn part_color(mode: PartMode) -> Color32 {
    match mode {
        PartMode::Keep => TRIM,
        PartMode::Remove => theme::DANGER,
    }
}

pub fn show(ui: &mut egui::Ui, app: &mut CutlineApp) -> TimelineAction {
    let mut action = TimelineAction::None;
    let frame = egui::Frame::new()
        .fill(theme::BG)
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .corner_radius(10)
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 6,
            bottom: 4,
        });
    frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (outer, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), RULER_H + STRIP_H + 8.0),
            Sense::click_and_drag(),
        );
        let ruler = Rect::from_min_size(outer.min, egui::vec2(outer.width(), RULER_H));
        let strip = Rect::from_min_size(
            egui::pos2(outer.left(), ruler.bottom() + 4.0),
            egui::vec2(outer.width(), STRIP_H),
        );
        let geo = Geometry {
            ruler,
            strip,
            total: app.project.total_duration().secs(),
        };
        if app.project.is_empty() || geo.total <= 0.0 {
            draw_ruler(ui, &geo, false);
            let p = ui.painter();
            p.rect(
                strip,
                6,
                theme::PANEL,
                Stroke::new(1.0, theme::HAIRLINE),
                egui::StrokeKind::Inside,
            );
            p.text(
                strip.center(),
                egui::Align2::CENTER_CENTER,
                "Your videos appear here",
                egui::FontId::proportional(12.5),
                FAINT,
            );
            ui.allocate_exact_size(egui::vec2(strip.width(), LABELS_H), Sense::hover());
            return;
        }
        draw_ruler(ui, &geo, true);
        draw(ui, app, &geo);
        action = interact(ui, app, &geo, &resp);
        hover_line(ui, app, &geo, &resp);
        labels(ui, app, &geo);
    });
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

fn draw_ruler(ui: &egui::Ui, geo: &Geometry, labelled: bool) {
    let p = ui.painter();
    let r = geo.ruler;
    p.hline(r.x_range(), r.bottom(), Stroke::new(1.0, theme::HAIRLINE));
    if !labelled {
        return;
    }
    let step = tick_step(geo.total, r.width(), 84.0);
    let minor = step / 5.0;
    let n_minor = (geo.total / minor).floor() as usize;
    for i in 0..=n_minor {
        let t = i as f64 * minor;
        let x = geo.x(t);
        let major = i % 5 == 0;
        let h = if major { 8.0 } else { 4.0 };
        p.vline(
            x,
            r.bottom() - h..=r.bottom(),
            Stroke::new(
                1.0,
                if major {
                    theme::BORDER_STRONG
                } else {
                    theme::BORDER
                },
            ),
        );
        if major {
            let text = if step < 1.0 {
                format::clock_precise(t)
            } else {
                format::clock(t)
            };
            let g = p.layout_no_wrap(text, egui::FontId::monospace(10.5), FAINT);
            let gx = (x + 4.0).min(r.right() - g.size().x);
            if gx > x - 2.0 {
                p.galley(egui::pos2(gx, r.top() + 1.0), g, FAINT);
            }
        }
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
        clip_painter.rect_filled(rect, 6, theme::RAISED);
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
        // Round the strip's corners by painting the background over them.
        painter.rect_stroke(
            rect.expand(3.0),
            9,
            Stroke::new(6.0, theme::BG),
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
                rect.left_top() + egui::vec2(5.0, 5.0),
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

    // Shade and hatch what will be cut out.
    let mode = app.project.mode;
    for c in &app.project.clips {
        let off = app.project.clip_offset(c.id).secs();
        let mut cur = 0.0;
        let shade = |a: f64, b: f64| {
            if b - a > 1e-6 {
                let r = Rect::from_x_y_ranges(geo.x(off + a)..=geo.x(off + b), strip.y_range());
                painter.rect_filled(r, 0, theme::SHADE);
                let hp = painter.with_clip_rect(r.intersect(painter.clip_rect()));
                let mut x = r.left() - r.height();
                let x0 = (x / 9.0).floor() * 9.0;
                x = x0;
                while x < r.right() {
                    hp.line_segment(
                        [
                            egui::pos2(x, r.bottom()),
                            egui::pos2(x + r.height(), r.top()),
                        ],
                        Stroke::new(1.0, Color32::from_white_alpha(9)),
                    );
                    x += 9.0;
                }
            }
        };
        for (a, b) in c.kept_ranges(mode) {
            shade(cur, a.secs());
            cur = b.secs();
        }
        shade(cur, c.duration.secs());
    }

    // Outline the marked parts and draw their handles as trim brackets.
    let selected = app.project.selected;
    let color = part_color(mode);
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
            let col = if is_sel {
                color
            } else {
                color.linear_multiply(0.7)
            };
            let outline = Rect::from_x_y_ranges(xa..=xb, strip.top() - 2.0..=strip.bottom() + 2.0);
            if is_sel {
                painter.rect_filled(outline, 4, theme::TRIM_SOFT);
            }
            painter.rect_stroke(
                outline,
                4,
                Stroke::new(if is_sel { 2.5 } else { 2.0 }, col),
                egui::StrokeKind::Inside,
            );
            for (x, start) in [(xa, true), (xb, false)] {
                let h = Rect::from_x_y_ranges(
                    if start {
                        x..=x + HANDLE_W
                    } else {
                        x - HANDLE_W..=x
                    },
                    strip.top() - 2.0..=strip.bottom() + 2.0,
                );
                let cr = if start {
                    egui::CornerRadius {
                        nw: 4,
                        sw: 4,
                        ne: 0,
                        se: 0,
                    }
                } else {
                    egui::CornerRadius {
                        nw: 0,
                        sw: 0,
                        ne: 4,
                        se: 4,
                    }
                };
                painter.rect_filled(h, cr, col);
                // Grip lines.
                for dx in [-1.5, 1.5] {
                    painter.vline(
                        h.center().x + dx,
                        h.center().y - 7.0..=h.center().y + 7.0,
                        Stroke::new(
                            1.0,
                            Color32::from_black_alpha(if is_sel { 150 } else { 110 }),
                        ),
                    );
                }
            }
        }
    }

    // Playhead.
    if let Some((clip, t)) = app.playhead {
        let x = geo.x(app.project.clip_offset(clip).secs() + t.secs());
        let top = geo.ruler.top() + 2.0;
        painter.vline(
            x,
            top + 10.0..=strip.bottom() + 4.0,
            Stroke::new(2.0, theme::PLAYHEAD),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x - 6.0, top),
                egui::pos2(x + 6.0, top),
                egui::pos2(x + 6.0, top + 8.0),
                egui::pos2(x, top + 14.0),
                egui::pos2(x - 6.0, top + 8.0),
            ],
            theme::PLAYHEAD,
            Stroke::NONE,
        ));
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
        geo.ruler.top()..=geo.strip.bottom(),
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
            for (h, t) in [(Handle::Start, p.start), (Handle::End, p.end)] {
                let d = (geo.x(off + t.secs()) - pos.x).abs();
                // Prefer the selected part's handles when two sit together.
                let sel_bonus = if app.project.selected
                    == Some(PartRef {
                        clip: c.id,
                        part: p.id,
                    }) {
                    -0.5
                } else {
                    0.0
                };
                if d <= GRAB && best.map(|b| d + sel_bonus < b.0).unwrap_or(true) {
                    best = Some((
                        d + sel_bonus,
                        PartRef {
                            clip: c.id,
                            part: p.id,
                        },
                        h,
                    ));
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
                    app.checkpoint();
                    app.project.add_part(c, t);
                    return TimelineAction::None;
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
            app.checkpoint();
            app.project.add_part(c, t);
            ui.close();
        }
        if let Some(r) = part {
            if ui.button("Remove this part").clicked() {
                app.checkpoint();
                app.project.remove_part(r);
                ui.close();
            }
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
    if let Some(Drag::Handle(..)) = app.timeline.drag {
        // keep drawing the tooltip while the pointer is still
    }
    let _ = TimelineAction::Checkpoint;
    action
}

fn tooltip(ui: &egui::Ui, geo: &Geometry, x: f32, text: &str) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("handle-time"),
    ));
    let galley = painter.layout_no_wrap(
        text.to_string(),
        egui::FontId::monospace(12.0),
        theme::VIEWER,
    );
    let size = galley.size() + egui::vec2(14.0, 8.0);
    let r = Rect::from_center_size(egui::pos2(x, geo.ruler.top() - 12.0), size);
    painter.rect_filled(r, 5, TRIM);
    painter.galley(r.min + egui::vec2(7.0, 4.0), galley, theme::VIEWER);
}

fn labels(ui: &mut egui::Ui, app: &CutlineApp, geo: &Geometry) {
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
                (egui::FontId::new(11.5, theme::semibold_family()), TEXT)
            } else {
                (egui::FontId::proportional(11.5), MUTED)
            };
            let galley = painter.layout_no_wrap(text, font, color);
            let w = galley.size().x;
            let left = (mid - w / 2.0).clamp(row.left(), row.right() - w);
            if left < last_right + 8.0 {
                continue;
            }
            painter.galley(egui::pos2(left, row.top() + 3.0), galley, color);
            last_right = left + w;
        }
    }
}
