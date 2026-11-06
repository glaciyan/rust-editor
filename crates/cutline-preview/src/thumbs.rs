//! Timeline thumbnails: one ffmpeg call per picture (nearest keyframe, so
//! it's fast), a few in parallel, cached on disk keyed by xxh3 of the file
//! identity and the request.

use crate::Frame;
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct ThumbRequest {
    pub clip: u64,
    pub path: PathBuf,
    pub index: usize,
    pub time: f64,
    pub width: usize,
    pub height: usize,
}

pub struct ThumbResult {
    pub clip: u64,
    pub index: usize,
    pub frame: Frame,
}

pub struct Thumbnailer {
    tx: Sender<ThumbRequest>,
    pub results: Receiver<ThumbResult>,
}

fn key(req: &ThumbRequest) -> String {
    let meta = std::fs::metadata(&req.path).ok();
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let size = meta.map(|m| m.len()).unwrap_or(0);
    let id = format!(
        "{}|{size}|{mtime}|{:.3}|{}x{}",
        req.path.display(),
        req.time,
        req.width,
        req.height
    );
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(id.as_bytes()))
}

/// Decoding only keyframes is fastest; some streams need a full decode near the end.
fn render(ffmpeg: &Path, req: &ThumbRequest) -> Option<Vec<u8>> {
    render_with(ffmpeg, req, true).or_else(|| render_with(ffmpeg, req, false))
}

fn render_with(ffmpeg: &Path, req: &ThumbRequest, keyframes_only: bool) -> Option<Vec<u8>> {
    let skip: &[&str] = if keyframes_only {
        &["-skip_frame", "nokey"]
    } else {
        &[]
    };
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error"])
        .args(skip)
        .args(["-noaccurate_seek", "-ss", &format!("{:.3}", req.time), "-i"])
        .arg(&req.path)
        .args([
            "-an",
            "-sn",
            "-frames:v",
            "1",
            "-vf",
            &format!(
                "scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}",
                w = req.width,
                h = req.height
            ),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let n = req.width * req.height * 4;
    (out.stdout.len() >= n).then(|| out.stdout[..n].to_vec())
}

impl Thumbnailer {
    pub fn new(ffmpeg: PathBuf, cache_dir: PathBuf, waker: crate::Waker) -> Thumbnailer {
        let (tx, rx) = unbounded::<ThumbRequest>();
        let (res_tx, results) = unbounded();
        let rx = Arc::new(Mutex::new(rx));
        let _ = std::fs::create_dir_all(&cache_dir);
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 4);
        for _ in 0..workers {
            let rx = rx.clone();
            let res_tx = res_tx.clone();
            let ffmpeg = ffmpeg.clone();
            let cache = cache_dir.clone();
            let waker = waker.clone();
            std::thread::spawn(move || loop {
                let req = match rx.lock().unwrap().recv() {
                    Ok(r) => r,
                    Err(_) => break,
                };
                let file = cache.join(format!("{}.rgba", key(&req)));
                let n = req.width * req.height * 4;
                let bytes = match std::fs::read(&file) {
                    Ok(b) if b.len() == n => Some(b),
                    _ => {
                        let b = render(&ffmpeg, &req);
                        if let Some(b) = &b {
                            let _ = std::fs::write(&file, b);
                        }
                        b
                    }
                };
                if let Some(rgba) = bytes {
                    let _ = res_tx.send(ThumbResult {
                        clip: req.clip,
                        index: req.index,
                        frame: Frame {
                            width: req.width,
                            height: req.height,
                            rgba,
                        },
                    });
                    waker();
                }
            });
        }
        Thumbnailer { tx, results }
    }

    pub fn request(&self, r: ThumbRequest) {
        let _ = self.tx.send(r);
    }
}
