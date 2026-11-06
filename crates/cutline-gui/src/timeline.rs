//! The timeline: every video end to end as thumbnails, cut-out parts shaded,
//! marked parts outlined with a handle at each end, and the playhead.

use crate::app::CutlineApp;
use crate::theme::{self, ACCENT, INK, MUTED};
use cutline_core::{format, Handle, PartMode, PartRef, Timestamp};
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke};

const STRIP_H: f32 = 48.0;
const HANDLE_W: f32 = 8.0;
const GRAB: f32 = 9.0;

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

pub fn show(ui: &mut egui::Ui, app: &mut CutlineApp) -> TimelineAction {
    let mut action = TimelineAction::None;
    let frame = egui::Frame::new()
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(13, 14));
    frame.show(ui, |ui| {
        let (outer, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), STRIP_H),
            Sense::click_and_drag(),
        );
        let geo = Geometry {
            strip: outer,
            total: app.project.total_duration().secs(),
        };
        if app.project.is_empty() || geo.total <= 0.0 {
            ui.painter().rect_filled(outer, 3, theme::WELL);
            ui.painter().text(
                outer.center(),
                egui::Align2::CENTER_CENTER,
                "Your timeline",
                egui::FontId::proportional(13.0),
                MUTED,
            );
            return;
        }
        draw(ui, app, &geo);
        action = interact(ui, app, &geo, &resp);
        labels(ui, app, &geo);
    });
    action
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

    // Thumbnails.
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
        for j in 0..count {
            let r = Rect::from_x_y_ranges(
                rect.left() + j as f32 * w..=rect.left() + (j + 1) as f32 * w,
                strip.y_range(),
            );
            let r = r.shrink2(egui::vec2(0.5, 0.0));
            match app.thumb_tex.get(&(*id, count, j)) {
                Some(tex) => {
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
                    painter.image(tex.id(), r, uv, Color32::WHITE);
                }
                None => {
                    painter.rect_filled(r, 3, theme::WELL);
                    painter.rect_stroke(
                        r,
                        3,
                        Stroke::new(1.0, theme::HAIRLINE),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        if multi && rect.width() > 60.0 {
            let galley = painter.layout_no_wrap(
                name.clone(),
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
            let w = galley.size().x.min(rect.width() - 8.0);
            let bg = Rect::from_min_size(
                rect.left_top() + egui::vec2(4.0, 4.0),
                egui::vec2(w + 8.0, 16.0),
            );
            painter
                .with_clip_rect(rect)
                .rect_filled(bg, 3, Color32::from_black_alpha(150));
            painter.with_clip_rect(bg.shrink(1.0)).galley(
                bg.min + egui::vec2(4.0, 1.5),
                galley,
                Color32::WHITE,
            );
        }
    }

    // Shade what will be cut out.
    let mode = app.project.mode;
    for c in &app.project.clips {
        let off = app.project.clip_offset(c.id).secs();
        let mut cur = 0.0;
        let shade = |a: f64, b: f64| {
            if b - a > 1e-6 {
                let r = Rect::from_x_y_ranges(geo.x(off + a)..=geo.x(off + b), strip.y_range());
                painter.rect_filled(r, 0, theme::SHADE);
            }
        };
        for (a, b) in c.kept_ranges(mode) {
            shade(cur, a.secs());
            cur = b.secs();
        }
        shade(cur, c.duration.secs());
    }

    // Outline the marked parts and draw their handles.
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
            let outline = Rect::from_x_y_ranges(xa..=xb, strip.top() - 2.0..=strip.bottom() + 2.0);
            if is_sel {
                painter.rect_filled(
                    outline,
                    3,
                    Color32::from_rgba_unmultiplied(47, 111, 214, 28),
                );
            }
            painter.rect_stroke(
                outline,
                3,
                Stroke::new(2.0, ACCENT),
                egui::StrokeKind::Inside,
            );
            for x in [xa, xb] {
                let h = Rect::from_center_size(
                    egui::pos2(x, strip.center().y),
                    egui::vec2(HANDLE_W, STRIP_H + 12.0),
                );
                painter.rect_filled(h, 3, ACCENT);
                if is_sel {
                    for dy in [-4.0, 0.0, 4.0] {
                        painter.line_segment(
                            [
                                egui::pos2(x - 2.0, h.center().y + dy),
                                egui::pos2(x + 2.0, h.center().y + dy),
                            ],
                            Stroke::new(1.0, Color32::from_white_alpha(200)),
                        );
                    }
                }
            }
        }
    }

    // Playhead.
    if let Some((clip, t)) = app.playhead {
        let x = geo.x(app.project.clip_offset(clip).secs() + t.secs());
        painter.line_segment(
            [
                egui::pos2(x, strip.top() - 8.0),
                egui::pos2(x, strip.bottom() + 8.0),
            ],
            Stroke::new(2.0, INK),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x - 5.0, strip.top() - 12.0),
                egui::pos2(x + 5.0, strip.top() - 12.0),
                egui::pos2(x, strip.top() - 6.0),
            ],
            INK,
            Stroke::NONE,
        ));
    }
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
        Color32::WHITE,
    );
    let size = galley.size() + egui::vec2(12.0, 6.0);
    let r = Rect::from_center_size(egui::pos2(x, geo.strip.top() - 26.0), size);
    painter.rect_filled(r, 4, INK);
    painter.galley(r.min + egui::vec2(6.0, 3.0), galley, Color32::WHITE);
}

fn labels(ui: &mut egui::Ui, app: &CutlineApp, geo: &Geometry) {
    let (row, _) = ui.allocate_exact_size(egui::vec2(geo.strip.width(), 16.0), Sense::hover());
    let painter = ui.painter();
    let mut last_right = f32::MIN;
    let mut n = 0;
    for c in &app.project.clips {
        let off = app.project.clip_offset(c.id).secs();
        for p in &c.parts {
            n += 1;
            let mid = geo.x(off + (p.start.secs() + p.end.secs()) / 2.0);
            let text = match app.project.mode {
                PartMode::Keep => format!("Part {n} · {}", format::length(p.len().secs())),
                PartMode::Remove => format!("Cut {n} · {}", format::length(p.len().secs())),
            };
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), INK);
            let w = galley.size().x;
            let left = (mid - w / 2.0).clamp(row.left(), row.right() - w);
            if left < last_right + 8.0 {
                continue;
            }
            painter.galley(egui::pos2(left, row.top()), galley, INK);
            last_right = left + w;
        }
    }
}
