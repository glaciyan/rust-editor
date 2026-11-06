//! Playback and thumbnails for the timeline.
//!
//! [`Player`] uses libmpv when it can be loaded (smooth playback with sound,
//! exact seeks) and falls back to decoding frames with ffmpeg (no sound).

mod fallback;
mod mpv;
pub mod thumbs;

use std::path::Path;
use std::sync::Arc;

/// An RGBA image, row-major, no padding.
#[derive(Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Called from any thread when there's something new to draw.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub enum Player {
    Mpv(mpv::MpvPlayer),
    Ffmpeg(fallback::FfmpegPlayer),
}

impl Player {
    pub fn new(ffmpeg: &Path, waker: Waker) -> Player {
        match mpv::MpvPlayer::new(waker.clone()) {
            Ok(p) => Player::Mpv(p),
            Err(e) => {
                eprintln!("cutline: libmpv unavailable ({e}); preview without sound");
                Player::Ffmpeg(fallback::FfmpegPlayer::new(ffmpeg.to_path_buf(), waker))
            }
        }
    }

    pub fn has_sound(&self) -> bool {
        matches!(self, Player::Mpv(_))
    }

    /// Load a file and start at `start` seconds.
    pub fn open(&mut self, path: &Path, start: f64, play: bool) {
        match self {
            Player::Mpv(p) => p.open(path, start, play),
            Player::Ffmpeg(p) => p.open(path, start, play),
        }
    }
    pub fn set_paused(&mut self, paused: bool) {
        match self {
            Player::Mpv(p) => p.set_paused(paused),
            Player::Ffmpeg(p) => p.set_paused(paused),
        }
    }
    pub fn paused(&self) -> bool {
        match self {
            Player::Mpv(p) => p.paused(),
            Player::Ffmpeg(p) => p.paused(),
        }
    }
    /// Frame-exact seek (used while dragging handles).
    pub fn seek(&mut self, t: f64) {
        match self {
            Player::Mpv(p) => p.seek(t),
            Player::Ffmpeg(p) => p.seek(t),
        }
    }
    pub fn position(&self) -> f64 {
        match self {
            Player::Mpv(p) => p.position(),
            Player::Ffmpeg(p) => p.position(),
        }
    }
    /// The file played to its end.
    pub fn at_end(&self) -> bool {
        match self {
            Player::Mpv(p) => p.at_end(),
            Player::Ffmpeg(p) => p.at_end(),
        }
    }
    pub fn set_volume(&mut self, v: f64) {
        if let Player::Mpv(p) = self {
            p.set_volume(v)
        }
    }
    /// Pump events; returns a new frame sized to fit `max_w`×`max_h` if one is ready.
    pub fn update(&mut self, max_w: usize, max_h: usize) -> Option<Frame> {
        match self {
            Player::Mpv(p) => p.update(max_w, max_h),
            Player::Ffmpeg(p) => p.update(max_w, max_h),
        }
    }
    pub fn close(&mut self) {
        match self {
            Player::Mpv(p) => p.close(),
            Player::Ffmpeg(p) => p.close(),
        }
    }
}

/// Fit `w`×`h` inside `max_w`×`max_h`, keeping aspect, even dimensions.
pub fn fit_size(w: usize, h: usize, max_w: usize, max_h: usize) -> (usize, usize) {
    if w == 0 || h == 0 {
        return (max_w.max(2), max_h.max(2));
    }
    let s = (max_w as f64 / w as f64)
        .min(max_h as f64 / h as f64)
        .min(1.0);
    let fw = ((w as f64 * s) as usize / 2 * 2).max(2);
    let fh = ((h as f64 * s) as usize / 2 * 2).max(2);
    (fw, fh)
}
