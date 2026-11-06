//! Fit to size: given a size in megabytes, find the encode settings with the
//! best picture that still fits.
//!
//! The solver is pure arithmetic so it can run on every keystroke in the
//! export dialog. The executor encodes two-pass at the solved bitrate, then
//! checks the real file and re-runs the second pass lower if it came out too
//! big (see `job.rs`), so the promise "fits in N MB" holds even when the rate
//! control misses.
//!
//! How the budget is split:
//!
//! 1. Reserve a safety margin ([`MARGIN`]) for rate-control error, plus the
//!    container's own bytes ([`container_overhead`]): headers and the per
//!    packet index tables, which grow with frame count, not bitrate.
//! 2. Give audio a share that shrinks as the budget does ([`audio_kbps`]).
//! 3. Spend the rest on video, choosing the largest picture size (and, for
//!    high frame rates, full or half rate) at which that bitrate still looks
//!    good. "Looks good" is judged against a reference bitrate that scales
//!    sub-linearly with pixels (`pixels^0.75`) and frame rate (`fps^0.6`),
//!    because bigger frames and higher rates compress better per pixel.

use cutline_core::{Quality, VideoCodec};

/// Share of the target held back for rate-control error.
pub const MARGIN: f64 = 0.03;
/// Below this ratio to the "good" bitrate even the smallest size is mush.
const MIN_RATIO: f64 = 0.3;
/// HEVC bitrate (bit/s) that looks good at 1080p30 for typical footage.
const GOOD_1080P30_HEVC: f64 = 1_400_000.0;
const H264_FACTOR: f64 = 1.6;
const AUDIO_LADDER: [u32; 6] = [160, 128, 96, 64, 48, 32];
const HEIGHTS: [u32; 8] = [2160, 1440, 1080, 720, 540, 480, 360, 240];
/// AAC frames per second at 48 kHz (1024 samples each).
const AAC_PACKETS_PER_SEC: f64 = 48_000.0 / 1024.0;

#[derive(Clone, Debug, PartialEq)]
pub struct FitInput {
    pub target_bytes: f64,
    /// Output length in seconds.
    pub duration: f64,
    /// Display size of the source (after rotation). 0×0 for audio only.
    pub src_width: u32,
    pub src_height: u32,
    pub src_fps: f64,
    /// Source video bitrate (bit/s), when known.
    pub src_video_bps: Option<f64>,
    /// Channels of each audio track that will be exported.
    pub audio_channels: Vec<u32>,
    /// Source audio bitrate per track (kbit/s), when known.
    pub src_audio_kbps: Vec<Option<u32>>,
    pub codec: VideoCodec,
    /// What Original quality would produce, and whether it can (all sources match).
    pub original_bytes: f64,
    pub original_possible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FitSettings {
    pub video_kbps: u32,
    /// Per audio track.
    pub audio_kbps: u32,
    /// Output size; `None` keeps the source size.
    pub height: Option<u32>,
    pub width: u32,
    /// Output frame rate when reduced.
    pub fps: Option<f64>,
    pub quality: Quality,
    /// What we aim for (target minus margin).
    pub expected_bytes: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FitOutcome {
    /// The target is at least what Original quality makes; no need to re-encode.
    Original {
        bytes: f64,
    },
    Encode(FitSettings),
    /// Not even the smallest picture fits. `min_bytes` is the smallest target that would.
    TooSmall {
        min_bytes: f64,
    },
}

/// Bytes the MP4 container adds on top of the streams.
pub fn container_overhead(duration: f64, fps: f64, audio_tracks: usize) -> f64 {
    let frames = duration * fps.max(1.0);
    let audio_packets = duration * AAC_PACKETS_PER_SEC * audio_tracks as f64;
    // moov/ftyp/headers + ~16 bytes of sample tables per video frame
    // (size, time, composition offset, chunk offset) + ~12 per audio packet.
    48_000.0 + frames * 16.0 + audio_packets * 12.0
}

/// Bitrate (bit/s) that looks good at this size and rate.
pub fn good_bitrate(codec: VideoCodec, width: u32, height: u32, fps: f64) -> f64 {
    let px = (width.max(2) as f64) * (height.max(2) as f64);
    let base = GOOD_1080P30_HEVC * (px / 2_073_600.0).powf(0.75) * (fps.max(1.0) / 30.0).powf(0.6);
    if codec == VideoCodec::H264 {
        base * H264_FACTOR
    } else {
        base
    }
}

fn even(x: f64) -> u32 {
    ((x / 2.0).round() as u32).max(1) * 2
}

/// Per-track audio bitrate for a total budget (kbit/s).
pub fn audio_kbps(total_kbps: f64, channels: &[u32], src_kbps: &[Option<u32>]) -> u32 {
    if channels.is_empty() {
        return 0;
    }
    let n = channels.len() as f64;
    // Audio gets up to 15% of a healthy budget, up to 25% of a tight one:
    // at low sizes speech intelligibility matters more than extra pixels.
    let share = if total_kbps < 600.0 { 0.25 } else { 0.15 };
    let allowed = total_kbps * share / n;
    let mono = channels.iter().all(|c| *c <= 1);
    let mut a = AUDIO_LADDER
        .iter()
        .copied()
        .find(|a| (*a as f64) <= allowed)
        .unwrap_or(32);
    if mono {
        a = a.min(96);
    }
    // Never spend more than the source had (it can't sound better).
    if let Some(src_max) = src_kbps.iter().flatten().max() {
        let cap = AUDIO_LADDER
            .iter()
            .copied()
            .find(|l| *l <= (*src_max).max(32))
            .unwrap_or(32);
        a = a.min(cap);
    }
    a
}

fn label(ratio: f64) -> Quality {
    if ratio >= 2.2 {
        Quality::Great
    } else if ratio >= 1.0 {
        Quality::Good
    } else if ratio >= 0.55 {
        Quality::Okay
    } else {
        Quality::Low
    }
}

/// A small picture can't be "Great" however many bits it gets.
fn size_cap(height: u32, src_height: u32) -> Quality {
    if height >= src_height || height >= 720 {
        Quality::Great
    } else if height >= 480 {
        Quality::Good
    } else if height > 240 {
        Quality::Okay
    } else {
        Quality::Low
    }
}

pub fn solve(input: &FitInput) -> FitOutcome {
    let dur = input.duration.max(0.1);
    let has_video = input.src_height > 0 && input.src_width > 0;
    let fps = if input.src_fps > 0.0 {
        input.src_fps
    } else {
        30.0
    };
    let n_audio = input.audio_channels.len();

    if input.original_possible
        && input.original_bytes > 0.0
        && input.target_bytes >= input.original_bytes
    {
        return FitOutcome::Original {
            bytes: input.original_bytes,
        };
    }

    let min_bytes = minimum_bytes(input);
    let overhead = container_overhead(dur, if has_video { fps } else { 0.0 }, n_audio);
    let budget = input.target_bytes * (1.0 - MARGIN) - overhead;
    if budget <= 0.0 {
        return FitOutcome::TooSmall { min_bytes };
    }
    let total_kbps = budget * 8.0 / dur / 1000.0;
    let a = audio_kbps(total_kbps, &input.audio_channels, &input.src_audio_kbps);
    let mut video_kbps = total_kbps - (a as f64) * n_audio as f64;

    if !has_video {
        // Audio only: everything goes to sound, within reason.
        let per = (total_kbps / n_audio.max(1) as f64).min(256.0);
        if per < 24.0 {
            return FitOutcome::TooSmall { min_bytes };
        }
        let per = AUDIO_LADDER
            .iter()
            .copied()
            .find(|l| (*l as f64) <= per)
            .unwrap_or(32);
        return FitOutcome::Encode(FitSettings {
            video_kbps: 0,
            audio_kbps: per,
            height: None,
            width: 0,
            fps: None,
            quality: if per >= 128 {
                Quality::Great
            } else if per >= 96 {
                Quality::Good
            } else if per >= 64 {
                Quality::Okay
            } else {
                Quality::Low
            },
            expected_bytes: input.target_bytes * (1.0 - MARGIN),
        });
    }
    if video_kbps < 40.0 {
        return FitOutcome::TooSmall { min_bytes };
    }

    // Re-encoding at more than the source's own bitrate can't add detail.
    let mut capped_by_source = false;
    if let Some(src) = input.src_video_bps.filter(|b| *b > 0.0) {
        if video_kbps * 1000.0 >= src * 1.05 {
            if input.original_possible {
                return FitOutcome::Original {
                    bytes: input.original_bytes,
                };
            }
            video_kbps = src * 1.05 / 1000.0;
            capped_by_source = true;
        }
    }

    let bps = video_kbps * 1000.0;
    let (sw, sh) = (input.src_width, input.src_height);
    let aspect = sw as f64 / sh as f64;
    let mut heights: Vec<u32> = vec![sh];
    heights.extend(HEIGHTS.iter().copied().filter(|h| *h < sh));
    let mut rates = vec![None];
    if fps > 31.0 {
        rates.push(Some(fps / 2.0));
    }

    let mut chosen = None;
    let mut last = None;
    'outer: for &h in &heights {
        for &r in &rates {
            let w = if h == sh { sw } else { even(h as f64 * aspect) };
            let f = r.unwrap_or(fps);
            let ratio = bps / good_bitrate(input.codec, w, h, f);
            last = Some((h, w, r, ratio));
            if ratio >= 1.0 {
                chosen = Some((h, w, r, ratio));
                break 'outer;
            }
        }
    }
    let (h, w, r, ratio) = match chosen {
        Some(c) => c,
        None => match last {
            Some(l) if l.3 >= MIN_RATIO => l,
            _ => return FitOutcome::TooSmall { min_bytes },
        },
    };
    let mut quality = label(ratio).min(size_cap(h, sh));
    if r.is_some() {
        quality = quality.min(Quality::Good);
    }
    if capped_by_source {
        quality = Quality::Great;
    }
    FitOutcome::Encode(FitSettings {
        video_kbps: video_kbps.floor() as u32,
        audio_kbps: a,
        height: (h != sh).then_some(h),
        width: w,
        fps: r,
        quality,
        expected_bytes: ((video_kbps.floor() + (a as f64) * n_audio as f64) * 1000.0 / 8.0) * dur
            + overhead,
    })
}

/// Smallest target (bytes) the solver accepts, rounded up to a whole MB.
pub fn minimum_bytes(input: &FitInput) -> f64 {
    let dur = input.duration.max(0.1);
    let n_audio = input.audio_channels.len() as f64;
    let has_video = input.src_height > 0;
    let (video_bps, fps) = if has_video {
        let sh = input.src_height;
        let h = HEIGHTS.iter().copied().rfind(|h| *h < sh).unwrap_or(sh);
        let w = even(h as f64 * input.src_width as f64 / sh as f64);
        let fps = if input.src_fps > 31.0 {
            input.src_fps / 2.0
        } else {
            input.src_fps.max(1.0)
        };
        (
            (good_bitrate(input.codec, w, h, fps) * MIN_RATIO).max(40_000.0),
            fps,
        )
    } else {
        (0.0, 0.0)
    };
    let audio_bps = 32_000.0 * n_audio;
    let raw = (video_bps + audio_bps) / 8.0 * dur + container_overhead(dur, fps, n_audio as usize);
    // The audio share rule means audio at the minimum is ~25% of total: solve with that in mind.
    let with_share = if n_audio > 0.0 && has_video {
        raw.max((audio_bps / 0.25) / 8.0 * dur)
    } else {
        raw
    };
    ((with_share / (1.0 - MARGIN)) / 1e6).ceil() * 1e6
}

/// Video kbit/s for a retry after a first attempt came out `actual_bytes`.
pub fn corrected_video_kbps(
    previous_kbps: u32,
    actual_bytes: f64,
    target_bytes: f64,
    audio_bytes: f64,
    overhead: f64,
) -> u32 {
    let video_actual = (actual_bytes - audio_bytes - overhead).max(1.0);
    let video_room = (target_bytes * (1.0 - MARGIN) - audio_bytes - overhead).max(1.0);
    let k = (video_room / video_actual) * 0.98;
    ((previous_kbps as f64) * k.min(0.99)).floor().max(20.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(mb: f64, dur: f64, w: u32, h: u32, fps: f64) -> FitInput {
        FitInput {
            target_bytes: mb * 1e6,
            duration: dur,
            src_width: w,
            src_height: h,
            src_fps: fps,
            src_video_bps: Some(20_000_000.0),
            audio_channels: vec![2],
            src_audio_kbps: vec![Some(192)],
            codec: VideoCodec::H265,
            original_bytes: 20_000_000.0 / 8.0 * dur,
            original_possible: true,
        }
    }

    fn total_bytes(s: &FitSettings, dur: f64, fps: f64, tracks: usize) -> f64 {
        (s.video_kbps as f64 + s.audio_kbps as f64 * tracks as f64) * 1000.0 / 8.0 * dur
            + container_overhead(dur, fps, tracks)
    }

    #[test]
    fn budget_never_exceeds_target() {
        for &(mb, dur) in &[
            (8.0, 30.0),
            (10.0, 120.0),
            (25.0, 60.0),
            (50.0, 600.0),
            (8.0, 8.0),
            (100.0, 3600.0),
        ] {
            let i = input(mb, dur, 1920, 1080, 30.0);
            match solve(&i) {
                FitOutcome::Encode(s) => {
                    let fps = s.fps.unwrap_or(30.0);
                    let b = total_bytes(&s, dur, fps, 1);
                    assert!(
                        b <= mb * 1e6 * (1.0 - MARGIN) + 1.0,
                        "{mb} MB / {dur}s planned {b}"
                    );
                    assert!(
                        b >= mb * 1e6 * 0.9,
                        "wastes budget: {mb} MB / {dur}s planned {b}"
                    );
                }
                FitOutcome::TooSmall { min_bytes } => assert!(min_bytes > mb * 1e6, "{mb} {dur}"),
                FitOutcome::Original { .. } => panic!("unexpected original"),
            }
        }
    }

    #[test]
    fn scales_down_when_tight() {
        // 10 MB for two minutes of 1080p: ~600 kbps, should drop below 1080.
        let FitOutcome::Encode(s) = solve(&input(10.0, 120.0, 1920, 1080, 30.0)) else {
            panic!()
        };
        assert!(s.height.unwrap() < 1080);
        assert_eq!(s.width % 2, 0);
        // 50 MB for 30 s: plenty, keep full size, great.
        let FitOutcome::Encode(s) = solve(&input(50.0, 30.0, 1920, 1080, 30.0)) else {
            panic!()
        };
        assert_eq!(s.height, None);
        assert_eq!(s.quality, Quality::Great);
    }

    #[test]
    fn high_frame_rate_halves_before_shrinking() {
        let FitOutcome::Encode(s) = solve(&input(20.0, 60.0, 1920, 1080, 60.0)) else {
            panic!()
        };
        // ~2.5 Mbps: 1080p60 needs ~2.1 Mbps → should stay 1080p60.
        assert_eq!((s.height, s.fps), (None, None));
        // ~1.4 Mbps: 1080p60 would look poor; something smaller must be chosen.
        let FitOutcome::Encode(s) = solve(&input(12.0, 60.0, 1920, 1080, 60.0)) else {
            panic!()
        };
        assert!(s.height.is_some() || s.fps.is_some());
        assert!(s.quality >= Quality::Good);
        // 1080p60 at ~1.9 Mbps: half rate at full size is enough.
        let FitOutcome::Encode(s) = solve(&input(16.0, 60.0, 1920, 1080, 60.0)) else {
            panic!()
        };
        assert_eq!((s.height, s.fps), (None, Some(30.0)));
    }

    #[test]
    fn bigger_than_original_keeps_original() {
        let mut i = input(500.0, 10.0, 1920, 1080, 30.0);
        i.original_bytes = 25e6;
        assert!(matches!(solve(&i), FitOutcome::Original { .. }));
        i.original_possible = false;
        let FitOutcome::Encode(s) = solve(&i) else {
            panic!()
        };
        assert!(s.video_kbps <= 21_000);
        assert_eq!(s.quality, Quality::Great);
    }

    #[test]
    fn too_small_reports_a_working_minimum() {
        let i = input(1.0, 3600.0, 1920, 1080, 30.0);
        let FitOutcome::TooSmall { min_bytes } = solve(&i) else {
            panic!()
        };
        let mut j = i.clone();
        j.target_bytes = min_bytes;
        assert!(
            matches!(solve(&j), FitOutcome::Encode(_)),
            "minimum {min_bytes} doesn't solve"
        );
        assert_eq!(min_bytes % 1e6, 0.0);
    }

    #[test]
    fn quality_monotonic_in_size() {
        let mut last = Quality::Low;
        for mb in [6.0, 8.0, 10.0, 15.0, 25.0, 40.0, 60.0, 100.0] {
            if let FitOutcome::Encode(s) = solve(&input(mb, 60.0, 1920, 1080, 30.0)) {
                assert!(s.quality >= last, "{mb}: {:?} < {:?}", s.quality, last);
                last = s.quality;
            }
        }
    }

    #[test]
    fn audio_shrinks_with_budget() {
        assert_eq!(audio_kbps(5000.0, &[2], &[Some(256)]), 160);
        assert!(audio_kbps(300.0, &[2], &[Some(256)]) <= 64);
        assert_eq!(audio_kbps(5000.0, &[2], &[Some(96)]), 96);
        assert_eq!(audio_kbps(100.0, &[2], &[None]), 32);
    }

    #[test]
    fn retry_correction_lands_under() {
        // 10% overshoot → next attempt aims lower than proportional.
        let k = corrected_video_kbps(1000, 11e6, 10e6, 1e6, 50_000.0);
        assert!(k < 1000 * 9 / 10);
    }

    #[test]
    fn audio_only() {
        let mut i = input(5.0, 300.0, 0, 0, 0.0);
        i.original_possible = false;
        let FitOutcome::Encode(s) = solve(&i) else {
            panic!()
        };
        assert!(
            s.audio_kbps >= 96 && s.audio_kbps <= 160,
            "{}",
            s.audio_kbps
        );
    }
}
