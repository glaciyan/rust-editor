//! The timeline: every video end to end as thumbnails, cut-out stretches
//! striped dark, parts outlined (the selected one blue, with a handle at each
//! end), part labels underneath, and the playhead.
//!
//! It zooms: Ctrl+scroll or a pinch zooms around the pointer, scrolling pans,
//! and a thin bar under the labels shows and drags the visible stretch.

use crate::app::CutlineApp;
use crate::icons::Icon;
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
/// The scroll bar's row under the labels: a gap, then the bar.
const BAR_GAP: f32 = 6.0;
const BAR_H: f32 = 4.0;
/// At the deepest zoom a frame is this wide.
const MAX_PX_PER_FRAME: f64 = 30.0;
/// Frames get their own ticks on the ruler from this width up.
const FRAME_TICK_PX: f64 = 6.0;

#[derive(Default)]
pub struct TimelineView {
    drag: Option<Drag>,
    /// Where the last right-click landed: (clip, clip-local time).
    menu_at: Option<(u64, Timestamp)>,
    /// Seconds of timeline in view; `None` shows it all.
    span: Option<f64>,
    /// Where the view starts, in seconds of the whole timeline.
    start: f64,
    /// Width of the strip last frame, for zooming from the keyboard.
    width: f32,
    /// The playhead last frame, to bring it into view when it moves.
    last_playhead: Option<f64>,
    /// Dragging the scroll bar: where on its thumb it was grabbed (seconds).
    bar_grab: Option<f64>,
}

impl TimelineView {
    /// The stretch in view, as (start, length) in seconds.
    pub fn window(&self, total: f64) -> (f64, f64) {
        let total = total.max(1e-6);
        let span = self.span.unwrap_or(total).clamp(1e-6, total);
        (self.start.clamp(0.0, total - span), span)
    }

    pub fn is_zoomed(&self) -> bool {
        self.span.is_some()
    }

    /// The narrowest view: a frame `MAX_PX_PER_FRAME` wide.
    pub fn min_span(&self, fps: f64) -> f64 {
        let w = if self.width > 0.0 { self.width } else { 1000.0 };
        w as f64 / MAX_PX_PER_FRAME / fps.max(1.0)
    }

    /// Zoom in (`factor` > 1) or out, keeping `anchor` (seconds) where it is
    /// on screen.
    pub fn zoom(&mut self, total: f64, factor: f64, anchor: f64, min_span: f64) {
        let (start, span) = self.window(total);
        let frac = ((anchor - start) / span).clamp(0.0, 1.0);
        let new = (span / factor).clamp(min_span.min(total), total);
        if new >= total * 0.999 {
            self.fit();
            return;
        }
        self.span = Some(new);
        self.start = (anchor - frac * new).clamp(0.0, total - new);
    }

    pub fn fit(&mut self) {
        self.span = None;
        self.start = 0.0;
    }

    /// Scroll the view by `secs` (negative: earlier).
    pub fn pan(&mut self, total: f64, secs: f64) {
        let (start, span) = self.window(total);
        self.start = (start + secs).clamp(0.0, (total - span).max(0.0));
    }

    /// Scroll so `t` is in view: `paging` (while playing) flips to the next
    /// stretch, otherwise `t` lands in the middle.
    pub fn reveal(&mut self, total: f64, t: f64, paging: bool) {
        let (start, span) = self.window(total);
        if !self.is_zoomed() || (t >= start && t <= start + span) {
            return;
        }
        let lead = if paging { 0.1 } else { 0.5 };
        self.start = (t - lead * span).clamp(0.0, total - span);
    }
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
    SplitAt(u64, Timestamp),
    RemovePart(PartRef),
    SetMode(PartMode),
}

struct Geometry {
    ruler: Rect,
    strip: Rect,
    total: f64,
    /// The stretch in view, in seconds.
    start: f64,
    span: f64,
}

impl Geometry {
    fn new(ruler: Rect, strip: Rect, total: f64, view: &TimelineView) -> Self {
        let (start, span) = view.window(total);
        Geometry {
            ruler,
            strip,
            total,
            start,
            span,
        }
    }
    fn x(&self, t: f64) -> f32 {
        self.strip.left() + ((t - self.start) / self.span) as f32 * self.strip.width()
    }
    /// The time under `x`, kept to the stretch in view.
    fn t(&self, x: f32) -> f64 {
        let f = ((x - self.strip.left()) / self.strip.width()).clamp(0.0, 1.0) as f64;
        (self.start + f * self.span).clamp(0.0, self.total)
    }
    fn end(&self) -> f64 {
        self.start + self.span
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
    let total = app.project.total_duration().secs();
    app.timeline.width = strip.width();
    if app.project.is_empty() || total <= 0.0 {
        app.timeline.fit();
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
            egui::vec2(strip.width(), LABELS_GAP + LABELS_H + BAR_GAP + BAR_H),
            Sense::hover(),
        );
        return action;
    }
    steer(ui, app, &resp, ruler, strip, total);
    let geo = Geometry::new(ruler, strip, total, &app.timeline);
    // Nothing of the timeline spills past its edges when zoomed in.
    let clip = Rect::from_x_y_ranges(
        outer.left() - 7.0..=outer.right() + 7.0,
        ui.clip_rect().y_range(),
    )
    .intersect(ui.clip_rect());
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(ui.max_rect()));
    inner.set_clip_rect(clip);
    draw_ruler(&inner, app, &geo);
    draw(&mut inner, app, &geo);
    action = interact(ui, app, &geo, &resp);
    hover_line(&inner, app, &geo, &resp);
    labels(ui, app, &geo);
    scroll_bar(ui, app, &geo);
    action
}

/// Zoom, pan, follow the playhead and scroll while dragging near an edge,
/// before anything is drawn.
fn steer(
    ui: &egui::Ui,
    app: &mut CutlineApp,
    resp: &egui::Response,
    ruler: Rect,
    strip: Rect,
    total: f64,
) {
    let fps = app.current_fps();
    let min_span = app.timeline.min_span(fps);
    let geo = Geometry::new(ruler, strip, total, &app.timeline);
    if resp.contains_pointer() {
        let (zoom, scroll) = ui.input(|i| (i.zoom_delta() as f64, i.smooth_scroll_delta));
        if zoom != 1.0 {
            let anchor = ui
                .ctx()
                .pointer_hover_pos()
                .map(|p| geo.t(p.x))
                .unwrap_or(geo.start + geo.span / 2.0);
            app.timeline.zoom(total, zoom, anchor, min_span);
        } else if app.timeline.is_zoomed() {
            let px = (scroll.x + scroll.y) as f64;
            if px != 0.0 {
                app.timeline
                    .pan(total, -px / strip.width() as f64 * geo.span);
            }
        }
    }
    // Keep the playhead in view when it moves on its own (playing, keys).
    let now = app
        .playhead
        .map(|(c, t)| app.project.clip_offset(c).secs() + t.secs());
    if app.timeline.drag.is_none() && now != app.timeline.last_playhead {
        if let Some(t) = now {
            app.timeline.reveal(total, t, app.playing());
        }
    }
    app.timeline.last_playhead = now;
    // Dragging past an edge scrolls, faster the further out.
    if app.timeline.drag.is_some() && app.timeline.is_zoomed() {
        if let Some(p) = ui.ctx().pointer_latest_pos() {
            let edge = 32.0;
            let out = if p.x < strip.left() + edge {
                (p.x - strip.left() - edge) / edge
            } else if p.x > strip.right() - edge {
                (p.x - strip.right() + edge) / edge
            } else {
                0.0
            };
            if out != 0.0 {
                let dt = ui.input(|i| i.stable_dt).min(0.1) as f64;
                let (_, span) = app.timeline.window(total);
                app.timeline
                    .pan(total, out.clamp(-3.0, 3.0) as f64 * span * 0.8 * dt);
                ui.ctx().request_repaint();
            }
        }
    }
}

/// The thin bar under the labels: where the view sits in the whole
/// timeline. Drag it to scroll, click the track to jump there.
fn scroll_bar(ui: &mut egui::Ui, app: &mut CutlineApp, geo: &Geometry) {
    ui.add_space(BAR_GAP - 3.0);
    let (row, resp) = ui.allocate_exact_size(
        egui::vec2(geo.strip.width(), BAR_H + 6.0),
        if app.timeline.is_zoomed() {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    if !app.timeline.is_zoomed() {
        app.timeline.bar_grab = None;
        return;
    }
    resp.widget_info(|| egui::WidgetInfo::slider(true, geo.start, "Timeline scroll"));
    let track = Rect::from_center_size(row.center(), egui::vec2(row.width(), BAR_H));
    let thumb_of = |start: f64, span: f64| {
        let a = track.left() + (start / geo.total) as f32 * track.width();
        let w = ((span / geo.total) as f32 * track.width()).max(24.0);
        Rect::from_x_y_ranges(a..=(a + w).min(track.right()), track.y_range())
    };
    let at = |x: f32| ((x - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * geo.total;
    if resp.drag_started() || resp.clicked() {
        // Where the press began: a drag only registers once the pointer has
        // moved, and grabbing there would make the view lag the pointer.
        let origin = ui
            .input(|i| i.pointer.press_origin())
            .or(resp.interact_pointer_pos());
        if let Some(p) = origin {
            let thumb = thumb_of(geo.start, geo.span);
            app.timeline.bar_grab = Some(if thumb.expand2(egui::vec2(0.0, 4.0)).contains(p) {
                at(p.x) - geo.start
            } else {
                geo.span / 2.0
            });
        }
    }
    if let (Some(grab), Some(p)) = (app.timeline.bar_grab, resp.interact_pointer_pos()) {
        if resp.dragged() || resp.drag_started() || resp.clicked() {
            app.timeline.start = 0.0;
            app.timeline.pan(geo.total, at(p.x) - grab);
        }
    }
    if resp.drag_stopped() || resp.clicked() {
        app.timeline.bar_grab = None;
    }
    let (start, span) = app.timeline.window(geo.total);
    let p = ui.painter();
    p.rect_filled(track, 2, theme::RAISED);
    let active = resp.hovered() || resp.dragged();
    p.rect_filled(
        thumb_of(start, span),
        2,
        if active { MUTED } else { theme::BORDER_STRONG },
    );
}

/// Pick a tick step so labels sit at least `min_px` apart.
fn tick_step(span: f64, width: f32, min_px: f32) -> f64 {
    const STEPS: [f64; 18] = [
        0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0,
        1800.0, 3600.0, 7200.0,
    ];
    let px_per_s = width as f64 / span.max(1e-6);
    STEPS
        .iter()
        .copied()
        .find(|s| s * px_per_s >= min_px as f64)
        .unwrap_or(7200.0)
}

/// `0:06`, or `0:06.2` / `0:06.25` when the labels are closer than a
/// second (whole seconds stay `0:06`).
fn ruler_label(t: f64, step: f64) -> String {
    if step >= 1.0 || (t - t.round()).abs() < 1e-6 {
        format::clock(t)
    } else if step >= 0.1 {
        format::clock_precise(t)
    } else {
        let hundredths = ((t.max(0.0) * 100.0).round() as u64) % 100;
        format!("{}.{hundredths:02}", format::clock(t))
    }
}

/// Time labels for the stretch in view, each centred on its time but kept
/// inside the ruler. Zoomed in far enough, every frame gets a small tick.
fn draw_ruler(ui: &egui::Ui, app: &CutlineApp, geo: &Geometry) {
    let p = ui.painter();
    let r = geo.ruler;
    // Frame ticks, per video (each has its own rate).
    for c in &app.project.clips {
        let fps = if c.fps > 0.0 { c.fps } else { continue };
        let px_per_frame = r.width() as f64 / geo.span / fps;
        if px_per_frame < FRAME_TICK_PX {
            continue;
        }
        let off = app.project.clip_offset(c.id).secs();
        let first = ((geo.start - off) * fps).ceil().max(0.0) as u64;
        let last = ((geo.end() - off).min(c.duration.secs()) * fps)
            .floor()
            .max(0.0) as u64;
        for k in first..=last {
            let x = geo.x(off + k as f64 / fps);
            // Above the strip's top edge, which its rounded corners cover.
            p.vline(
                x,
                r.bottom() - 7.0..=r.bottom() - 3.0,
                Stroke::new(1.0, theme::BORDER_STRONG),
            );
        }
    }
    let step = tick_step(geo.span, r.width(), 110.0);
    let first = (geo.start / step - 1e-9).ceil().max(0.0) as u64;
    let last = (geo.end() / step + 1e-9).floor() as u64;
    let mut last_right = f32::MIN;
    for i in first..=last {
        let t = i as f64 * step;
        let g = p.layout_no_wrap(ruler_label(t, step), theme::MONO(11.0), MUTED);
        let w = g.size().x;
        let left = (geo.x(t) - w / 2.0).clamp(r.left(), r.right() - w);
        if left < last_right + 8.0 {
            continue;
        }
        last_right = left + w;
        p.galley(egui::pos2(left, r.top()), g, MUTED);
    }
}

/// Tile counts a strip can be cut into: about √2 apart, so a tile is never
/// far from its natural width, yet zooming doesn't ask for a new set of
/// thumbnails on every frame.
fn tile_count(needed: f32) -> usize {
    let mut c = 4.0f64;
    while c < needed as f64 && c < 4096.0 {
        c *= std::f64::consts::SQRT_2;
    }
    c.round() as usize
}

fn draw(ui: &mut egui::Ui, app: &mut CutlineApp, geo: &Geometry) {
    let full = ui.painter().clone();
    // The strip's contents stop at its edges; only the playhead's head may
    // reach past them.
    let painter = full.with_clip_rect(Rect::from_x_y_ranges(
        geo.strip.x_range(),
        full.clip_rect().y_range(),
    ));
    let settling = ui.input(|i| i.zoom_delta()) == 1.0;
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
        let count = tile_count(rect.width() / tile_w);
        let w = rect.width() / count as f32;
        // Only the tiles in view.
        let first = (((strip.left() - rect.left()) / w).floor().max(0.0) as usize).min(count);
        let last = (((strip.right() - rect.left()) / w).ceil().max(0.0) as usize).min(count);
        if settling {
            app.want_thumbs(*id, count, (STRIP_H * ppp) as usize, first..last);
        }
        let clip_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        clip_painter.rect_filled(rect, 8, theme::RAISED);
        for j in first..last {
            let r = Rect::from_x_y_ranges(
                rect.left() + j as f32 * w..=rect.left() + (j + 1) as f32 * w,
                strip.y_range(),
            );
            if let Some(tex) = app.thumb_for(*id, count, j) {
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
            // Pinned to the visible start of its video when that's scrolled off.
            let left = rect.left().max(strip.left()).min(rect.right() - w - 16.0);
            let bg = Rect::from_min_size(
                egui::pos2(left + 6.0, rect.top() + 6.0),
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
                let r = Rect::from_x_y_ranges(geo.x(off + a)..=geo.x(off + b), strip.y_range())
                    .intersect(painter.clip_rect());
                if r.width() <= 0.0 {
                    return;
                }
                painter.rect_filled(r, 0, Color32::from_black_alpha(168));
                let hp = painter.with_clip_rect(r);
                // 45° bands, 6 px wide and 12 px apart, moving with the video.
                let period = 17.0;
                let phase = geo.x(0.0).rem_euclid(period);
                let mut x = ((r.left() - phase) / period).floor() * period + phase - r.height();
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
        full.vline(
            x,
            top..=strip.bottom() + 6.0,
            Stroke::new(2.0, theme::PLAYHEAD),
        );
        let head = Rect::from_min_size(egui::pos2(x - 6.0, top - 2.0), egui::vec2(12.0, 10.0));
        full.rect_filled(
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
            // Right-clicking a part selects it, so the menu's Delete part
            // (and the Del key) act on the part under the pointer.
            if let Some((c, t)) = app.timeline.menu_at {
                if let Some(r) = part_at(app, c, t) {
                    app.project.selected = Some(r);
                }
            }
        }
    }
    let mut action = TimelineAction::None;
    resp.context_menu(|ui| {
        let Some((c, t)) = app.timeline.menu_at else {
            return;
        };
        theme::menu(ui);
        let mut pick = |a: TimelineAction, ui: &mut egui::Ui| {
            action = a;
            ui.close();
        };
        match part_at(app, c, t) {
            Some(r) => {
                let mut trial = app.project.clone();
                ui.add_enabled_ui(trial.split_part(c, t).is_some(), |ui| {
                    if theme::menu_item(ui, Icon::Scissors, "Split here", None).clicked() {
                        pick(TimelineAction::SplitAt(c, t), ui);
                    }
                });
                if theme::menu_danger(ui, Icon::Trash, "Delete part", Some("Del")).clicked() {
                    pick(TimelineAction::RemovePart(r), ui);
                }
            }
            None => {
                if theme::menu_item(ui, Icon::Plus, "Add part here", Some("Double-click")).clicked()
                {
                    pick(TimelineAction::AddPart(c, t), ui);
                }
            }
        }
        theme::menu_separator(ui);
        theme::menu_caption(ui, "Marked parts");
        let mode = app.project.mode;
        if theme::menu_choice(ui, mode == PartMode::Keep, "Keep them").clicked() {
            pick(TimelineAction::SetMode(PartMode::Keep), ui);
        }
        if theme::menu_choice(ui, mode == PartMode::Remove, "Cut them out").clicked() {
            pick(TimelineAction::SetMode(PartMode::Remove), ui);
        }
        theme::menu_separator(ui);
        if app.project.clips.len() > 1 {
            let idx = app.project.clip_index(c).unwrap_or(0);
            ui.add_enabled_ui(idx > 0, |ui| {
                if theme::menu_item(ui, Icon::ArrowLeft, "Move video earlier", None).clicked() {
                    pick(TimelineAction::MoveClip(c, -1), ui);
                }
            });
            ui.add_enabled_ui(idx + 1 < app.project.clips.len(), |ui| {
                if theme::menu_item(ui, Icon::ArrowRight, "Move video later", None).clicked() {
                    pick(TimelineAction::MoveClip(c, 1), ui);
                }
            });
        }
        if theme::menu_danger(ui, Icon::Close, "Remove video from timeline", None).clicked() {
            pick(TimelineAction::RemoveClip(c), ui);
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
            let (xa, xb) = (geo.x(off + p.start.secs()), geo.x(off + p.end.secs()));
            if xb < row.left() || xa > row.right() {
                continue;
            }
            let mid = (xa.max(row.left()) + xb.min(row.right())) / 2.0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_labels() {
        assert_eq!(ruler_label(6.0, 2.0), "0:06");
        assert_eq!(ruler_label(4.0, 0.5), "0:04");
        assert_eq!(ruler_label(4.5, 0.5), "0:04.5");
        assert_eq!(ruler_label(4.55, 0.05), "0:04.55");
        assert_eq!(ruler_label(65.0, 0.05), "1:05");
    }

    #[test]
    fn tile_counts_stay_near_their_natural_width() {
        for needed in [1.0, 4.5, 9.2, 100.0, 1000.0] {
            let c = tile_count(needed) as f32;
            assert!(
                c >= needed && c <= (needed * 1.5).max(4.0),
                "{needed} -> {c}"
            );
        }
    }
}
