use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Sub};

/// A point in a media file, in microseconds.
///
/// Integer microseconds avoid float drift when parts are moved around and
/// saved/restored; conversion to seconds happens at the edges (ffmpeg argv,
/// UI labels).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub const ZERO: Timestamp = Timestamp(0);

    pub fn from_secs(s: f64) -> Self {
        Timestamp((s * 1_000_000.0).round() as i64)
    }
    pub fn from_millis(ms: i64) -> Self {
        Timestamp(ms * 1000)
    }
    pub fn secs(self) -> f64 {
        self.0 as f64 / 1_000_000.0
    }
    pub fn micros(self) -> i64 {
        self.0
    }
    pub fn clamp(self, lo: Timestamp, hi: Timestamp) -> Timestamp {
        Timestamp(self.0.clamp(lo.0, hi.0.max(lo.0)))
    }
    /// Snap to the nearest frame boundary for the given frame rate.
    pub fn snap_to_frame(self, fps: f64) -> Timestamp {
        if fps <= 0.0 || !fps.is_finite() {
            return self;
        }
        let frame = (self.secs() * fps).round();
        Timestamp::from_secs(frame / fps)
    }
    /// Seconds formatted for an ffmpeg argument (`12.345678`).
    pub fn ffmpeg_arg(self) -> String {
        let neg = self.0 < 0;
        let a = self.0.unsigned_abs();
        format!(
            "{}{}.{:06}",
            if neg { "-" } else { "" },
            a / 1_000_000,
            a % 1_000_000
        )
    }
}

impl Add for Timestamp {
    type Output = Timestamp;
    fn add(self, o: Timestamp) -> Timestamp {
        Timestamp(self.0 + o.0)
    }
}
impl Sub for Timestamp {
    type Output = Timestamp;
    fn sub(self, o: Timestamp) -> Timestamp {
        Timestamp(self.0 - o.0)
    }
}
impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}s", self.ffmpeg_arg())
    }
}
impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::format::clock(self.secs()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arg_format() {
        assert_eq!(Timestamp::from_secs(12.5).ffmpeg_arg(), "12.500000");
        assert_eq!(Timestamp(-1500).ffmpeg_arg(), "-0.001500");
    }
    #[test]
    fn snap() {
        let t = Timestamp::from_secs(1.01).snap_to_frame(30.0);
        assert_eq!(t, Timestamp::from_secs(1.0));
    }
}
