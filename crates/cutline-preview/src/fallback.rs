//! Preview without libmpv: decode frames with ffmpeg into raw RGBA. No sound.

use crate::{fit_size, Frame, Waker};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender, TryRecvError};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

const PREVIEW_FPS: f64 = 25.0;

struct Stream {
    child: Child,
    rx: Receiver<Frame>,
    t0: f64,
    started: Instant,
    shown: u64,
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct FfmpegPlayer {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    waker: Waker,
    path: Option<PathBuf>,
    display: (usize, usize),
    size: (usize, usize),
    paused: bool,
    pos: f64,
    stream: Option<Stream>,
    still_tx: Sender<(PathBuf, f64, usize, usize)>,
    still_rx: Receiver<(f64, Frame)>,
    ended: bool,
}

fn decode_cmd(
    ffmpeg: &Path,
    path: &Path,
    t: f64,
    w: usize,
    h: usize,
    frames: Option<u32>,
) -> Command {
    let mut c = Command::new(ffmpeg);
    c.args([
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-ss",
        &format!("{t:.6}"),
        "-i",
    ])
    .arg(path)
    .args([
        "-an",
        "-sn",
        "-vf",
        &format!("fps={PREVIEW_FPS},scale={w}:{h}"),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
    ]);
    if let Some(n) = frames {
        c.args(["-frames:v", &n.to_string()]);
    }
    c.arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    c
}

impl FfmpegPlayer {
    pub fn new(ffmpeg: PathBuf, waker: Waker) -> FfmpegPlayer {
        let ffprobe = ffmpeg.with_file_name("ffprobe");
        let (still_tx, req_rx) = unbounded::<(PathBuf, f64, usize, usize)>();
        let (res_tx, still_rx) = unbounded();
        let ff = ffmpeg.clone();
        let wk = waker.clone();
        std::thread::spawn(move || {
            while let Ok(mut req) = req_rx.recv() {
                // Only the newest request matters while scrubbing.
                while let Ok(r) = req_rx.try_recv() {
                    req = r;
                }
                let (path, t, w, h) = req;
                if let Ok(out) = decode_cmd(&ff, &path, t, w, h, Some(1)).output() {
                    if out.stdout.len() >= w * h * 4 {
                        let _ = res_tx.send((
                            t,
                            Frame {
                                width: w,
                                height: h,
                                rgba: out.stdout[..w * h * 4].to_vec(),
                            },
                        ));
                        wk();
                    }
                }
            }
        });
        FfmpegPlayer {
            ffmpeg,
            ffprobe,
            waker,
            path: None,
            display: (16, 9),
            size: (640, 360),
            paused: true,
            pos: 0.0,
            stream: None,
            still_tx,
            still_rx,
            ended: false,
        }
    }

    fn probe_display(&self, path: &Path) -> (usize, usize) {
        let tools = cutline_ffmpeg::Tools {
            ffmpeg: self.ffmpeg.clone(),
            ffprobe: self.ffprobe.clone(),
            caps: Default::default(),
        };
        cutline_ffmpeg::probe::probe(&tools, path)
            .ok()
            .and_then(|m| m.video)
            .map(|v| {
                let (w, h) = v.display_size();
                (w as usize, h as usize)
            })
            .unwrap_or((1280, 720))
    }

    pub fn open(&mut self, path: &Path, start: f64, play: bool) {
        self.display = self.probe_display(path);
        self.path = Some(path.to_path_buf());
        self.pos = start;
        self.ended = false;
        self.stream = None;
        self.paused = !play;
        if play {
            self.start_stream();
        } else {
            self.request_still();
        }
    }

    fn request_still(&self) {
        if let Some(p) = &self.path {
            let _ = self
                .still_tx
                .send((p.clone(), self.pos, self.size.0, self.size.1));
        }
    }

    fn start_stream(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let (w, h) = self.size;
        let Ok(mut child) = decode_cmd(&self.ffmpeg, &path, self.pos, w, h, None).spawn() else {
            return;
        };
        let mut out = child.stdout.take().expect("piped");
        let (tx, rx) = bounded::<Frame>(4);
        let waker = self.waker.clone();
        std::thread::spawn(move || loop {
            let mut buf = vec![0u8; w * h * 4];
            if out.read_exact(&mut buf).is_err() {
                waker();
                break;
            }
            if tx
                .send(Frame {
                    width: w,
                    height: h,
                    rgba: buf,
                })
                .is_err()
            {
                break;
            }
            waker();
        });
        self.stream = Some(Stream {
            child,
            rx,
            t0: self.pos,
            started: Instant::now(),
            shown: 0,
        });
    }

    pub fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        if paused {
            self.stream = None;
        } else {
            if self.ended {
                self.pos = 0.0;
                self.ended = false;
            }
            self.start_stream();
        }
    }
    pub fn paused(&self) -> bool {
        self.paused
    }
    pub fn seek(&mut self, t: f64) {
        self.pos = t.max(0.0);
        self.ended = false;
        if self.paused {
            self.request_still();
        } else {
            self.stream = None;
            self.start_stream();
        }
    }
    pub fn position(&self) -> f64 {
        self.pos
    }
    pub fn at_end(&self) -> bool {
        self.ended
    }

    pub fn update(&mut self, max_w: usize, max_h: usize) -> Option<Frame> {
        let want = fit_size(
            self.display.0,
            self.display.1,
            max_w.clamp(64, 1280),
            max_h.clamp(64, 720),
        );
        if want != self.size {
            self.size = want;
            if self.paused {
                self.request_still();
            }
        }
        let mut latest = None;
        while let Ok((t, f)) = self.still_rx.try_recv() {
            if self.paused {
                self.pos = t;
                latest = Some(f);
            }
        }
        if let Some(s) = &mut self.stream {
            let due = (s.started.elapsed().as_secs_f64() * PREVIEW_FPS) as u64;
            while s.shown <= due {
                match s.rx.try_recv() {
                    Ok(f) => {
                        s.shown += 1;
                        latest = Some(f);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.ended = true;
                        self.paused = true;
                        break;
                    }
                }
            }
            self.pos = s.t0 + s.shown as f64 / PREVIEW_FPS;
            if self.ended {
                self.stream = None;
            } else {
                // Keep the UI ticking at frame rate.
                let w = self.waker.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(
                        (1000.0 / PREVIEW_FPS) as u64,
                    ));
                    w();
                });
            }
        }
        latest
    }

    pub fn close(&mut self) {
        self.stream = None;
        self.path = None;
    }
}
