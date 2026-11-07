//! Size and time estimates for the export dialog.
//!
//! Original quality is estimated from each file's average rate. Re-encodes
//! start from a formula and are refined by a calibration: a three-second
//! sample encode with the same settings, timed and weighed, scaled up.

use crate::fit::{self, FitOutcome};
use crate::plan::{self, encode_cost, CutStyle, Facts, Planner, Step};
use crate::request::ExportRequest;
use cutline_core::{Profile, Quality, RateControl, Segment, Timestamp, VideoCodec};
use cutline_ffmpeg::{runner, Capabilities, MediaInfo, Tools};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// Wall seconds per media second for x265 medium at 1080p30, before calibration.
/// Conservative for a 4-core laptop.
const BASE_SECS_PER_SEC: f64 = 2.2;
/// Copying runs at roughly disk speed.
const COPY_BYTES_PER_SEC: f64 = 150e6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub bytes: f64,
    pub seconds: f64,
    pub quality: Option<Quality>,
    /// One plain sentence when something is unusual.
    pub note: Option<String>,
    /// From a real sample encode, not just a formula.
    pub calibrated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// Output bytes per media second (only meaningful for quality-based encodes).
    pub bytes_per_sec: f64,
    /// Wall seconds per media second of encoding.
    pub secs_per_sec: f64,
}

/// Stable key for caching calibrations: same sources, same profile → same result.
pub fn calibration_key(req: &ExportRequest) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for s in &req.segments {
        s.path.hash(&mut h);
    }
    serde_json::to_string(&req.profile.video)
        .unwrap_or_default()
        .hash(&mut h);
    serde_json::to_string(&req.profile.audio)
        .unwrap_or_default()
        .hash(&mut h);
    req.profile.container.extension("mp4").hash(&mut h);
    h.finish()
}

fn segs(req: &ExportRequest) -> Vec<&Segment> {
    req.segments.iter().collect()
}

/// Original quality: copy size and time.
pub fn original(
    req: &ExportRequest,
    sources: &HashMap<PathBuf, MediaInfo>,
    caps: &Capabilities,
    cal: Option<Calibration>,
) -> Estimate {
    let s = segs(req);
    let dur = req.total_duration();
    if !plan::copy_possible(req, sources, caps) {
        // Converted at matching quality.
        let mut p = Profile::original();
        let first = s.first().and_then(|x| sources.get(&x.path));
        let hevc = first
            .and_then(|m| m.video.as_ref())
            .map(|v| v.codec != "h264")
            .unwrap_or(true);
        p.video.codec = if hevc {
            VideoCodec::H265
        } else {
            VideoCodec::H264
        };
        p.video.rate = RateControl::Crf(if hevc { 18 } else { 16 });
        let mut e = encode(
            &ExportRequest {
                profile: p,
                ..req.clone()
            },
            sources,
            cal,
        );
        e.bytes = plan::estimate_copy_bytes(&s, sources, &req.audio_tracks);
        e.quality = None;
        e.note = Some(if caps.smart_cut {
            "Your videos don't match, so they're converted. Same look.".into()
        } else {
            "Your ffmpeg is old, so the video is converted. Same look.".into()
        });
        return e;
    }
    let mut bytes = plan::estimate_copy_bytes(&s, sources, &req.audio_tracks);
    // The re-encoded edges run up to 1.5x the source rate (see plan::matching_encoder).
    for seg in &s {
        if let Some(m) = sources.get(&seg.path) {
            bytes += m.video_bitrate() / 8.0 * 0.5 * 4.0_f64.min(seg.len().secs());
        }
    }
    // Each cut re-encodes up to one keyframe interval at each end (~2 s each, typically).
    let mut edge_work = 0.0;
    for seg in &s {
        if let Some(v) = sources.get(&seg.path).and_then(|m| m.video.as_ref()) {
            let codec = if v.codec == "h264" {
                VideoCodec::H264
            } else {
                VideoCodec::H265
            };
            let per = cal.map(|c| c.secs_per_sec).unwrap_or(
                BASE_SECS_PER_SEC * encode_cost(codec, "medium", v.width, v.height, v.fps),
            );
            edge_work += per * 4.0_f64.min(seg.len().secs());
        }
    }
    Estimate {
        bytes,
        seconds: bytes / COPY_BYTES_PER_SEC + edge_work + 1.0,
        quality: None,
        note: None,
        calibrated: cal.is_some() || dur < 1.0,
    }
}

fn out_picture(req: &ExportRequest, first: Option<&MediaInfo>) -> (u32, u32, f64) {
    let v = first.and_then(|m| m.video.as_ref());
    let (w, h) = v.map(|v| v.display_size()).unwrap_or((1920, 1080));
    let fps = v.map(|v| v.fps).unwrap_or(30.0);
    let (mut ow, mut oh, mut of) = (w, h, fps);
    if let Some(cap) = req.profile.video.max_height {
        if h > cap {
            oh = cap;
            ow = (w as f64 * cap as f64 / h as f64) as u32;
        }
    }
    if let Some(cap) = req.profile.video.max_fps {
        if of > cap + 0.01 {
            of = if fps / 2.0 <= cap + 0.01 {
                fps / 2.0
            } else {
                cap
            };
        }
    }
    (ow, oh, of)
}

/// A quality-based (CRF) or bitrate preset.
pub fn encode(
    req: &ExportRequest,
    sources: &HashMap<PathBuf, MediaInfo>,
    cal: Option<Calibration>,
) -> Estimate {
    let dur = req.total_duration();
    let first = req.segments.first().and_then(|s| sources.get(&s.path));
    let (w, h, fps) = out_picture(req, first);
    let p = &req.profile;
    let n_audio = first
        .map(|m| {
            req.audio_tracks
                .iter()
                .filter(|t| **t < m.audio.len())
                .count()
        })
        .unwrap_or(0);
    let audio_bps = if p.audio.codec == cutline_core::AudioCodec::None {
        0.0
    } else {
        p.audio.bitrate_kbps as f64 * 1000.0 * n_audio as f64
    };
    if p.audio_only() || first.and_then(|m| m.video.as_ref()).is_none() {
        let secs = cal.map(|c| c.secs_per_sec).unwrap_or(0.02) * dur + 1.0;
        let q = if p.audio.bitrate_kbps >= 160 {
            Quality::Great
        } else if p.audio.bitrate_kbps >= 112 {
            Quality::Good
        } else {
            Quality::Okay
        };
        return Estimate {
            bytes: audio_bps / 8.0 * dur,
            seconds: secs,
            quality: Some(q),
            note: None,
            calibrated: cal.is_some(),
        };
    }
    let codec = if p.video.codec == VideoCodec::H264 {
        VideoCodec::H264
    } else {
        VideoCodec::H265
    };
    let formula_secs = BASE_SECS_PER_SEC * encode_cost(codec, &p.video.preset, w, h, fps);
    let (video_bps, quality, passes) = match p.video.rate {
        RateControl::Crf(crf) => {
            // Each 6 CRF steps roughly halves the bitrate; anchor at the "good" rate.
            let anchor = fit::good_bitrate(codec, w, h, fps) * 1.2;
            let x265_equiv = if codec == VideoCodec::H265 {
                crf as f64
            } else {
                crf as f64 + 4.0
            };
            let bps = anchor * 2f64.powf((24.0 - x265_equiv) / 6.0);
            // Can't come out much bigger than what it's made from at visually-lossless settings.
            let src = first.map(|m| m.video_bitrate()).unwrap_or(bps);
            (
                bps.min(src.max(1.0) * 1.3),
                Quality::from_crf(codec, crf),
                1.0,
            )
        }
        RateControl::Bitrate(k) => (
            k as f64 * 1000.0,
            ratio_quality(k as f64 * 1000.0, codec, w, h, fps),
            1.0 + plan::first_pass_cost(codec),
        ),
        RateControl::TargetSize(mb) => (
            mb * 1e6 * 8.0 / dur.max(0.1) - audio_bps,
            Quality::Good,
            1.0 + plan::first_pass_cost(codec),
        ),
    };
    let (bytes, secs_per_sec) = match cal {
        Some(c) => {
            let b = if matches!(p.video.rate, RateControl::Crf(_)) {
                c.bytes_per_sec * dur
            } else {
                (video_bps + audio_bps) / 8.0 * dur
            };
            (b, c.secs_per_sec)
        }
        None => ((video_bps + audio_bps) / 8.0 * dur, formula_secs),
    };
    Estimate {
        bytes,
        seconds: secs_per_sec * dur * passes + 1.5,
        quality: Some(quality),
        note: None,
        calibrated: cal.is_some(),
    }
}

fn ratio_quality(bps: f64, codec: VideoCodec, w: u32, h: u32, fps: f64) -> Quality {
    let r = bps / fit::good_bitrate(codec, w, h, fps);
    if r >= 2.2 {
        Quality::Great
    } else if r >= 1.0 {
        Quality::Good
    } else if r >= 0.55 {
        Quality::Okay
    } else {
        Quality::Low
    }
}

/// What Fit to size will do at `target_mb`, as the dialog shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum FitEstimate {
    /// Already fits: Original quality will be used.
    Original(Estimate),
    Encode(Estimate),
    TooSmall {
        min_mb: u64,
    },
}

pub fn fit(
    req: &ExportRequest,
    sources: &HashMap<PathBuf, MediaInfo>,
    caps: &Capabilities,
    target_mb: f64,
    cal: Option<Calibration>,
) -> FitEstimate {
    let s = segs(req);
    let codec = VideoCodec::H265;
    let possible = plan::copy_possible(req, sources, caps);
    let input = plan::fit_input(req, sources, &s, target_mb * 1e6, codec, possible);
    match fit::solve(&input) {
        FitOutcome::Original { .. } => {
            let mut r = req.clone();
            r.profile = Profile::original();
            let mut e = original(&r, sources, caps, None);
            e.quality = Some(Quality::Great);
            e.note = Some("Already fits, so it keeps the original quality.".into());
            FitEstimate::Original(e)
        }
        FitOutcome::TooSmall { min_bytes } => FitEstimate::TooSmall {
            min_mb: (min_bytes / 1e6).ceil() as u64,
        },
        FitOutcome::Encode(st) => {
            let dur = req.total_duration();
            let first = s.first().and_then(|x| sources.get(&x.path));
            let fps = st
                .fps
                .or(first.and_then(|m| m.video.as_ref()).map(|v| v.fps))
                .unwrap_or(30.0);
            let (w, h) = if st.width > 0 {
                (st.width, st.height.unwrap_or(input.src_height))
            } else {
                (0, 0)
            };
            let per = cal
                .map(|c| c.secs_per_sec)
                .unwrap_or(BASE_SECS_PER_SEC * encode_cost(codec, "medium", w, h, fps));
            let note = st
                .height
                .map(|_| "Made smaller on screen so it still looks good at this size.".to_string());
            FitEstimate::Encode(Estimate {
                bytes: st.expected_bytes,
                seconds: if w > 0 {
                    per * dur * (1.0 + plan::first_pass_cost(codec)) + 2.0
                } else {
                    0.02 * dur + 1.0
                },
                quality: Some(st.quality),
                note,
                calibrated: cal.is_some(),
            })
        }
    }
}

/// `-x265-params` without the two-pass options.
fn single_pass_x265(params: &str) -> String {
    params
        .split(':')
        .filter(|kv| {
            let key = kv.split('=').next().unwrap_or("");
            !matches!(key, "pass" | "stats" | "slow-firstpass")
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// Encode a short sample with the request's settings and time it.
///
/// For copy requests this measures the matching re-encode used at cut edges.
pub fn calibrate(
    tools: &Tools,
    req: &ExportRequest,
    sources: &HashMap<PathBuf, MediaInfo>,
    cancel: &AtomicBool,
) -> Option<Calibration> {
    let longest = req.segments.iter().max_by_key(|s| s.len())?;
    let len = longest.len().secs();
    let sample = len.min(3.0);
    if sample < 0.5 {
        return None;
    }
    let mid = longest.start.secs() + (len - sample) / 2.0;
    let mut r = req.clone();
    r.segments = vec![Segment {
        start: Timestamp::from_secs(mid),
        end: Timestamp::from_secs(mid + sample),
        ..longest.clone()
    }];
    r.separate = false;
    r.overwrite = true;
    let tmp = std::env::temp_dir().join(format!(
        "cutline-cal-{}-{:x}",
        std::process::id(),
        calibration_key(req)
    ));
    std::fs::create_dir_all(&tmp).ok()?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _c = Cleanup(tmp.clone());
    if r.profile.is_copy() {
        // Time the edge re-encode, which dominates copy exports.
        let mut p = Profile::original();
        let hevc = sources
            .get(&longest.path)
            .and_then(|m| m.video.as_ref())
            .map(|v| v.codec != "h264")
            .unwrap_or(true);
        p.video.codec = if hevc {
            VideoCodec::H265
        } else {
            VideoCodec::H264
        };
        p.video.rate = RateControl::Crf(if hevc { 18 } else { 16 });
        r.profile = p;
    }
    if let RateControl::TargetSize(_) = r.profile.video.rate {
        // The solver needs the whole length to pick a size; sample at the solved settings instead.
        let s: Vec<&Segment> = req.segments.iter().collect();
        let input = plan::fit_input(
            req,
            sources,
            &s,
            match req.profile.video.rate {
                RateControl::TargetSize(mb) => mb * 1e6,
                _ => 0.0,
            },
            VideoCodec::H265,
            false,
        );
        if let FitOutcome::Encode(st) = fit::solve(&input) {
            r.profile.video.rate = RateControl::Bitrate(st.video_kbps);
            r.profile.video.max_height = st.height;
            r.profile.video.max_fps = st.fps;
        } else {
            return None;
        }
    }
    r.output_dir = tmp.clone();
    r.name = "sample".into();
    let facts = Facts {
        sources: sources.clone(),
        packets: vec![],
    };
    let planner = Planner {
        req: &r,
        facts: &facts,
        caps: &tools.caps,
        tmp: &tmp,
        pass_log_dir: &tmp,
    };
    let plan = planner.plan_with(CutStyle::Smart, None).ok()?;
    let out = plan.outputs.first()?;
    // Only the final pass: that's what we want to time and weigh.
    let step = out.steps.iter().rev().find_map(|s| match s {
        Step::Run {
            cmd,
            second_pass: true,
            ..
        } => Some(cmd.clone()),
        _ => None,
    })?;
    let mut cmd = step;
    // A single-pass stand-in for a two-pass encode: drop the pass options,
    // keep the rest (block size changes the speed a lot at small sizes).
    if let Some(i) = cmd.output_args.iter().position(|a| a == "-x265-params") {
        cmd.output_args[i + 1] = single_pass_x265(&cmd.output_args[i + 1]);
    }
    if let Some(i) = cmd.output_args.iter().position(|a| a == "-pass") {
        cmd.output_args.drain(i..i + 4);
    }
    let t0 = Instant::now();
    runner::run(tools, &cmd, sample, cancel, &mut |_| {}).ok()?;
    // ffmpeg start-up and seeking aren't proportional to length.
    let wall = (t0.elapsed().as_secs_f64() - 0.25).max(0.05);
    let bytes = std::fs::metadata(&out.temp_path).ok()?.len() as f64;
    Some(Calibration {
        bytes_per_sec: bytes / sample,
        secs_per_sec: wall / sample,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_keeps_x265_settings_but_not_passes() {
        assert_eq!(
            single_pass_x265(
                "log-level=error:ctu=32:pass=1:stats=/tmp/a.x265.log:slow-firstpass=0"
            ),
            "log-level=error:ctu=32"
        );
        assert_eq!(
            single_pass_x265("log-level=error:pass=2:stats=/tmp/a.x265.log"),
            "log-level=error"
        );
    }
}
