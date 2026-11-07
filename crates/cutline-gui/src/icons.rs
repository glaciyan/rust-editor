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

/// [`logo`] as a window icon, rasterised in software (4×4 samples a pixel)
/// since there's no GPU before the window exists. Shown by X11 window
/// managers; Wayland compositors take the icon from the desktop file.
pub fn window_icon(size: u32) -> egui::IconData {
    use crate::theme::{BORDER, RAISED, TRIM};
    // In the logo's 24-unit space.
    let radius = 24.0 * 0.26;
    let in_rounded = |x: f32, y: f32, inset: f32| {
        let (lo, hi, r) = (inset, 24.0 - inset, radius - inset);
        let cx = x.clamp(lo + r, hi - r);
        let cy = y.clamp(lo + r, hi - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
            && (lo..=hi).contains(&x)
            && (lo..=hi).contains(&y)
    };
    let near_segment = |x: f32, y: f32, a: (f32, f32), b: (f32, f32), w: f32| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (px, py) = (a.0 + t * dx - x, a.1 + t * dy - y);
        px * px + py * py <= (w / 2.0).powi(2)
    };
    let brackets = [
        [(8.0, 5.5), (5.5, 5.5), (5.5, 18.5), (8.0, 18.5)],
        [(16.0, 5.5), (18.5, 5.5), (18.5, 18.5), (16.0, 18.5)],
    ];
    let in_trim = |x: f32, y: f32| {
        brackets
            .iter()
            .any(|b| b.windows(2).any(|s| near_segment(x, y, s[0], s[1], 2.2)))
    };
    let in_play = |x: f32, y: f32| {
        let tri = [(10.0, 8.5), (15.0, 12.0), (10.0, 15.5)];
        let side = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
        let s = [
            side(tri[0], tri[1]),
            side(tri[1], tri[2]),
            side(tri[2], tri[0]),
        ];
        s.iter().all(|v| *v >= 0.0) || s.iter().all(|v| *v <= 0.0)
    };

    const SS: u32 = 4;
    let border_w = 24.0 / size as f32 * 1.0f32.max(size as f32 / 32.0);
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut acc, mut cover) = ([0.0f32; 3], 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) * 24.0 / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) * 24.0 / size as f32;
                    if !in_rounded(x, y, 0.0) {
                        continue;
                    }
                    let c = if in_play(x, y) {
                        Color32::WHITE
                    } else if in_trim(x, y) {
                        TRIM
                    } else if !in_rounded(x, y, border_w) {
                        BORDER
                    } else {
                        RAISED
                    };
                    acc[0] += c.r() as f32;
                    acc[1] += c.g() as f32;
                    acc[2] += c.b() as f32;
                    cover += 1.0;
                }
            }
            let n = (SS * SS) as f32;
            if cover == 0.0 {
                rgba.extend([0, 0, 0, 0]);
            } else {
                // Unmultiplied: average colour of the covered samples.
                rgba.extend(acc.map(|v| (v / cover).round() as u8));
                rgba.push((cover / n * 255.0).round() as u8);
            }
        }
    }
    egui::IconData {
        rgba,
        width: size,
        height: size,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_icon_is_the_logo() {
        let icon = super::window_icon(64);
        assert_eq!(
            (icon.width, icon.height, icon.rgba.len()),
            (64, 64, 64 * 64 * 4)
        );
        let px = |x: usize, y: usize| &icon.rgba[(y * 64 + x) * 4..][..4];
        // Rounded corners are transparent, the middle is the white play mark,
        // and the brackets are amber.
        assert_eq!(px(0, 0)[3], 0);
        assert_eq!(px(30, 32), [255, 255, 255, 255]);
        assert_eq!(px(15, 32), [0xf5, 0xb8, 0x3d, 255]);
    }
}
