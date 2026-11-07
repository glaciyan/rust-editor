//! Small line icons drawn with the painter, so they're crisp at any scale
//! and don't depend on which fonts the system has. Each fits a 16×16 box.

use eframe::egui::{self, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Open,
    Export,
    Undo,
    Redo,
    Play,
    Pause,
    StepBack,
    StepForward,
    ToStart,
    ToEnd,
    /// Speaker with 0–2 waves.
    Volume(u8),
    Muted,
    Plus,
    Close,
    Check,
    Film,
    Folder,
    Info,
    Sliders,
}

impl Icon {
    pub fn paint(self, p: &Painter, rect: Rect, color: Color32) {
        let s = rect.width().min(rect.height()) / 16.0;
        let o = rect.center() - vec2(8.0, 8.0) * s;
        let at = |x: f32, y: f32| -> Pos2 { o + vec2(x, y) * s };
        let line = Stroke::new(1.6 * s, color);
        let poly = |pts: Vec<Pos2>| Shape::convex_polygon(pts, color, Stroke::NONE);
        let path = |pts: Vec<Pos2>| Shape::line(pts, line);
        match self {
            Icon::Open | Icon::Folder => {
                p.add(path(vec![
                    at(1.5, 4.0),
                    at(1.5, 13.0),
                    at(14.5, 13.0),
                    at(14.5, 5.5),
                    at(8.0, 5.5),
                    at(6.5, 3.0),
                    at(1.5, 3.0),
                    at(1.5, 4.0),
                ]));
                if self == Icon::Open {
                    p.add(path(vec![at(1.5, 8.0), at(14.5, 8.0)]));
                }
            }
            Icon::Export => {
                p.add(path(vec![at(8.0, 10.5), at(8.0, 1.5)]));
                p.add(path(vec![at(4.5, 5.0), at(8.0, 1.5), at(11.5, 5.0)]));
                p.add(path(vec![
                    at(2.5, 9.0),
                    at(2.5, 14.0),
                    at(13.5, 14.0),
                    at(13.5, 9.0),
                ]));
            }
            Icon::Undo | Icon::Redo => {
                let flip = self == Icon::Redo;
                let fx = |x: f32| if flip { 16.0 - x } else { x };
                // A hook: left along the top, round a half circle, back along the bottom.
                let mut pts = vec![at(fx(3.0), 5.0)];
                for i in 0..=12 {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 12.0 * std::f32::consts::PI;
                    pts.push(at(fx(9.5 + a.cos() * 4.0), 9.0 + a.sin() * 4.0));
                }
                pts.push(at(fx(5.0), 13.0));
                p.add(path(pts));
                p.add(path(vec![
                    at(fx(5.8), 2.2),
                    at(fx(3.0), 5.0),
                    at(fx(5.8), 7.8),
                ]));
            }
            Icon::Play => {
                p.add(poly(vec![at(4.5, 2.5), at(13.5, 8.0), at(4.5, 13.5)]));
            }
            Icon::Pause => {
                for x in [4.0, 9.5] {
                    p.rect_filled(
                        Rect::from_min_size(at(x, 2.5), vec2(2.8, 11.0) * s),
                        1.0 * s,
                        color,
                    );
                }
            }
            Icon::StepBack => {
                p.rect_filled(
                    Rect::from_min_size(at(3.0, 3.5), vec2(2.0, 9.0) * s),
                    0.8 * s,
                    color,
                );
                p.add(poly(vec![at(13.0, 3.5), at(13.0, 12.5), at(6.5, 8.0)]));
            }
            Icon::StepForward => {
                p.rect_filled(
                    Rect::from_min_size(at(11.0, 3.5), vec2(2.0, 9.0) * s),
                    0.8 * s,
                    color,
                );
                p.add(poly(vec![at(3.0, 3.5), at(9.5, 8.0), at(3.0, 12.5)]));
            }
            Icon::ToStart => {
                p.rect_filled(
                    Rect::from_min_size(at(2.0, 3.5), vec2(2.0, 9.0) * s),
                    0.8 * s,
                    color,
                );
                p.add(poly(vec![at(9.0, 3.5), at(9.0, 12.5), at(4.5, 8.0)]));
                p.add(poly(vec![at(14.0, 3.5), at(14.0, 12.5), at(9.5, 8.0)]));
            }
            Icon::ToEnd => {
                p.rect_filled(
                    Rect::from_min_size(at(12.0, 3.5), vec2(2.0, 9.0) * s),
                    0.8 * s,
                    color,
                );
                p.add(poly(vec![at(2.0, 3.5), at(6.5, 8.0), at(2.0, 12.5)]));
                p.add(poly(vec![at(7.0, 3.5), at(11.5, 8.0), at(7.0, 12.5)]));
            }
            Icon::Volume(_) | Icon::Muted => {
                p.add(poly(vec![
                    at(1.5, 5.5),
                    at(4.5, 5.5),
                    at(8.5, 2.0),
                    at(8.5, 14.0),
                    at(4.5, 10.5),
                    at(1.5, 10.5),
                ]));
                match self {
                    Icon::Muted => {
                        p.add(path(vec![at(10.5, 5.5), at(15.0, 10.5)]));
                        p.add(path(vec![at(15.0, 5.5), at(10.5, 10.5)]));
                    }
                    Icon::Volume(level) => {
                        let wave = |r: f32| {
                            let pts: Vec<Pos2> = (0..=8)
                                .map(|i| {
                                    let a = (-50.0 + i as f32 * 12.5_f32).to_radians();
                                    at(8.5 + a.cos() * r, 8.0 + a.sin() * r)
                                })
                                .collect();
                            Shape::line(pts, line)
                        };
                        if level >= 1 {
                            p.add(wave(3.2));
                        }
                        if level >= 2 {
                            p.add(wave(6.0));
                        }
                    }
                    _ => {}
                }
            }
            Icon::Plus => {
                p.add(path(vec![at(8.0, 2.5), at(8.0, 13.5)]));
                p.add(path(vec![at(2.5, 8.0), at(13.5, 8.0)]));
            }
            Icon::Close => {
                p.add(path(vec![at(3.5, 3.5), at(12.5, 12.5)]));
                p.add(path(vec![at(12.5, 3.5), at(3.5, 12.5)]));
            }
            Icon::Check => {
                p.add(path(vec![at(2.5, 8.5), at(6.5, 12.5), at(13.5, 3.5)]));
            }
            Icon::Film => {
                let r = Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5));
                p.rect_stroke(r, 2.0 * s, line, egui::StrokeKind::Middle);
                p.add(path(vec![at(4.5, 2.5), at(4.5, 13.5)]));
                p.add(path(vec![at(11.5, 2.5), at(11.5, 13.5)]));
                for y in [5.5, 8.0, 10.5] {
                    p.add(path(vec![at(1.5, y), at(4.5, y)]));
                    p.add(path(vec![at(11.5, y), at(14.5, y)]));
                }
            }
            Icon::Info => {
                p.circle_stroke(at(8.0, 8.0), 6.5 * s, line);
                p.add(path(vec![at(8.0, 7.0), at(8.0, 11.5)]));
                p.circle_filled(at(8.0, 4.8), 1.0 * s, color);
            }
            Icon::Sliders => {
                for (y, x) in [(4.0, 10.0), (8.0, 5.0), (12.0, 11.5)] {
                    p.add(path(vec![at(1.5, y), at(14.5, y)]));
                    p.circle_filled(at(x, y), 2.0 * s, color);
                }
            }
        }
    }
}

/// The app mark: two amber trim brackets around a white play triangle.
pub fn logo(p: &Painter, rect: Rect) {
    p.rect_filled(rect, rect.width() * 0.26, crate::theme::RAISED);
    p.rect_stroke(
        rect,
        rect.width() * 0.26,
        Stroke::new(1.0, crate::theme::BORDER),
        egui::StrokeKind::Inside,
    );
    let s = rect.width() / 24.0;
    let o = rect.min;
    let at = |x: f32, y: f32| o + vec2(x, y) * s;
    let trim = Stroke::new(2.2 * s, crate::theme::TRIM);
    p.add(Shape::line(
        vec![at(8.0, 5.5), at(5.5, 5.5), at(5.5, 18.5), at(8.0, 18.5)],
        trim,
    ));
    p.add(Shape::line(
        vec![at(16.0, 5.5), at(18.5, 5.5), at(18.5, 18.5), at(16.0, 18.5)],
        trim,
    ));
    p.add(Shape::convex_polygon(
        vec![at(10.0, 8.5), at(15.0, 12.0), at(10.0, 15.5)],
        Color32::WHITE,
        Stroke::NONE,
    ));
}
