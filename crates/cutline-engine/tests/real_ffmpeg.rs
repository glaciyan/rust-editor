//! End-to-end exports against the real ffmpeg. Skipped (passing) when
//! ffmpeg with libx265/libx264 isn't installed.

use cutline_core::{Profile, Segment, Timestamp};
use cutline_engine::job::{Job, JobDetails};
use cutline_engine::request::ExportRequest;
use cutline_ffmpeg::Tools;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

fn tools() -> Option<Tools> {
    let t = Tools::locate().ok()?;
    (t.caps.libx265 && t.caps.libx264 && t.caps.smart_cut).then_some(t)
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cutline-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A clip that makes cutting hard: B-frames and (for `open`) open GOPs.
fn make_clip(t: &Tools, path: &Path, codec: &str, seconds: u32, rate: &str) {
    make_clip_gop(t, path, codec, seconds, rate, true)
}

fn make_clip_gop(t: &Tools, path: &Path, codec: &str, seconds: u32, rate: &str, open: bool) {
    let mut c = Command::new(&t.ffmpeg);
    c.args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size=640x360:rate={rate}"))
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            &seconds.to_string(),
        ]);
    match codec {
        "hevc" => c.args([
            "-c:v".to_string(),
            "libx265".into(),
            "-preset".into(),
            "ultrafast".into(),
            "-x265-params".into(),
            format!(
                "log-level=error:keyint=60:open-gop={}:bframes=3",
                open as u8
            ),
            "-tag:v".into(),
            "hvc1".into(),
        ]),
        _ => c.args([
            "-c:v".to_string(),
            "libx264".into(),
            "-preset".into(),
            "veryfast".into(),
            "-x264-params".into(),
            format!("open-gop={}:keyint=60", open as u8),
            "-bf".into(),
            "3".into(),
        ]),
    };
    let st = c
        .args(["-pix_fmt", "yuv420p", "-c:a", "aac"])
        .arg(path)
        .status()
        .unwrap();
    assert!(st.success());
}

fn frames_and_errors(t: &Tools, path: &Path) -> (u64, String) {
    let probe = Command::new(&t.ffprobe)
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .unwrap();
    let n = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    let dec = Command::new(&t.ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    (n, String::from_utf8_lossy(&dec.stderr).into_owned())
}

fn export(t: &Tools, req: &ExportRequest) -> (Vec<PathBuf>, JobDetails) {
    let cancel = AtomicBool::new(false);
    let job = Job {
        id: 1,
        tools: t,
        req,
        cancel: &cancel,
    };
    let mut details = JobDetails::default();
    let out = job
        .run(&mut |_| {}, &mut details)
        .unwrap_or_else(|e| panic!("{e}\n{}", details.log));
    (out.into_iter().map(|o| o.path).collect(), details)
}

fn seg(path: &Path, a: f64, b: f64, fps: f64) -> Segment {
    Segment {
        clip: 1,
        path: path.to_path_buf(),
        start: Timestamp::from_secs(a).snap_to_frame(fps),
        end: Timestamp::from_secs(b).snap_to_frame(fps),
    }
}

/// `may_convert`: open-GOP H.264 can't be spliced without picture-order
/// confusion, so the job is expected to notice and convert instead, still
/// frame-exact.
fn frame_exact_copy(codec: &str, open: bool, may_convert: bool) {
    let Some(t) = tools() else { return };
    let dir = scratch(&format!("{codec}-{open}"));
    let src = dir.join(format!("src-{codec}.mp4"));
    let fps = 30000.0 / 1001.0;
    make_clip_gop(&t, &src, codec, 12, "30000/1001", open);
    let parts = [(1.3, 6.1), (7.0, 11.4)];
    let req = ExportRequest {
        segments: parts.iter().map(|(a, b)| seg(&src, *a, *b, fps)).collect(),
        profile: Profile::original(),
        output_dir: dir.clone(),
        name: "out (cut)".into(),
        separate: false,
        audio_tracks: vec![0],
        overwrite: false,
    };
    let (outs, details) = export(&t, &req);
    let expected: i64 = req
        .segments
        .iter()
        .map(|s| ((s.end.secs() - s.start.secs()) * fps).round() as i64)
        .sum();
    let (n, errors) = frames_and_errors(&t, &outs[0]);
    assert_eq!(n as i64, expected, "frame count\n{}", details.log);
    assert!(errors.trim().is_empty(), "decode errors: {errors}");
    if !may_convert {
        assert!(
            details.notes.iter().all(|n| !n.contains("converted")),
            "fell back: {:?}\n{}",
            details.notes,
            details.log
        );
    }
    // Only the finished file is left behind.
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| !n.starts_with(".cutline")),
        "{names:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hevc_open_gop_copy_is_frame_exact() {
    frame_exact_copy("hevc", true, false);
}

#[test]
fn hevc_closed_gop_copy_is_frame_exact() {
    frame_exact_copy("hevc", false, false);
}

#[test]
fn h264_closed_gop_copy_is_frame_exact() {
    frame_exact_copy("h264", false, false);
}

#[test]
fn h264_open_gop_is_still_frame_exact() {
    frame_exact_copy("h264", true, true);
}

#[test]
fn fit_to_size_never_exceeds_target() {
    let Some(t) = tools() else { return };
    let dir = scratch("fit");
    let src = dir.join("src.mp4");
    make_clip(&t, &src, "hevc", 10, "30");
    // Targets well below what the source weighs, so the encode has to work for it.
    let src_mb = std::fs::metadata(&src).unwrap().len() as f64 / 1e6;
    for mb in [src_mb * 0.2, src_mb * 0.5] {
        let req = ExportRequest {
            segments: vec![seg(&src, 0.5, 9.5, 30.0)],
            profile: Profile::fit(mb),
            output_dir: dir.clone(),
            name: format!("fit {mb}"),
            separate: false,
            audio_tracks: vec![0],
            overwrite: false,
        };
        let (outs, details) = export(&t, &req);
        let size = std::fs::metadata(&outs[0]).unwrap().len() as f64;
        assert!(
            size <= mb * 1e6,
            "{mb} MB target came out {size} bytes\n{}",
            details.log
        );
        assert!(
            size >= mb * 1e6 * 0.6,
            "{mb} MB target wasted: {size} bytes"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refuses_to_overwrite_unless_asked() {
    let Some(t) = tools() else { return };
    let dir = scratch("overwrite");
    let src = dir.join("src.mp4");
    make_clip(&t, &src, "hevc", 4, "30");
    std::fs::write(dir.join("taken.mp4"), b"keep me").unwrap();
    let mut req = ExportRequest {
        segments: vec![seg(&src, 0.0, 3.0, 30.0)],
        profile: Profile::original(),
        output_dir: dir.clone(),
        name: "taken".into(),
        separate: false,
        audio_tracks: vec![0],
        overwrite: false,
    };
    let cancel = AtomicBool::new(false);
    let mut d = JobDetails::default();
    let err = Job {
        id: 2,
        tools: &t,
        req: &req,
        cancel: &cancel,
    }
    .run(&mut |_| {}, &mut d)
    .unwrap_err();
    assert!(err.to_string().contains("already exists"));
    assert_eq!(std::fs::read(dir.join("taken.mp4")).unwrap(), b"keep me");
    req.overwrite = true;
    export(&t, &req);
    assert_ne!(std::fs::read(dir.join("taken.mp4")).unwrap(), b"keep me");
    let _ = std::fs::remove_dir_all(&dir);
}
