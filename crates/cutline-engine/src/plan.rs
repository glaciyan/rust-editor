//! The planner: `ExportRequest` + facts about the sources → the exact ffmpeg
//! steps. Pure: no IO, so every decision is unit-testable and the Details
//! view can show precisely what will run.
//!
//! ## Original quality (copy) and the smart cut
//!
//! Copying a video stream can only start on a keyframe. To land on the exact
//! frame the user picked, each part is built from up to three pieces:
//!
//! ```text
//!  start        K1                                   Km        end
//!    |  head    |  copied packets (decode order K1..Km)  | tail  |
//!    re-encoded            stream copy                 re-encoded
//! ```
//!
//! * The copy covers the packets from keyframe K1 (first at/after `start`) up
//!   to, not including, keyframe Km (last at/before `end`), in decode order.
//!   Any decode-order prefix that stops right before a keyframe is decodable.
//! * Leading pictures after K1 (open-GOP RASL frames, which reference the
//!   previous GOP) are dropped with `noise=drop='lt(pts,startpts)'`: their
//!   references are gone. Leading pictures of Km arrive after Km in decode
//!   order with pts below the stop time, so they are dropped by pts too.
//! * The copy then shows frames `[K1, last_pts + 1 frame)`; the head covers
//!   `[start, K1)` and the tail `[last_pts + 1 frame, end)`, both decoded
//!   from the source and re-encoded with the source's codec, size, pixel
//!   format and colours.
//! * Pieces are MPEG-TS (Annex B, parameter sets in-band) joined with the
//!   concat demuxer using explicit durations; audio is stream-copied per
//!   part and joined the same way; the final mux is a plain copy.
//!
//! The job verifies the frames around every junction after muxing and falls
//! back to a keyframe cut (slightly longer, never broken) if anything fails
//! to decode.

use crate::fit::{self, FitOutcome, FitSettings};
use crate::request::ExportRequest;
use cutline_core::{AudioCodec, Profile, RateControl, Segment, Timestamp, VideoCodec};
use cutline_ffmpeg::{Capabilities, FfmpegCommand, Input, MediaInfo, Packet, VideoStream};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Facts gathered by the job before planning.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub sources: HashMap<PathBuf, MediaInfo>,
    /// Video packets for each segment (same index as `request.segments`),
    /// from the keyframe at/before its start to a little past its end.
    /// Only needed for copy exports.
    pub packets: Vec<Vec<Packet>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Write {
        path: PathBuf,
        contents: String,
    },
    Run {
        label: String,
        cmd: FfmpegCommand,
        out_duration: f64,
        weight: f64,
        second_pass: bool,
    },
    /// Decode a short window around each time (seconds) and fail on any error.
    Verify {
        path: PathBuf,
        around: Vec<f64>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FitInfo {
    pub target_bytes: f64,
    pub settings: FitSettings,
    pub audio_bytes: f64,
    pub overhead: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputPlan {
    pub final_path: PathBuf,
    pub temp_path: PathBuf,
    pub duration: f64,
    pub steps: Vec<Step>,
    /// Present for fit-to-size encodes: lets the job check and retry.
    pub fit: Option<FitInfo>,
    /// The copy plan used the smart cut and can fall back to keyframes.
    pub smart: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportPlan {
    pub outputs: Vec<OutputPlan>,
    /// Plain sentences for Details ("Cuts land on the nearest keyframe…").
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlanError {
    #[error("There's nothing to export: every part is cut out.")]
    Empty,
    #[error("Cutline couldn't read one of the videos ({0}).")]
    MissingSource(String),
    #[error("This size is too small for {length}. Try at least {min_mb} MB.")]
    TooSmall { length: String, min_mb: u64 },
    #[error("This ffmpeg can't make {0} video. Install an ffmpeg with {0} support.")]
    MissingEncoder(&'static str),
    #[error("The videos have no audio to export.")]
    NoAudio,
}

/// How a copy-mode part is cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutStyle {
    /// Head/tail re-encoded, exact frames.
    Smart,
    /// Copy from the keyframe before the start to the keyframe after the end.
    Keyframe,
}

pub struct Planner<'a> {
    pub req: &'a ExportRequest,
    pub facts: &'a Facts,
    pub caps: &'a Capabilities,
    /// Job scratch directory (inside the output folder, so the final rename is atomic).
    pub tmp: &'a Path,
    /// For two-pass logs; must not contain `:` (x265 parses its params on it).
    pub pass_log_dir: &'a Path,
}

fn secs(t: Timestamp) -> String {
    t.ffmpeg_arg()
}
fn f6(x: f64) -> String {
    format!("{x:.6}")
}
fn concat_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

/// Audio codecs each container takes without re-encoding.
fn container_takes(container_ext: &str, codec: &str) -> bool {
    match container_ext {
        "mkv" | "webm" => container_ext == "mkv" || matches!(codec, "opus" | "vorbis"),
        "mp4" | "m4v" | "mov" | "m4a" => matches!(
            codec,
            "aac" | "mp3" | "ac3" | "eac3" | "alac" | "opus" | "flac"
        ),
        _ => codec == "aac",
    }
}

fn is_mp4_family(ext: &str) -> bool {
    matches!(ext, "mp4" | "m4v" | "mov" | "m4a")
}

/// A short hash to keep temp names unique and colon-free.
fn tag(i: usize, j: usize) -> String {
    format!("{i:03}-{j:03}")
}

struct EncoderChoice {
    args: Vec<String>,
    /// For progress weighting.
    cost: f64,
}

/// Encoder settings that reproduce a source stream closely enough to splice
/// into it: same codec, size, pixel format and colours, near-transparent quality.
fn matching_encoder(v: &VideoStream, src_bps: f64, caps: &Capabilities) -> Option<EncoderChoice> {
    let mut args: Vec<String> = vec![];
    // Near-transparent quality, but never much above the source's own rate:
    // the re-encoded edges shouldn't bloat a lean file.
    let cap = ((src_bps * 1.5) / 1000.0).round().max(500.0) as u64;
    match v.codec.as_str() {
        "hevc" if caps.libx265 => {
            args.extend(["-c:v", "libx265", "-preset", "medium", "-crf", "18"].map(String::from));
            // No B-frames: DTS == PTS, so the copied piece that follows can
            // always start its DTS after ours (see the setts note in plan_copy).
            args.extend([
                "-x265-params".into(),
                format!(
                    "log-level=error:repeat-headers=1:bframes=0:vbv-maxrate={cap}:vbv-bufsize={}",
                    cap * 2
                ),
            ]);
        }
        "h264" if caps.libx264 => {
            args.extend(
                [
                    "-c:v", "libx264", "-preset", "medium", "-crf", "16", "-bf", "0",
                ]
                .map(String::from),
            );
            args.extend([
                "-maxrate".into(),
                format!("{cap}k"),
                "-bufsize".into(),
                format!("{}k", cap * 2),
            ]);
        }
        _ => return None,
    }
    args.extend(["-pix_fmt".into(), v.pix_fmt.clone()]);
    args.extend(colour_args(v));
    Some(EncoderChoice { args, cost: 1.0 })
}

fn colour_args(v: &VideoStream) -> Vec<String> {
    let mut a = vec![];
    if let Some(x) = &v.color_primaries {
        a.extend(["-color_primaries".into(), x.clone()]);
    }
    if let Some(x) = &v.color_trc {
        a.extend(["-color_trc".into(), x.clone()]);
    }
    if let Some(x) = &v.color_space {
        a.extend(["-colorspace".into(), x.clone()]);
    }
    if let Some(x) = &v.color_range {
        a.extend(["-color_range".into(), x.clone()]);
    }
    a
}

/// Bitstream filters that make a copied piece self-contained: Annex B with
/// parameter sets in-band at every keyframe. `hevc_mp4toannexb` already
/// repeats VPS/SPS/PPS before every IRAP picture; `h264_mp4toannexb` only
/// does so before IDR frames, so an open-GOP H.264 piece (which starts on a
/// non-IDR I-frame) would otherwise be decoded with the re-encoded head's
/// SPS/PPS. `dump_extra` re-inserts the stream's own.
fn annexb_bsf(codec: &str) -> Option<&'static str> {
    match codec {
        "hevc" => Some("hevc_mp4toannexb"),
        "h264" => Some("h264_mp4toannexb,dump_extra=freq=keyframe"),
        _ => None,
    }
}

/// Relative encode cost of one media second (1.0 ≈ x265 medium at 1080p30).
pub fn encode_cost(codec: VideoCodec, preset: &str, w: u32, h: u32, fps: f64) -> f64 {
    let px = (w.max(16) as f64 * h.max(16) as f64) / 2_073_600.0;
    let preset_k = match preset {
        "ultrafast" => 0.15,
        "superfast" => 0.22,
        "veryfast" => 0.3,
        "faster" => 0.45,
        "fast" => 0.6,
        "medium" => 1.0,
        "slow" => 2.0,
        "slower" => 4.0,
        "veryslow" => 8.0,
        _ => 1.0,
    };
    let codec_k = match codec {
        VideoCodec::H265 => 1.0,
        VideoCodec::H264 => 0.3,
        _ => 0.02,
    };
    px * (fps.max(1.0) / 30.0) * preset_k * codec_k
}

/// Below this output height x265 uses 32-pixel blocks instead of 64. Its
/// threads work on block rows in parallel, and a 240p frame has only four
/// rows of 64: most cores sit idle. Smaller blocks double the rows, which made
/// 240p–720p encodes 20–40% faster on 16 threads at no visible cost. At
/// 1080p there are rows enough, and smaller blocks only cost quality.
pub const X265_SMALL_CTU_BELOW: u32 = 1080;

/// The `-x265-params` every x265 encode of an `out_h`-pixel-high video starts from.
pub fn x265_params(out_h: u32) -> String {
    if out_h < X265_SMALL_CTU_BELOW {
        "log-level=error:ctu=32".into()
    } else {
        "log-level=error".into()
    }
}

/// What a first pass costs next to the second. x265's first pass runs at
/// full effort unless told otherwise; with `slow-firstpass=0` it still costs
/// about as much as the second (decoding and scaling the source is the same
/// work). x264's first pass is quick by default.
pub fn first_pass_cost(codec: VideoCodec) -> f64 {
    match codec {
        VideoCodec::H265 => 1.0,
        _ => 0.55,
    }
}

/// A cut decided from the packet list. All times are seconds on the `-ss` clock.
#[derive(Clone, Debug, PartialEq)]
pub struct CutPieces {
    pub head: Option<(f64, f64)>,
    /// (seek to keyframe, stop before this pts, last pts kept)
    pub copy: Option<CopyRange>,
    pub tail: Option<(f64, f64)>,
    /// Where the part actually starts/ends (differs from the request for keyframe cuts).
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CopyRange {
    pub key: f64,
    /// Stop before this keyframe (None: to the end of the file).
    pub stop: Option<f64>,
    pub last_pts: f64,
    /// Displayed length of the copied frames.
    pub shown: f64,
}

/// Decide head / copy / tail for one part. `packets` are in decode order.
pub fn decide_cut(
    packets: &[Packet],
    start: f64,
    end: f64,
    fps: f64,
    style: CutStyle,
) -> CutPieces {
    let frame = 1.0 / if fps > 0.0 { fps } else { 30.0 };
    let eps = frame / 4.0;
    let whole = CutPieces {
        head: Some((start, end)),
        copy: None,
        tail: None,
        start,
        end,
    };
    let keys: Vec<(usize, f64)> = packets
        .iter()
        .enumerate()
        .filter(|(_, p)| p.key)
        .map(|(i, p)| (i, p.pts))
        .collect();

    let copy_between = |ka: usize, kb: Option<usize>| -> Option<CopyRange> {
        let key = packets[ka].pts;
        let range = &packets[ka..kb.unwrap_or(packets.len())];
        let last_pts = range
            .iter()
            .map(|p| p.pts)
            .filter(|p| *p >= key - eps)
            .fold(f64::MIN, f64::max);
        if last_pts < key - eps {
            return None;
        }
        Some(CopyRange {
            key,
            stop: kb.map(|k| packets[k].pts),
            last_pts,
            shown: last_pts + frame - key,
        })
    };

    match style {
        CutStyle::Smart => {
            let k1 = keys.iter().find(|(_, t)| *t >= start - eps);
            let km = keys.iter().rev().find(|(_, t)| *t <= end + eps);
            let (Some(&(i1, t1)), Some(&(im, tm))) = (k1, km) else {
                return whole;
            };
            if tm <= t1 + eps {
                return whole; // fewer than two keyframes: just re-encode the part
            }
            let Some(copy) = copy_between(i1, Some(im)) else {
                return whole;
            };
            let copy_end = copy.last_pts + frame;
            let head = (t1 - start > eps).then_some((start, t1));
            let tail = (end - copy_end > eps).then_some((copy_end, end));
            CutPieces {
                head,
                copy: Some(copy),
                tail,
                start,
                end,
            }
        }
        CutStyle::Keyframe => {
            let ka = keys
                .iter()
                .rev()
                .find(|(_, t)| *t <= start + eps)
                .or(keys.first());
            let Some(&(ia, ta)) = ka else { return whole };
            let kb = keys
                .iter()
                .find(|(_, t)| *t >= end - eps && *t > ta + eps)
                .map(|(i, _)| *i);
            let Some(copy) = copy_between(ia, kb) else {
                return whole;
            };
            let real_end = ta + copy.shown;
            CutPieces {
                head: None,
                copy: Some(copy),
                tail: None,
                start: ta,
                end: real_end,
            }
        }
    }
}

impl<'a> Planner<'a> {
    fn info(&self, seg: &Segment) -> Result<&'a MediaInfo, PlanError> {
        self.facts
            .sources
            .get(&seg.path)
            .ok_or_else(|| MissingSourceName(seg).into())
    }

    fn first_info(&self) -> Result<&'a MediaInfo, PlanError> {
        self.info(self.req.segments.first().ok_or(PlanError::Empty)?)
    }

    fn ext(&self) -> String {
        self.req.extension(self.first_info().ok())
    }

    pub fn plan(&self) -> Result<ExportPlan, PlanError> {
        self.plan_with(CutStyle::Smart, None)
    }

    /// `style` applies to copy exports; `fit_kbps` overrides the solved video
    /// bitrate when a fit encode is retried.
    pub fn plan_with(
        &self,
        style: CutStyle,
        fit_kbps: Option<u32>,
    ) -> Result<ExportPlan, PlanError> {
        let req = self.req;
        if req.segments.is_empty() {
            return Err(PlanError::Empty);
        }
        for s in &req.segments {
            self.info(s)?;
        }
        let ext = self.ext();
        let finals = req.output_paths(&ext);
        let groups: Vec<Vec<usize>> = if req.separate {
            (0..req.segments.len()).map(|i| vec![i]).collect()
        } else {
            vec![(0..req.segments.len()).collect()]
        };

        let mut notes = vec![];
        let mut outputs = vec![];
        for (oi, (group, final_path)) in groups.iter().zip(finals).enumerate() {
            let temp_path = self.tmp.join(format!("out-{oi}.{ext}"));
            let duration: f64 = group.iter().map(|&i| req.segments[i].len().secs()).sum();
            let out = if req.profile.is_copy() {
                match self.copy_blocker(group) {
                    None => {
                        self.plan_copy(oi, group, &final_path, &temp_path, &ext, style, &mut notes)?
                    }
                    Some(why) => {
                        notes.push(why);
                        let p = self.original_as_encode();
                        self.plan_encode(oi, group, &final_path, &temp_path, &ext, &p, None)?
                    }
                }
            } else {
                self.plan_encode(
                    oi,
                    group,
                    &final_path,
                    &temp_path,
                    &ext,
                    &req.profile,
                    fit_kbps,
                )?
            };
            debug_assert!((out.duration - duration).abs() < 1.0 || style == CutStyle::Keyframe);
            outputs.push(out);
        }
        notes.dedup();
        Ok(ExportPlan { outputs, notes })
    }

    /// Why these parts can't be stream-copied together, in plain words.
    fn copy_blocker(&self, group: &[usize]) -> Option<String> {
        if !self.caps.smart_cut {
            return Some("This version of ffmpeg can't cut without converting, so the video is converted at high quality.".into());
        }
        let segs: Vec<&Segment> = group.iter().map(|&i| &self.req.segments[i]).collect();
        let first = self.info(segs[0]).ok()?;
        let fv = first.video.as_ref();
        for s in &segs[1..] {
            let m = self.info(s).ok()?;
            let same_video = match (fv, m.video.as_ref()) {
                (Some(a), Some(b)) => {
                    a.codec == b.codec
                        && a.width == b.width
                        && a.height == b.height
                        && a.pix_fmt == b.pix_fmt
                        && (a.fps - b.fps).abs() < 0.01
                        && a.rotation == b.rotation
                }
                (None, None) => true,
                _ => false,
            };
            if !same_video {
                return Some(
                    "Your videos have different sizes or formats, so they're converted to match."
                        .into(),
                );
            }
            for &pos in &self.req.audio_tracks {
                match (first.audio.get(pos), m.audio.get(pos)) {
                    (Some(a), Some(b))
                        if a.codec == b.codec
                            && a.sample_rate == b.sample_rate
                            && a.channels == b.channels => {}
                    _ => return Some(
                        "Your videos have different sound formats, so they're converted to match."
                            .into(),
                    ),
                }
            }
        }
        None
    }

    /// Original quality when copying isn't possible: same look, re-encoded.
    pub fn original_as_encode(&self) -> Profile {
        let first = self.first_info().ok();
        let hevc = first
            .and_then(|m| m.video.as_ref())
            .map(|v| v.codec != "h264")
            .unwrap_or(true);
        let mut p = Profile::original();
        p.video.codec = if hevc && self.caps.libx265 {
            VideoCodec::H265
        } else {
            VideoCodec::H264
        };
        p.video.rate = RateControl::Crf(if p.video.codec == VideoCodec::H265 {
            18
        } else {
            16
        });
        p.audio.codec = AudioCodec::Aac;
        p.audio.bitrate_kbps = 192;
        p
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_copy(
        &self,
        oi: usize,
        group: &[usize],
        final_path: &Path,
        temp_path: &Path,
        ext: &str,
        style: CutStyle,
        notes: &mut Vec<String>,
    ) -> Result<OutputPlan, PlanError> {
        let req = self.req;
        let first = self.info(&req.segments[group[0]])?;
        let mut steps = vec![];
        let mut vlist = String::new();
        let mut alist = String::new();
        let mut junctions = vec![];
        let mut timeline = 0.0;
        let mut any_smart = false;
        let has_video = first.video.is_some();
        let tracks: Vec<usize> = req
            .audio_tracks
            .iter()
            .copied()
            .filter(|p| *p < first.audio.len())
            .collect();

        for (j, &si) in group.iter().enumerate() {
            let seg = &req.segments[si];
            let m = self.info(seg)?;
            let (start, end) = (seg.start.secs(), seg.end.secs());
            let mut part_start = start;
            let mut part_end = end;

            if let Some(v) = &m.video {
                let fps = v.fps;
                let frame = 1.0 / fps.max(1.0);
                let encoder = matching_encoder(v, m.video_bitrate(), self.caps);
                let effective = if encoder.is_none() {
                    CutStyle::Keyframe
                } else {
                    style
                };
                if encoder.is_none() && style == CutStyle::Smart {
                    notes.push("Cuts in this kind of video land on the nearest keyframe, so a part may start a moment early.".into());
                }
                let empty = vec![];
                let packets = self.facts.packets.get(si).unwrap_or(&empty);
                let mut pieces = decide_cut(packets, start, end, fps, effective);
                if pieces.copy.is_none() && encoder.is_none() {
                    // Can't re-encode and nothing to copy: copy the whole file range.
                    pieces = CutPieces {
                        head: None,
                        copy: Some(CopyRange {
                            key: start,
                            stop: None,
                            last_pts: end,
                            shown: end - start,
                        }),
                        tail: None,
                        start,
                        end,
                    };
                }
                part_start = pieces.start;
                let _ = pieces.end;
                let rot_free = ["-noautorotate"];
                let mut piece_n = 0;
                let mut encode_piece =
                    |steps: &mut Vec<Step>, vlist: &mut String, a: f64, b: f64, what: &str| {
                        let enc = encoder
                            .as_ref()
                            .expect("encode only when an encoder exists");
                        let frames = ((b - a) * fps).round().max(1.0);
                        let shown = frames / fps;
                        let path = self.tmp.join(format!("v-{}-{piece_n}.ts", tag(oi, j)));
                        piece_n += 1;
                        let cmd = FfmpegCommand::new(&path)
                            .input(Input::new(&m.path).args(rot_free).args([
                                "-ss".to_string(),
                                f6(a),
                                "-t".into(),
                                f6(shown - frame / 2.0),
                            ]))
                            .args([
                                "-map".to_string(),
                                format!("0:{}", v.index),
                                "-an".into(),
                                "-sn".into(),
                                "-dn".into(),
                            ])
                            .args(enc.args.iter().cloned())
                            .args(["-fps_mode", "passthrough", "-f", "mpegts"]);
                        steps.push(Step::Run {
                            label: format!("Re-encoding the {what} of part {}", j + 1),
                            cmd,
                            out_duration: shown,
                            weight: shown
                                * enc.cost
                                * encode_cost(
                                    if v.codec == "h264" {
                                        VideoCodec::H264
                                    } else {
                                        VideoCodec::H265
                                    },
                                    "medium",
                                    v.width,
                                    v.height,
                                    fps,
                                ),
                            second_pass: false,
                        });
                        let _ = writeln!(
                            vlist,
                            "file {}\nduration {}",
                            concat_quote(&path),
                            f6(shown)
                        );
                        shown
                    };

                let mut t = timeline;
                if let Some((a, b)) = pieces.head {
                    t += encode_piece(
                        &mut steps,
                        &mut vlist,
                        a,
                        b,
                        if pieces.copy.is_some() {
                            "start"
                        } else {
                            "whole"
                        },
                    );
                    if pieces.copy.is_some() {
                        junctions.push(t);
                    }
                    any_smart = true;
                }
                if let Some(c) = &pieces.copy {
                    let path = self.tmp.join(format!("v-{}-c.ts", tag(oi, j)));
                    // Seek a hair past the keyframe so ffmpeg picks exactly it.
                    let seek = c.key + frame / 8.0;
                    let mut input = Input::new(&m.path)
                        .args(rot_free)
                        .args(["-ss".to_string(), f6(seek)]);
                    if let Some(stop) = c.stop {
                        input = input.args(["-t".to_string(), f6((stop - seek) - frame / 8.0)]);
                    }
                    let keep_until = c.last_pts - c.key + frame / 4.0;
                    let drop = format!(
                        "noise=drop='lt(pts\\,startpts)+gt((pts-startpts)*tb\\,{keep_until:.6})'"
                    );
                    // With the leading pictures gone, the first kept packet's DTS still
                    // sits several frames before its PTS, i.e. before the re-encoded
                    // head's last DTS. Start the DTS at the keyframe's PTS and keep it
                    // strictly increasing; the dropped packets left room for that.
                    let drop = format!(
                        "{drop},setts=pts=PTS:dts='if(eq(N\\,0)\\,PTS\\,max(DTS\\,PREV_OUTDTS+1))'"
                    );
                    let bsf = match annexb_bsf(&v.codec) {
                        Some(a) => format!("{a},{drop}"),
                        None => drop,
                    };
                    let piece_fmt = if annexb_bsf(&v.codec).is_some() {
                        "mpegts"
                    } else {
                        "matroska"
                    };
                    let path = if piece_fmt == "matroska" {
                        path.with_extension("mkv")
                    } else {
                        path
                    };
                    let cmd = FfmpegCommand::new(&path)
                        .input(input)
                        .args([
                            "-map".to_string(),
                            format!("0:{}", v.index),
                            "-an".into(),
                            "-sn".into(),
                            "-dn".into(),
                        ])
                        .args([
                            "-c:v".to_string(),
                            "copy".into(),
                            "-bsf:v".into(),
                            bsf,
                            "-f".into(),
                            piece_fmt.into(),
                        ]);
                    steps.push(Step::Run {
                        label: format!("Copying part {}", j + 1),
                        cmd,
                        out_duration: c.shown,
                        weight: c.shown * 0.01,
                        second_pass: false,
                    });
                    let _ = writeln!(
                        vlist,
                        "file {}\nduration {}",
                        concat_quote(&path),
                        f6(c.shown)
                    );
                    t += c.shown;
                }
                if let Some((a, b)) = pieces.tail {
                    if pieces.copy.is_some() {
                        junctions.push(t);
                    }
                    t += encode_piece(&mut steps, &mut vlist, a, b, "end");
                    any_smart = true;
                }
                part_end = part_start + (t - timeline);
                if j + 1 < group.len() {
                    junctions.push(t);
                }
            }

            // Audio for this part, cut to the same span as the video.
            if !tracks.is_empty() {
                let dur = part_end - part_start;
                let path = self.tmp.join(format!("a-{}.mka", tag(oi, j)));
                let mut cmd = FfmpegCommand::new(&path).input(Input::new(&m.path).args([
                    "-ss".to_string(),
                    f6(part_start),
                    "-t".into(),
                    f6(dur),
                ]));
                for (k, &pos) in tracks.iter().enumerate() {
                    let a = &m.audio[pos];
                    cmd = cmd.args(["-map".to_string(), format!("0:{}", a.index)]);
                    if container_takes(ext, &a.codec) {
                        cmd = cmd.args([format!("-c:a:{k}"), "copy".into()]);
                    } else {
                        cmd = cmd.args([
                            format!("-c:a:{k}"),
                            "aac".into(),
                            format!("-b:a:{k}"),
                            "192k".into(),
                        ]);
                    }
                }
                cmd = cmd.args(["-vn", "-sn", "-dn", "-f", "matroska"]);
                steps.push(Step::Run {
                    label: format!("Copying the sound of part {}", j + 1),
                    cmd,
                    out_duration: dur,
                    weight: dur * 0.004,
                    second_pass: false,
                });
                let _ = writeln!(
                    alist,
                    "file {}\nduration {}\noutpoint {}",
                    concat_quote(&path),
                    f6(dur),
                    f6(dur)
                );
            }
            timeline += part_end - part_start;
        }

        let vtxt = self.tmp.join(format!("video-{oi}.txt"));
        let atxt = self.tmp.join(format!("audio-{oi}.txt"));
        let mut mux = FfmpegCommand::new(temp_path);
        let mut n_inputs = 0;
        if has_video {
            steps.push(Step::Write {
                path: vtxt.clone(),
                contents: vlist,
            });
            let mut i = Input::new(&vtxt).args(["-f", "concat", "-safe", "0"]);
            if let Some(v) = &first.video {
                if v.rotation != 0 {
                    i = i.args(["-display_rotation:v:0".to_string(), v.rotation.to_string()]);
                }
            }
            mux = mux.input(i).args(["-map", "0:v:0"]);
            n_inputs += 1;
        }
        if !tracks.is_empty() {
            steps.push(Step::Write {
                path: atxt.clone(),
                contents: alist,
            });
            mux = mux.input(Input::new(&atxt).args(["-f", "concat", "-safe", "0"]));
            for k in 0..tracks.len() {
                mux = mux.args(["-map".to_string(), format!("{n_inputs}:a:{k}")]);
            }
            for (k, &pos) in tracks.iter().enumerate() {
                if let Some(lang) = &first.audio[pos].language {
                    mux = mux.args([format!("-metadata:s:a:{k}"), format!("language={lang}")]);
                }
                if let Some(title) = &first.audio[pos].title {
                    mux = mux.args([format!("-metadata:s:a:{k}"), format!("title={title}")]);
                }
            }
        }
        mux = mux.args(["-c", "copy"]);
        if is_mp4_family(ext) {
            if first
                .video
                .as_ref()
                .map(|v| v.codec == "hevc")
                .unwrap_or(false)
            {
                // `hvc1` keeps parameter sets only in the sample entry, and
                // ffmpeg 9 strips in-band ones when muxing it. A stream joined
                // from pieces with different parameter sets (a re-encoded
                // edge, or another file) then decodes the copied pieces with
                // the wrong ones. `hev1` keeps them in-band. Apple players
                // want `hvc1`, so use it whenever there's one set throughout.
                let one_source = group
                    .iter()
                    .all(|&si| req.segments[si].path == req.segments[group[0]].path);
                let tag = if any_smart || !one_source {
                    "hev1"
                } else {
                    "hvc1"
                };
                mux = mux.args(["-tag:v", tag]);
            }
            mux = mux.args(["-movflags", "+faststart"]);
        }
        steps.push(Step::Run {
            label: "Putting it together".into(),
            cmd: mux,
            out_duration: timeline,
            weight: timeline * 0.01,
            second_pass: false,
        });
        if any_smart && has_video && !junctions.is_empty() {
            steps.push(Step::Verify {
                path: temp_path.to_path_buf(),
                around: junctions,
            });
        }
        if style == CutStyle::Keyframe {
            notes.push("Cuts were moved to the nearest keyframes, so parts may start a moment early and end a moment late.".into());
        }
        Ok(OutputPlan {
            final_path: final_path.to_path_buf(),
            temp_path: temp_path.to_path_buf(),
            duration: timeline,
            steps,
            fit: None,
            smart: any_smart,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_encode(
        &self,
        oi: usize,
        group: &[usize],
        final_path: &Path,
        temp_path: &Path,
        ext: &str,
        profile: &Profile,
        fit_kbps: Option<u32>,
    ) -> Result<OutputPlan, PlanError> {
        let req = self.req;
        let segs: Vec<&Segment> = group.iter().map(|&i| &req.segments[i]).collect();
        let infos: Vec<&MediaInfo> = segs
            .iter()
            .map(|s| self.info(s))
            .collect::<Result<_, _>>()?;
        let first = infos[0];
        let duration: f64 = segs.iter().map(|s| s.len().secs()).sum();
        let want_video = profile.video.codec != VideoCodec::None && first.video.is_some();
        let codec = match profile.video.codec {
            VideoCodec::Copy | VideoCodec::H265 => VideoCodec::H265,
            c => c,
        };
        if want_video && codec == VideoCodec::H265 && !self.caps.libx265 {
            return Err(PlanError::MissingEncoder("H.265"));
        }
        if want_video && codec == VideoCodec::H264 && !self.caps.libx264 {
            return Err(PlanError::MissingEncoder("H.264"));
        }
        let tracks: Vec<usize> = if profile.audio.codec == AudioCodec::None {
            vec![]
        } else {
            req.audio_tracks
                .iter()
                .copied()
                .filter(|p| *p < first.audio.len())
                .collect()
        };
        if !want_video && tracks.is_empty() {
            return Err(PlanError::NoAudio);
        }

        // Output picture.
        let (sw, sh) = first
            .video
            .as_ref()
            .map(|v| v.display_size())
            .unwrap_or((0, 0));
        let src_fps = first.video.as_ref().map(|v| v.fps).unwrap_or(30.0);
        let mut fit_info = None;
        let mut out_h = sh;
        let mut out_w = sw;
        let mut out_fps = src_fps;
        let mut rate = profile.video.rate;
        let mut audio_kbps = profile.audio.bitrate_kbps;
        if let Some(cap) = profile.video.max_height {
            if sh > cap {
                out_h = cap;
                out_w = even(sw as f64 * cap as f64 / sh as f64);
            }
        }
        if let Some(cap) = profile.video.max_fps {
            if out_fps > cap + 0.01 {
                out_fps = if src_fps / 2.0 <= cap + 0.01 {
                    src_fps / 2.0
                } else {
                    cap
                };
            }
        }
        if let RateControl::TargetSize(mb) = profile.video.rate {
            let input = fit_input(req, &self.facts.sources, &segs, mb * 1e6, codec, false);
            match fit::solve(&input) {
                FitOutcome::Encode(s) => {
                    if let Some(h) = s.height {
                        out_h = h;
                        out_w = s.width;
                    }
                    if let Some(f) = s.fps {
                        out_fps = f;
                    }
                    let kbps = fit_kbps.unwrap_or(s.video_kbps);
                    rate = RateControl::Bitrate(kbps);
                    audio_kbps = s.audio_kbps;
                    let audio_bytes =
                        s.audio_kbps as f64 * 1000.0 / 8.0 * duration * tracks.len() as f64;
                    let overhead = fit::container_overhead(
                        duration,
                        if want_video { out_fps } else { 0.0 },
                        tracks.len(),
                    );
                    fit_info = Some(FitInfo {
                        target_bytes: mb * 1e6,
                        settings: s,
                        audio_bytes,
                        overhead,
                    });
                }
                FitOutcome::TooSmall { min_bytes } => {
                    return Err(PlanError::TooSmall {
                        length: cutline_core::format::length(duration),
                        min_mb: (min_bytes / 1e6).ceil() as u64,
                    })
                }
                FitOutcome::Original { .. } => {
                    // The dialog switches to Original before this happens; if not, encode near-transparent.
                    rate = RateControl::Crf(if codec == VideoCodec::H265 { 20 } else { 18 });
                }
            }
        }
        // 4:2:0 needs even dimensions; an odd source (e.g. 1280x719) that the
        // caps leave alone would otherwise reach the encoder as-is. Round down
        // so the scale+pad below never upscales.
        out_w = (out_w & !1).max(2);
        out_h = (out_h & !1).max(2);
        let same_size_everywhere = infos
            .iter()
            .all(|m| m.video.as_ref().map(|v| v.display_size()) == Some((sw, sh)));
        let same_fps_everywhere = infos.iter().all(|m| {
            m.video
                .as_ref()
                .map(|v| (v.fps - src_fps).abs() < 0.01)
                .unwrap_or(true)
        });
        let need_scale = !same_size_everywhere || out_h != sh || out_w != sw;
        let need_fps = !same_fps_everywhere || (out_fps - src_fps).abs() > 0.01;
        let pix = match codec {
            VideoCodec::H265
                if first
                    .video
                    .as_ref()
                    .map(|v| v.bit_depth >= 10)
                    .unwrap_or(false) =>
            {
                "yuv420p10le"
            }
            _ => "yuv420p",
        };
        let fps_expr = fps_rational(out_fps, first.video.as_ref());
        let layout = match tracks
            .iter()
            .map(|&p| first.audio[p].channels)
            .max()
            .unwrap_or(2)
            .min(profile.audio.max_channels.unwrap_or(8))
        {
            1 => "mono",
            2 => "stereo",
            6 => "5.1",
            8 => "7.1",
            _ => "stereo",
        };
        // Single part + copy audio: map the audio straight through.
        let audio_copy = profile.audio.codec == AudioCodec::Copy
            && segs.len() == 1
            && tracks
                .iter()
                .all(|&p| container_takes(ext, &first.audio[p].codec));

        let build = |with_audio: bool| -> (Vec<Input>, String, Vec<String>) {
            let mut inputs = vec![];
            let mut graph = String::new();
            let mut labels = String::new();
            for (i, (s, m)) in segs.iter().zip(&infos).enumerate() {
                inputs.push(Input::new(&m.path).args([
                    "-ss".to_string(),
                    secs(s.start),
                    "-t".into(),
                    secs(s.len()),
                ]));
                if want_video {
                    let v = m.video.as_ref().expect("checked");
                    let mut chain = vec![];
                    if need_scale {
                        chain.push(format!(
                            "scale={out_w}:{out_h}:force_original_aspect_ratio=decrease:flags=bicubic,pad={out_w}:{out_h}:(ow-iw)/2:(oh-ih)/2"
                        ));
                    }
                    if need_fps {
                        chain.push(format!("fps={fps_expr}"));
                    }
                    chain.push(format!("format={pix}"));
                    chain.push("setsar=1".into());
                    chain.push("setpts=PTS-STARTPTS".into());
                    let _ = write!(graph, "[{i}:{}]{}[v{i}];", v.index, chain.join(","));
                    let _ = write!(labels, "[v{i}]");
                }
                if with_audio && !audio_copy {
                    for (k, &pos) in tracks.iter().enumerate() {
                        match m.audio.get(pos) {
                            Some(a) => {
                                let _ = write!(
                                    graph,
                                    "[{i}:{}]aresample=48000,aformat=sample_rates=48000:channel_layouts={layout},asetpts=PTS-STARTPTS[a{i}_{k}];",
                                    a.index
                                );
                            }
                            None => {
                                let _ = write!(
                                    graph,
                                    "anullsrc=r=48000:cl={layout},atrim=duration={}[a{i}_{k}];",
                                    secs(s.len())
                                );
                            }
                        }
                        let _ = write!(labels, "[a{i}_{k}]");
                    }
                }
            }
            let na = if with_audio && !audio_copy {
                tracks.len()
            } else {
                0
            };
            let mut outs = String::new();
            let mut maps = vec![];
            if want_video {
                outs.push_str("[vout]");
                maps.extend(["-map".to_string(), "[vout]".into()]);
            }
            for k in 0..na {
                let _ = write!(outs, "[aout{k}]");
                maps.extend(["-map".to_string(), format!("[aout{k}]")]);
            }
            let _ = write!(
                graph,
                "{labels}concat=n={}:v={}:a={na}{outs}",
                segs.len(),
                want_video as u8
            );
            if with_audio && audio_copy {
                for &pos in &tracks {
                    maps.extend(["-map".to_string(), format!("0:{}", first.audio[pos].index)]);
                }
            }
            (inputs, graph, maps)
        };

        let mut venc: Vec<String> = vec![];
        let preset = profile.video.preset.clone();
        let pass_base = self
            .pass_log_dir
            .join(format!("cutline-{}-{oi}", std::process::id()));
        if want_video {
            match codec {
                VideoCodec::H265 => venc.extend([
                    "-c:v".into(),
                    "libx265".into(),
                    "-preset".into(),
                    preset.clone(),
                ]),
                _ => venc.extend([
                    "-c:v".into(),
                    "libx264".into(),
                    "-preset".into(),
                    preset.clone(),
                ]),
            }
            venc.extend(["-pix_fmt".into(), pix.into()]);
            if let Some(g) = profile.video.keyframe_interval {
                venc.extend(["-g".into(), g.to_string()]);
            }
            if let Some(v) = &first.video {
                let hdr = matches!(v.color_trc.as_deref(), Some("smpte2084" | "arib-std-b67"));
                if codec == VideoCodec::H265 && pix == "yuv420p10le" || !hdr {
                    venc.extend(colour_args(v));
                }
            }
            if is_mp4_family(ext) && codec == VideoCodec::H265 {
                venc.extend(["-tag:v".into(), "hvc1".into()]);
            }
        }
        let mut aenc: Vec<String> = vec![];
        for (k, &pos) in tracks.iter().enumerate() {
            if audio_copy {
                aenc.extend([format!("-c:a:{k}"), "copy".into()]);
            } else {
                let (name, extra): (&str, &[&str]) = match profile.audio.codec {
                    AudioCodec::Opus if self.caps.libopus => ("libopus", &[]),
                    AudioCodec::Opus => ("opus", &["-strict", "-2"]),
                    _ => ("aac", &[]),
                };
                aenc.extend([
                    format!("-c:a:{k}"),
                    name.into(),
                    format!("-b:a:{k}"),
                    format!("{audio_kbps}k"),
                ]);
                aenc.extend(extra.iter().map(|s| s.to_string()));
            }
            if let Some(lang) = &first.audio[pos].language {
                aenc.extend([format!("-metadata:s:a:{k}"), format!("language={lang}")]);
            }
        }
        if tracks.is_empty() {
            aenc.push("-an".into());
        }
        let mut tail_args: Vec<String> = vec![
            "-sn".into(),
            "-dn".into(),
            "-map_metadata".into(),
            "-1".into(),
        ];
        if is_mp4_family(ext) {
            tail_args.extend(["-movflags".into(), "+faststart".into()]);
        }
        if ext == "m4a" {
            tail_args.extend(["-f".into(), "mp4".into()]);
        }

        let cost = if want_video {
            encode_cost(codec, &preset, out_w, out_h, out_fps)
        } else {
            0.02
        };
        let mut steps = vec![];
        let video_kind = if codec == VideoCodec::H265 {
            "H.265"
        } else {
            "H.264"
        };
        let _ = video_kind;
        match (want_video, rate) {
            (true, RateControl::Bitrate(kbps)) => {
                let (inputs, graph, maps) = build(false);
                let mut p1 = FfmpegCommand::new("-").filter(graph);
                p1.inputs = inputs;
                p1 = p1
                    .args(maps)
                    .args(venc.iter().cloned())
                    .args(["-b:v".to_string(), format!("{kbps}k")]);
                p1 = p1
                    .args(pass_args(codec, 1, &pass_base, out_h))
                    .args(["-an", "-sn", "-dn", "-f", "null"]);
                steps.push(Step::Run {
                    label: "Measuring the video".into(),
                    cmd: p1,
                    out_duration: duration,
                    weight: duration * cost * first_pass_cost(codec),
                    second_pass: false,
                });
                let (inputs, graph, maps) = build(true);
                let mut p2 = FfmpegCommand::new(temp_path).filter(graph);
                p2.inputs = inputs;
                p2 = p2
                    .args(maps)
                    .args(venc.iter().cloned())
                    .args(["-b:v".to_string(), format!("{kbps}k")]);
                p2 = p2
                    .args(pass_args(codec, 2, &pass_base, out_h))
                    .args(aenc.iter().cloned())
                    .args(tail_args.iter().cloned());
                steps.push(Step::Run {
                    label: "Making the video".into(),
                    cmd: p2,
                    out_duration: duration,
                    weight: duration * cost,
                    second_pass: true,
                });
            }
            (wv, rate) => {
                let (inputs, graph, maps) = build(true);
                let mut c = FfmpegCommand::new(temp_path).filter(graph);
                c.inputs = inputs;
                c = c.args(maps);
                if wv {
                    let crf = match rate {
                        RateControl::Crf(q) => q,
                        _ => 23,
                    };
                    c = c
                        .args(venc.iter().cloned())
                        .args(["-crf".to_string(), crf.to_string()]);
                    if codec == VideoCodec::H265 {
                        c = c.args(["-x265-params".to_string(), x265_params(out_h)]);
                    }
                } else {
                    c = c.arg("-vn");
                }
                c = c.args(aenc.iter().cloned()).args(tail_args.iter().cloned());
                steps.push(Step::Run {
                    label: if wv {
                        "Making the video".into()
                    } else {
                        "Making the sound file".into()
                    },
                    cmd: c,
                    out_duration: duration,
                    weight: duration * cost,
                    second_pass: true,
                });
            }
        }
        Ok(OutputPlan {
            final_path: final_path.to_path_buf(),
            temp_path: temp_path.to_path_buf(),
            duration,
            steps,
            fit: fit_info,
            smart: false,
        })
    }
}

struct MissingSourceName<'a>(&'a Segment);
impl From<MissingSourceName<'_>> for PlanError {
    fn from(m: MissingSourceName<'_>) -> Self {
        PlanError::MissingSource(
            m.0.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    }
}

fn even(x: f64) -> u32 {
    ((x / 2.0).round() as u32).max(1) * 2
}

/// Exact rationals for common rates (`30000/1001`), else decimal.
fn fps_rational(fps: f64, v: Option<&VideoStream>) -> String {
    if let Some(v) = v {
        if (v.fps - fps).abs() < 0.001 {
            return format!("{}/{}", v.fps_num, v.fps_den);
        }
        if (v.fps / 2.0 - fps).abs() < 0.001 {
            return if v.fps_num % 2 == 0 {
                format!("{}/{}", v.fps_num / 2, v.fps_den)
            } else {
                format!("{}/{}", v.fps_num, v.fps_den * 2)
            };
        }
    }
    format!("{fps:.3}")
}

fn pass_args(codec: VideoCodec, pass: u8, base: &Path, out_h: u32) -> Vec<String> {
    match codec {
        VideoCodec::H265 => vec![
            "-x265-params".into(),
            format!(
                "{}:pass={pass}:stats={}{}",
                x265_params(out_h),
                base.with_extension("x265.log").to_string_lossy(),
                // Only the stats matter from pass 1; full effort there changed nothing.
                if pass == 1 { ":slow-firstpass=0" } else { "" }
            ),
        ],
        _ => vec![
            "-pass".into(),
            pass.to_string(),
            "-passlogfile".into(),
            base.with_extension("x264").to_string_lossy().into_owned(),
        ],
    }
}

/// The solver's view of an export.
pub fn fit_input(
    req: &ExportRequest,
    sources: &HashMap<PathBuf, MediaInfo>,
    segs: &[&Segment],
    target_bytes: f64,
    codec: VideoCodec,
    original_possible: bool,
) -> fit::FitInput {
    let first = segs.first().and_then(|s| sources.get(&s.path));
    let v = first.and_then(|m| m.video.as_ref());
    let (mut w, mut h) = v.map(|v| v.display_size()).unwrap_or((0, 0));
    let mut fps = v.map(|v| v.fps).unwrap_or(0.0);
    // The profile's caps come first: the solver only ever goes down from
    // them, the same way the planner applies them (`max_fps` halves the
    // rate when that's enough, else uses the cap).
    let mut capped = false;
    if let Some(cap) = req.profile.video.max_height {
        if h > cap {
            w = even(w as f64 * cap as f64 / h as f64);
            h = cap;
            capped = true;
        }
    }
    if let Some(cap) = req.profile.video.max_fps {
        if fps > cap + 0.01 {
            fps = if fps / 2.0 <= cap + 0.01 {
                fps / 2.0
            } else {
                cap
            };
            capped = true;
        }
    }
    let tracks: Vec<usize> = first
        .map(|m| {
            req.audio_tracks
                .iter()
                .copied()
                .filter(|p| *p < m.audio.len())
                .collect()
        })
        .unwrap_or_default();
    let duration: f64 = segs.iter().map(|s| s.len().secs()).sum();
    let original_bytes = estimate_copy_bytes(segs, sources, &req.audio_tracks);
    let audio_only = req.profile.video.codec == VideoCodec::None;
    fit::FitInput {
        target_bytes,
        duration,
        src_width: if audio_only { 0 } else { w },
        src_height: if audio_only { 0 } else { h },
        src_fps: fps,
        src_video_bps: first.map(|m| m.video_bitrate()).filter(|b| *b > 0.0),
        audio_channels: tracks
            .iter()
            .map(|&p| first.unwrap().audio[p].channels.min(2))
            .collect(),
        src_audio_kbps: tracks
            .iter()
            .map(|&p| first.unwrap().audio[p].bit_rate.map(|b| (b / 1000) as u32))
            .collect(),
        codec,
        original_bytes,
        original_possible: original_possible && !audio_only && !capped,
    }
}

/// What a stream copy of these segments weighs, from each file's average rate.
pub fn estimate_copy_bytes(
    segs: &[&Segment],
    sources: &HashMap<PathBuf, MediaInfo>,
    tracks: &[usize],
) -> f64 {
    segs.iter()
        .filter_map(|s| {
            let m = sources.get(&s.path)?;
            let unused_audio: f64 = m
                .audio
                .iter()
                .enumerate()
                .filter(|(i, _)| !tracks.contains(i))
                .map(|(_, a)| a.bit_rate.unwrap_or(128_000) as f64 / 8.0)
                .sum();
            Some((m.bytes_per_sec() - unused_audio).max(0.0) * s.len().secs())
        })
        .sum()
}

/// Whether Original quality can be a pure copy (no conversion) for this request.
pub fn copy_possible(
    req: &ExportRequest,
    sources: &HashMap<PathBuf, MediaInfo>,
    caps: &Capabilities,
) -> bool {
    let facts = Facts {
        sources: sources.clone(),
        packets: vec![],
    };
    let tmp = std::env::temp_dir();
    let p = Planner {
        req,
        facts: &facts,
        caps,
        tmp: &tmp,
        pass_log_dir: &tmp,
    };
    let groups: Vec<Vec<usize>> = if req.separate {
        (0..req.segments.len()).map(|i| vec![i]).collect()
    } else {
        vec![(0..req.segments.len()).collect()]
    };
    groups.iter().all(|g| p.copy_blocker(g).is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutline_ffmpeg::AudioStream;

    fn pk(pts: f64, key: bool) -> Packet {
        Packet { pts, key }
    }

    /// Open-GOP stream like x265's default at 30 fps: keyframes every 2 s,
    /// each followed in decode order by 3 leading pictures from before it.
    fn open_gop(seconds: f64) -> Vec<Packet> {
        let f = 1.0 / 30.0;
        let mut out = vec![];
        let gop = 60;
        let n = (seconds * 30.0) as usize;
        let mut k = 0;
        while k < n {
            out.push(pk(k as f64 * f, true));
            if k > 0 {
                for l in (1..=3).rev() {
                    out.push(pk((k - l) as f64 * f, false));
                }
            }
            let last = (k + gop).min(n);
            let trail_end = if last < n { last - 3 } else { last };
            for i in k + 1..trail_end {
                out.push(pk(i as f64 * f, false));
            }
            k += gop;
        }
        out
    }

    #[test]
    fn smart_cut_pieces_cover_exactly() {
        let p = open_gop(20.0);
        let c = decide_cut(&p, 3.0, 13.0, 30.0, CutStyle::Smart);
        let f = 1.0 / 30.0;
        let (h0, h1) = c.head.unwrap();
        assert!((h0 - 3.0).abs() < 1e-9 && (h1 - 4.0).abs() < 1e-9);
        let copy = c.copy.as_ref().unwrap();
        assert!((copy.key - 4.0).abs() < 1e-9);
        assert!((copy.stop.unwrap() - 12.0).abs() < 1e-9);
        // last pts copied: frame before the 3 leading pictures of K=12
        assert!(
            (copy.last_pts - (12.0 - 4.0 * f)).abs() < 1e-6,
            "{}",
            copy.last_pts
        );
        let (t0, t1) = c.tail.unwrap();
        assert!((t0 - (12.0 - 3.0 * f)).abs() < 1e-6 && (t1 - 13.0).abs() < 1e-9);
        // contiguous
        assert!((h1 - copy.key).abs() < 1e-9);
        assert!((copy.key + copy.shown - t0).abs() < 1e-6);
    }

    #[test]
    fn cut_on_keyframe_needs_no_head() {
        let p = open_gop(20.0);
        let c = decide_cut(&p, 4.0, 10.0, 30.0, CutStyle::Smart);
        assert!(c.head.is_none());
        assert!(c.tail.is_some());
    }

    #[test]
    fn short_part_is_fully_reencoded() {
        let p = open_gop(20.0);
        let c = decide_cut(&p, 4.5, 5.5, 30.0, CutStyle::Smart);
        assert_eq!(c.head, Some((4.5, 5.5)));
        assert!(c.copy.is_none());
    }

    #[test]
    fn keyframe_cut_widens() {
        let p = open_gop(20.0);
        let c = decide_cut(&p, 3.0, 9.0, 30.0, CutStyle::Keyframe);
        assert!((c.start - 2.0).abs() < 1e-9);
        assert!(c.end >= 9.0);
        assert!(c.head.is_none() && c.tail.is_none());
    }

    fn info(path: &str, codec: &str, w: u32, h: u32) -> MediaInfo {
        MediaInfo {
            path: path.into(),
            size_bytes: 25_000_000,
            duration: 20.0,
            start_time: 0.0,
            format_name: "mov,mp4".into(),
            bit_rate: Some(10_000_000),
            video: Some(VideoStream {
                index: 0,
                codec: codec.into(),
                profile: None,
                width: w,
                height: h,
                pix_fmt: "yuv420p".into(),
                fps: 30.0,
                fps_num: 30,
                fps_den: 1,
                bit_rate: Some(9_800_000),
                color_primaries: None,
                color_trc: None,
                color_space: None,
                color_range: None,
                rotation: 0,
                bit_depth: 8,
            }),
            audio: vec![AudioStream {
                index: 1,
                codec: "aac".into(),
                channels: 2,
                sample_rate: 48000,
                bit_rate: Some(128_000),
                language: None,
                title: None,
            }],
        }
    }

    fn caps() -> Capabilities {
        Capabilities {
            version: "ffmpeg version 6.1".into(),
            major: Some(6),
            libx265: true,
            libx264: true,
            libopus: true,
            aac: true,
            smart_cut: true,
        }
    }

    fn req(profile: Profile, segs: Vec<(&str, f64, f64)>) -> ExportRequest {
        ExportRequest {
            segments: segs
                .into_iter()
                .map(|(p, a, b)| Segment {
                    clip: 1,
                    path: p.into(),
                    start: Timestamp::from_secs(a),
                    end: Timestamp::from_secs(b),
                })
                .collect(),
            profile,
            output_dir: "/out".into(),
            name: "clip (cut)".into(),
            separate: false,
            audio_tracks: vec![0],
            overwrite: false,
        }
    }

    fn all_args(plan: &OutputPlan) -> Vec<String> {
        plan.steps
            .iter()
            .filter_map(|s| match s {
                Step::Run { cmd, .. } => Some(cmd.display("ffmpeg")),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn original_plan_is_copy_with_smart_edges() {
        let r = req(Profile::original(), vec![("/v/a.mp4", 3.0, 13.0)]);
        let facts = Facts {
            sources: [("/v/a.mp4".into(), info("/v/a.mp4", "hevc", 1280, 720))].into(),
            packets: vec![open_gop(20.0)],
        };
        let tmp = PathBuf::from("/out/.cutline-1");
        let plan = Planner {
            req: &r,
            facts: &facts,
            caps: &caps(),
            tmp: &tmp,
            pass_log_dir: &tmp,
        }
        .plan()
        .unwrap();
        let o = &plan.outputs[0];
        assert_eq!(o.final_path, PathBuf::from("/out/clip (cut).mp4"));
        let cmds = all_args(o);
        assert!(
            cmds.iter()
                .any(|c| c.contains("libx265") && c.contains("-ss 3.000000")),
            "{cmds:#?}"
        );
        assert!(cmds
            .iter()
            .any(|c| c.contains("-c:v copy") && c.contains("noise=drop=")));
        // Re-encoded edges bring their own parameter sets: keep them in-band.
        assert!(cmds.last().unwrap().contains("-tag:v hev1"));
        assert!(o.steps.iter().any(|s| matches!(s, Step::Verify { .. })));
        assert!((o.duration - 10.0).abs() < 0.05, "{}", o.duration);
    }

    #[test]
    fn pure_copy_keeps_the_apple_friendly_tag() {
        // Closed GOPs, keyframes every 2 s, parts cut on keyframes of one
        // file: nothing re-encoded, one set of parameter sets, so `hvc1`.
        let closed: Vec<Packet> = (0..600).map(|i| pk(i as f64 / 30.0, i % 60 == 0)).collect();
        let r = req(
            Profile::original(),
            vec![("/v/a.mp4", 2.0, 8.0), ("/v/a.mp4", 10.0, 14.0)],
        );
        let facts = Facts {
            sources: [("/v/a.mp4".into(), info("/v/a.mp4", "hevc", 1280, 720))].into(),
            packets: vec![closed.clone(), closed],
        };
        let tmp = PathBuf::from("/out/.cutline-1");
        let plan = Planner {
            req: &r,
            facts: &facts,
            caps: &caps(),
            tmp: &tmp,
            pass_log_dir: &tmp,
        }
        .plan()
        .unwrap();
        let o = &plan.outputs[0];
        assert!(!o.smart, "{:#?}", all_args(o));
        assert!(all_args(o).last().unwrap().contains("-tag:v hvc1"));
    }

    #[test]
    fn mismatched_sources_are_converted() {
        let r = req(
            Profile::original(),
            vec![("/v/a.mp4", 0.0, 5.0), ("/v/b.mp4", 0.0, 5.0)],
        );
        let facts = Facts {
            sources: [
                ("/v/a.mp4".into(), info("/v/a.mp4", "hevc", 1920, 1080)),
                ("/v/b.mp4".into(), info("/v/b.mp4", "hevc", 1280, 720)),
            ]
            .into(),
            packets: vec![open_gop(20.0), open_gop(20.0)],
        };
        let tmp = PathBuf::from("/tmp/x");
        let plan = Planner {
            req: &r,
            facts: &facts,
            caps: &caps(),
            tmp: &tmp,
            pass_log_dir: &tmp,
        }
        .plan()
        .unwrap();
        assert!(!plan.notes.is_empty());
        let cmds = all_args(&plan.outputs[0]);
        assert!(cmds[0].contains("concat=n=2:v=1:a=1"), "{}", cmds[0]);
        assert!(cmds[0].contains("scale=1920:1080"));
    }

    #[test]
    fn odd_source_size_is_made_even() {
        // A 1280x719 source is under the 1080p cap, so nothing else would
        // touch its size; libx264 rejects odd heights in yuv420p.
        let share = Profile::presets()
            .into_iter()
            .find(|p| p.id == "share")
            .unwrap();
        let r = req(share, vec![("/v/a.mp4", 0.0, 5.0)]);
        let facts = Facts {
            sources: [("/v/a.mp4".into(), info("/v/a.mp4", "h264", 1281, 719))].into(),
            packets: vec![],
        };
        let tmp = PathBuf::from("/tmp/x");
        let plan = Planner {
            req: &r,
            facts: &facts,
            caps: &caps(),
            tmp: &tmp,
            pass_log_dir: &tmp,
        }
        .plan()
        .unwrap();
        let cmds = all_args(&plan.outputs[0]);
        assert!(cmds[0].contains("scale=1280:718"), "{}", cmds[0]);
    }

    #[test]
    fn fit_is_two_pass_and_records_budget() {
        let r = req(Profile::fit(10.0), vec![("/v/a.mp4", 0.0, 20.0)]);
        let facts = Facts {
            sources: [("/v/a.mp4".into(), info("/v/a.mp4", "hevc", 1920, 1080))].into(),
            packets: vec![],
        };
        let tmp = PathBuf::from("/tmp/x");
        let plan = Planner {
            req: &r,
            facts: &facts,
            caps: &caps(),
            tmp: &tmp,
            pass_log_dir: &tmp,
        }
        .plan()
        .unwrap();
        let o = &plan.outputs[0];
        let cmds = all_args(o);
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].contains("pass=1") && cmds[0].ends_with("-f null -"));
        assert!(cmds[1].contains("pass=2") && cmds[1].contains("-c:a:0 aac"));
        assert!(cmds[0].contains("slow-firstpass=0"));
        assert!(!cmds[1].contains("slow-firstpass"));
        let fit = o.fit.as_ref().unwrap();
        assert!(fit.settings.expected_bytes <= 10e6 * (1.0 - fit::MARGIN) + 1.0);
    }

    #[test]
    fn small_x265_encodes_use_smaller_blocks() {
        assert_eq!(x265_params(240), "log-level=error:ctu=32");
        assert_eq!(x265_params(720), "log-level=error:ctu=32");
        assert_eq!(x265_params(1080), "log-level=error");
        assert_eq!(x265_params(2160), "log-level=error");
        let base = Path::new("/tmp/cutline-1-0");
        let p1 = pass_args(VideoCodec::H265, 1, base, 240).join(" ");
        let p2 = pass_args(VideoCodec::H265, 2, base, 240).join(" ");
        assert_eq!(
            p1,
            "-x265-params log-level=error:ctu=32:pass=1:stats=/tmp/cutline-1-0.x265.log:slow-firstpass=0"
        );
        assert_eq!(
            p2,
            "-x265-params log-level=error:ctu=32:pass=2:stats=/tmp/cutline-1-0.x265.log"
        );
        // x264 is unaffected.
        assert_eq!(pass_args(VideoCodec::H264, 1, base, 240)[0], "-pass");
    }

    #[test]
    fn fit_keeps_a_profiles_size_and_rate_caps() {
        // A 1440p60 source into a "720p, 30 fps, under N MB" profile: at
        // every budget the picture stays within the caps (the solver may
        // still go smaller).
        let mut src = info("/v/a.mp4", "hevc", 2560, 1440);
        let v = src.video.as_mut().unwrap();
        (v.fps, v.fps_num) = (60.0, 60);
        let facts = Facts {
            sources: [("/v/a.mp4".into(), src)].into(),
            packets: vec![],
        };
        let tmp = PathBuf::from("/tmp/x");
        for mb in [2.0, 5.0, 8.0, 30.0] {
            let mut p = Profile::fit(mb);
            p.video.max_height = Some(720);
            p.video.max_fps = Some(30.0);
            let r = req(p, vec![("/v/a.mp4", 0.0, 20.0)]);
            let plan = Planner {
                req: &r,
                facts: &facts,
                caps: &caps(),
                tmp: &tmp,
                pass_log_dir: &tmp,
            }
            .plan()
            .unwrap();
            let cmd = &all_args(&plan.outputs[0])[1];
            let scale = cmd.split("scale=").nth(1).unwrap();
            let (sw, rest) = scale.split_once(':').unwrap();
            let sh: u32 = rest.split(':').next().unwrap().parse().unwrap();
            assert!(
                sh <= 720 && sw.parse::<u32>().unwrap() <= 1280,
                "{mb} MB: {cmd}"
            );
            let fps = cmd.split("fps=").nth(1).unwrap().split('/').next().unwrap();
            assert!(fps.parse::<f64>().unwrap() <= 30.0, "{mb} MB: {cmd}");
        }
    }

    #[test]
    fn fit_too_small_is_plain_error() {
        let r = req(Profile::fit(0.2), vec![("/v/a.mp4", 0.0, 20.0)]);
        let facts = Facts {
            sources: [("/v/a.mp4".into(), info("/v/a.mp4", "hevc", 1920, 1080))].into(),
            packets: vec![],
        };
        let tmp = PathBuf::from("/tmp/x");
        let err = Planner {
            req: &r,
            facts: &facts,
            caps: &caps(),
            tmp: &tmp,
            pass_log_dir: &tmp,
        }
        .plan()
        .unwrap_err();
        assert!(matches!(err, PlanError::TooSmall { .. }));
        assert!(err.to_string().contains("Try at least"));
    }

    #[test]
    fn separate_outputs() {
        let mut r = req(
            Profile::original(),
            vec![("/v/a.mp4", 3.0, 7.0), ("/v/a.mp4", 9.0, 15.0)],
        );
        r.separate = true;
        let facts = Facts {
            sources: [("/v/a.mp4".into(), info("/v/a.mp4", "hevc", 1280, 720))].into(),
            packets: vec![open_gop(20.0), open_gop(20.0)],
        };
        let tmp = PathBuf::from("/tmp/x");
        let plan = Planner {
            req: &r,
            facts: &facts,
            caps: &caps(),
            tmp: &tmp,
            pass_log_dir: &tmp,
        }
        .plan()
        .unwrap();
        assert_eq!(plan.outputs.len(), 2);
        assert_eq!(
            plan.outputs[1].final_path,
            PathBuf::from("/out/clip (part 2).mp4")
        );
    }
}
