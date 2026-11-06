//! The main window: one big preview, one timeline, one Export button.

use crate::dialogs::{AdvancedEditor, DetailsWindow};
use crate::export::{ExportDialog, ExportOutcome};
use crate::theme::{self, INK, MUTED};
use crate::timeline::{self, TimelineAction, TimelineView};
use cutline_core::{format, PartMode, Project, Timestamp};
use cutline_engine::estimate::Calibration;
use cutline_engine::job::{JobDetails, JobId, JobOutput};
use cutline_engine::store::{self, ProfileStore, Session};
use cutline_engine::{Engine, EngineEvent};
use cutline_ffmpeg::{probe, MediaInfo, Tools};
use cutline_preview::thumbs::{ThumbRequest, Thumbnailer};
use cutline_preview::Player;
use eframe::egui::{self, Color32, Key, Modifiers, Stroke};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const VIDEO_EXTS: [&str; 10] = [
    "mp4", "mkv", "mov", "m4v", "ts", "m2ts", "mts", "hevc", "webm", "avi",
];

pub enum JobState {
    Waiting,
    Running {
        fraction: f64,
        eta: Option<f64>,
        stage: String,
    },
    Done {
        outputs: Vec<JobOutput>,
        seconds: f64,
    },
    Failed {
        message: String,
    },
    Canceled,
}

pub struct JobView {
    pub id: JobId,
    pub outputs: Vec<PathBuf>,
    pub state: JobState,
    pub details: JobDetails,
    pub dismissed: bool,
}

enum Probed {
    Ok {
        path: PathBuf,
        info: Box<MediaInfo>,
        restore: bool,
    },
    Err {
        path: PathBuf,
        message: String,
    },
}

pub struct Toast {
    pub text: String,
    pub until: Instant,
}

pub struct CutlineApp {
    pub tools: Option<Arc<Tools>>,
    tools_error: Option<String>,
    pub engine: Option<Engine>,
    pub project: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
    pub sources: HashMap<PathBuf, MediaInfo>,
    probe_tx: Sender<Probed>,
    probe_rx: Receiver<Probed>,
    /// Files waiting for their probe, in the order they were added.
    pending: Vec<PathBuf>,
    player: Option<Player>,
    /// Which clip the player has loaded.
    loaded: Option<u64>,
    preview: Option<egui::TextureHandle>,
    preview_size: [usize; 2],
    /// Playhead: clip and time inside it.
    pub playhead: Option<(u64, Timestamp)>,
    was_in_kept: bool,
    /// Handle drag in progress: remember whether to resume playing.
    resume_after_drag: Option<bool>,
    thumbs: Option<Thumbnailer>,
    pub thumb_tex: HashMap<(u64, usize, usize), egui::TextureHandle>,
    thumb_requested: HashSet<(u64, usize)>,
    pub timeline: TimelineView,
    export: Option<ExportDialog>,
    advanced: Option<AdvancedEditor>,
    details: Option<DetailsWindow>,
    pub jobs: Vec<JobView>,
    pub profiles: ProfileStore,
    pub calibrations: HashMap<u64, Option<Calibration>>,
    calibrating: HashSet<u64>,
    pub last_export_dir: Option<PathBuf>,
    pub last_fit_mb: Option<f64>,
    toast: Option<Toast>,
    saved_session: Option<String>,
    last_session_check: Instant,
    file_dialog_rx: Option<(Receiver<Option<Vec<PathBuf>>>, Instant)>,
    path_prompt: Option<String>,
    confirm_quit: bool,
    /// Height of everything under the preview, measured last frame.
    below_h: f32,
    title: String,
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode_path(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Open the file manager with the file selected; fall back to opening the folder.
pub fn show_in_folder(path: &Path) {
    let uri = percent_encode_path(path);
    let ok = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("array:string:{uri}"),
            "string:",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        if let Some(dir) = path.parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
}

impl CutlineApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Vec<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let waker: cutline_preview::Waker = Arc::new(move || ctx.request_repaint());
        let (tools, tools_error) = match Tools::locate() {
            Ok(t) => (Some(Arc::new(t)), None),
            Err(e) => (None, Some(e)),
        };
        let engine = tools.as_ref().map(|t| {
            let e = Engine::new((**t).clone());
            let ctx = cc.egui_ctx.clone();
            e.set_waker(move || ctx.request_repaint());
            e
        });
        let player = tools
            .as_ref()
            .map(|t| Player::new(&t.ffmpeg, waker.clone()));
        let thumbs = tools.as_ref().map(|t| {
            Thumbnailer::new(
                t.ffmpeg.clone(),
                store::cache_dir().join("thumbs"),
                waker.clone(),
            )
        });
        let (probe_tx, probe_rx) = channel();
        let mut app = CutlineApp {
            tools,
            tools_error,
            engine,
            project: Project::default(),
            undo: vec![],
            redo: vec![],
            sources: HashMap::new(),
            probe_tx,
            probe_rx,
            pending: vec![],
            player,
            loaded: None,
            preview: None,
            preview_size: [0, 0],
            playhead: None,
            was_in_kept: false,
            resume_after_drag: None,
            thumbs,
            thumb_tex: HashMap::new(),
            thumb_requested: HashSet::new(),
            timeline: TimelineView::default(),
            export: None,
            advanced: None,
            details: None,
            jobs: vec![],
            profiles: ProfileStore::load(),
            calibrations: HashMap::new(),
            calibrating: HashSet::new(),
            last_export_dir: None,
            last_fit_mb: None,
            toast: None,
            saved_session: None,
            last_session_check: Instant::now(),
            file_dialog_rx: None,
            path_prompt: None,
            confirm_quit: false,
            below_h: 260.0,
            title: String::new(),
        };
        if args.is_empty() {
            app.restore_session();
        } else {
            app.add_files(args);
        }
        app
    }

    // ---------- files & session ----------

    fn restore_session(&mut self) {
        let Some(s) = store::load_session() else {
            return;
        };
        self.last_export_dir = s.last_export_dir.clone();
        self.saved_session = serde_json::to_string(&s).ok();
        if s.project.is_empty() {
            return;
        }
        self.project = s.project;
        self.playhead = s.playhead.map(|(c, t)| (c, Timestamp::from_secs(t)));
        let paths: Vec<PathBuf> = self.project.clips.iter().map(|c| c.path.clone()).collect();
        for p in paths {
            self.spawn_probe(p, true);
        }
    }

    fn session(&self) -> Session {
        Session {
            project: self.project.clone(),
            playhead: self.playhead.map(|(c, t)| (c, t.secs())),
            last_export_dir: self.last_export_dir.clone(),
        }
    }

    fn save_session_if_changed(&mut self, force: bool) {
        if !force && self.last_session_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_session_check = Instant::now();
        let s = self.session();
        let json = serde_json::to_string(&s).ok();
        if json != self.saved_session && store::save_session(&s).is_ok() {
            self.saved_session = json;
        }
    }

    fn spawn_probe(&mut self, path: PathBuf, restore: bool) {
        let Some(tools) = self.tools.clone() else {
            return;
        };
        let tx = self.probe_tx.clone();
        if !restore {
            self.pending.push(path.clone());
        }
        std::thread::spawn(move || {
            let r = match probe::probe(&tools, &path) {
                Ok(info) if info.video.is_some() => Probed::Ok {
                    path,
                    info: Box::new(info),
                    restore,
                },
                Ok(_) => Probed::Err {
                    path,
                    message: "has no picture".into(),
                },
                Err(e) => Probed::Err {
                    path,
                    message: e.to_string(),
                },
            };
            let _ = tx.send(r);
        });
    }

    /// Add videos in the given order.
    pub fn add_files(&mut self, files: Vec<PathBuf>) {
        for f in files.into_iter().filter(|p| p.is_file()) {
            self.spawn_probe(f, false);
        }
    }

    fn handle_probes(&mut self) {
        while let Ok(p) = self.probe_rx.try_recv() {
            match p {
                Probed::Ok {
                    path,
                    info,
                    restore,
                } => {
                    if let Some(v) = &info.video {
                        if v.codec != "hevc" && v.codec != "h264" && !restore {
                            self.toast(format!(
                                "{} isn't an H.265 video. Cuts in it land on the nearest keyframe.",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ));
                        }
                    }
                    self.sources.insert(path.clone(), *info);
                    if restore {
                        // Session restore: the clip is already in the project.
                        if self.loaded.is_none()
                            && self
                                .project
                                .clips
                                .first()
                                .map(|c| c.path == path)
                                .unwrap_or(false)
                        {
                            let (clip, t) = self
                                .playhead
                                .unwrap_or((self.project.clips[0].id, Timestamp::ZERO));
                            let clip = if self.project.clip(clip).is_some() {
                                clip
                            } else {
                                self.project.clips[0].id
                            };
                            self.load_clip(clip, t, false);
                        } else if let Some((pc, t)) = self.playhead {
                            if self.loaded.is_none()
                                && self
                                    .project
                                    .clip(pc)
                                    .map(|c| c.path == path)
                                    .unwrap_or(false)
                            {
                                self.load_clip(pc, t, false);
                            }
                        }
                    }
                }
                Probed::Err { path, message } => {
                    self.pending.retain(|p| p != &path);
                    if self.project.clips.iter().any(|c| c.path == path) {
                        self.toast(format!(
                            "{} can't be read any more.",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    } else {
                        self.toast(format!(
                            "Couldn't open {}: {message}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                }
            }
        }
        // Add probed files to the timeline in the order they were dropped.
        while let Some(first) = self.pending.first().cloned() {
            let Some(info) = self.sources.get(&first) else {
                break;
            };
            self.pending.remove(0);
            let fps = info.video.as_ref().map(|v| v.fps).unwrap_or(0.0);
            let dur = Timestamp::from_secs(info.duration);
            self.checkpoint();
            let was_empty = self.project.is_empty();
            let id = self.project.add_clip(first.clone(), dur, fps);
            if was_empty {
                self.load_clip(id, Timestamp::ZERO, true);
            }
        }
    }

    pub fn toast(&mut self, text: String) {
        self.toast = Some(Toast {
            text,
            until: Instant::now() + Duration::from_secs(6),
        });
    }

    pub fn open_dialog(&mut self) {
        if self.file_dialog_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let dir = self
            .project
            .clips
            .last()
            .and_then(|c| c.path.parent().map(Path::to_path_buf));
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new()
                .set_title("Open videos")
                .add_filter("Videos", &VIDEO_EXTS)
                .add_filter("All files", &["*"]);
            if let Some(dir) = dir {
                d = d.set_directory(dir);
            }
            let _ = tx.send(d.pick_files());
        });
        self.file_dialog_rx = Some((rx, Instant::now()));
    }

    fn poll_file_dialog(&mut self) {
        if let Some((rx, started)) = &self.file_dialog_rx {
            if let Ok(r) = rx.try_recv() {
                let quick = started.elapsed() < Duration::from_millis(300);
                self.file_dialog_rx = None;
                match r {
                    Some(files) => self.add_files(files),
                    // No file chooser on this system: ask for a path instead.
                    None if quick => self.path_prompt = Some(String::new()),
                    None => {}
                }
            }
        }
    }

    // ---------- undo ----------

    pub fn checkpoint(&mut self) {
        self.undo.push(self.project.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    fn undo(&mut self) {
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project, p));
            self.after_project_swap();
        }
    }
    fn redo(&mut self) {
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project, p));
            self.after_project_swap();
        }
    }
    fn after_project_swap(&mut self) {
        if let Some(l) = self.loaded {
            if self.project.clip(l).is_none() {
                self.loaded = None;
                self.playhead = None;
                if let Some(c) = self.project.clips.first().map(|c| c.id) {
                    self.load_clip(c, Timestamp::ZERO, false);
                } else if let Some(p) = &mut self.player {
                    p.close();
                    self.preview = None;
                }
            }
        }
    }

    // ---------- playback ----------

    pub fn playing(&self) -> bool {
        self.loaded.is_some() && self.player.as_ref().map(|p| !p.paused()).unwrap_or(false)
    }

    fn load_clip(&mut self, clip: u64, t: Timestamp, play: bool) {
        let Some(c) = self.project.clip(clip) else {
            return;
        };
        let path = c.path.clone();
        if let Some(p) = &mut self.player {
            p.open(&path, t.secs(), play);
        }
        self.loaded = Some(clip);
        self.playhead = Some((clip, t));
        self.was_in_kept = self.project.is_kept(clip, t);
    }

    /// Move the playhead (and the preview) to a clip-local time.
    pub fn seek(&mut self, clip: u64, t: Timestamp) {
        if self.loaded != Some(clip) {
            let play = self.playing();
            self.load_clip(clip, t, play);
            return;
        }
        if let Some(p) = &mut self.player {
            p.seek(t.secs());
        }
        self.playhead = Some((clip, t));
        self.was_in_kept = self.project.is_kept(clip, t);
    }

    pub fn toggle_play(&mut self) {
        let Some(p) = &mut self.player else { return };
        if self.loaded.is_none() {
            if let Some(c) = self.project.clips.first().map(|c| c.id) {
                self.load_clip(c, Timestamp::ZERO, true);
            }
            return;
        }
        let paused = p.paused();
        if paused && p.at_end() {
            // Played to the end: start over from the first kept part.
            if let Some(first) = self.project.clips.first().map(|c| c.id) {
                let (c, t) = self
                    .project
                    .next_kept(first, Timestamp::ZERO)
                    .unwrap_or((first, Timestamp::ZERO));
                self.load_clip(c, t, true);
                return;
            }
        }
        p.set_paused(!paused);
    }

    /// Follow the player; skip what's cut out while playing.
    fn tick_playback(&mut self) {
        let Some(clip) = self.loaded else { return };
        let Some(p) = &self.player else { return };
        if self.resume_after_drag.is_some() {
            return;
        }
        let playing = !p.paused();
        let at_end = p.at_end();
        let pos = Timestamp::from_secs(p.position());
        if playing || at_end {
            self.playhead = Some((clip, pos));
        }
        let kept = self.project.is_kept(clip, pos);
        if playing && self.was_in_kept && !kept {
            // Left a kept part: jump to the next one.
            match self.project.next_kept(clip, pos) {
                Some((c, t)) if c == clip => self.seek(c, t),
                Some((c, t)) => self.load_clip(c, t, true),
                None => {
                    if let Some(p) = &mut self.player {
                        p.set_paused(true);
                    }
                }
            }
            return;
        }
        if at_end && self.was_in_kept {
            let idx = self.project.clip_index(clip).unwrap_or(0);
            if let Some(next) = self.project.clips.get(idx + 1).map(|c| c.id) {
                let (c, t) = self
                    .project
                    .next_kept(next, Timestamp::ZERO)
                    .unwrap_or((next, Timestamp::ZERO));
                self.load_clip(c, t, true);
                return;
            }
        }
        self.was_in_kept = kept;
    }

    fn frame_step(&mut self, frames: f64) {
        let Some((clip, t)) = self.playhead else {
            return;
        };
        let fps = self
            .project
            .clip(clip)
            .map(|c| if c.fps > 0.0 { c.fps } else { 30.0 })
            .unwrap_or(30.0);
        if let Some(p) = &mut self.player {
            p.set_paused(true);
        }
        let dur = self
            .project
            .clip(clip)
            .map(|c| c.duration)
            .unwrap_or_default();
        let nt = Timestamp::from_secs(t.secs() + frames / fps)
            .snap_to_frame(fps)
            .clamp(Timestamp::ZERO, dur);
        self.seek(clip, nt);
    }

    // ---------- export ----------

    pub fn open_export(&mut self) {
        if self.project.is_empty() || self.tools.is_none() {
            return;
        }
        if self.project.segments().is_empty() {
            self.toast("Everything is cut out. Keep at least one part to export.".into());
            return;
        }
        if let Some(p) = &mut self.player {
            p.set_paused(true);
        }
        self.export = Some(ExportDialog::new(self));
    }

    pub fn request_calibration(&mut self, key: u64, req: cutline_engine::ExportRequest) {
        if self.calibrations.contains_key(&key) || self.calibrating.contains(&key) {
            return;
        }
        if let Some(e) = &self.engine {
            // One sample at a time; the newest request wins.
            self.calibrating.clear();
            self.calibrating.insert(key);
            let srcs = req
                .segments
                .iter()
                .filter_map(|s| {
                    self.sources
                        .get(&s.path)
                        .map(|m| (s.path.clone(), m.clone()))
                })
                .collect();
            e.calibrate(key, req, srcs);
        }
    }

    fn handle_engine(&mut self) {
        let Some(e) = &self.engine else { return };
        for ev in e.poll() {
            match ev {
                EngineEvent::Queued { job, outputs } => {
                    self.jobs.push(JobView {
                        id: job,
                        outputs,
                        state: JobState::Waiting,
                        details: JobDetails::default(),
                        dismissed: false,
                    });
                }
                EngineEvent::Started { job } => self.with_job(job, |j| {
                    j.state = JobState::Running {
                        fraction: 0.0,
                        eta: None,
                        stage: "Saving".into(),
                    }
                }),
                EngineEvent::Progress {
                    job,
                    fraction,
                    eta_secs,
                    stage,
                } => {
                    self.with_job(job, |j| {
                        j.state = JobState::Running {
                            fraction,
                            eta: eta_secs,
                            stage,
                        }
                    });
                }
                EngineEvent::Finished {
                    job,
                    outputs,
                    details,
                    seconds,
                } => self.with_job(job, |j| {
                    j.state = JobState::Done { outputs, seconds };
                    j.details = details;
                }),
                EngineEvent::Failed {
                    job,
                    message,
                    details,
                } => self.with_job(job, |j| {
                    j.state = JobState::Failed { message };
                    j.details = details;
                }),
                EngineEvent::Canceled { job } => {
                    self.with_job(job, |j| j.state = JobState::Canceled)
                }
                EngineEvent::Calibrated { key, calibration } => {
                    self.calibrating.remove(&key);
                    self.calibrations.insert(key, calibration);
                }
            }
        }
    }

    fn with_job(&mut self, id: JobId, f: impl FnOnce(&mut JobView)) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            f(j);
        }
    }

    // ---------- thumbnails ----------

    fn pump_thumbnails(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.thumbs else { return };
        while let Ok(r) = t.results.try_recv() {
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [r.frame.width, r.frame.height],
                &r.frame.rgba,
            );
            let tex = ctx.load_texture(
                format!("thumb-{}-{}", r.clip, r.index),
                img,
                egui::TextureOptions::LINEAR,
            );
            self.thumb_tex
                .insert((r.clip, r.index / 1000, r.index % 1000), tex);
        }
    }

    /// Ask for `count` thumbnails across a clip (cached per count).
    pub fn want_thumbs(&mut self, clip: u64, count: usize, tile_h: usize) {
        if self.thumb_requested.contains(&(clip, count)) {
            return;
        }
        let Some(c) = self.project.clip(clip) else {
            return;
        };
        let Some(info) = self.sources.get(&c.path) else {
            return;
        };
        let Some(v) = &info.video else { return };
        let Some(t) = &self.thumbs else { return };
        let (dw, dh) = v.display_size();
        let h = tile_h.max(16);
        let w = ((h as f64 * dw as f64 / dh.max(1) as f64).round() as usize).clamp(16, 400) / 2 * 2;
        self.thumb_requested.insert((clip, count));
        let dur = c.duration.secs();
        // Middle tiles first feels faster than left-to-right.
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by_key(|i| (*i as i64 * 2 - count as i64).abs());
        for i in order {
            t.request(ThumbRequest {
                clip,
                path: c.path.clone(),
                index: count * 1000 + i,
                time: (i as f64 + 0.5) * dur / count as f64,
                width: w,
                height: h,
            });
        }
    }

    // ---------- keyboard ----------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let modal = self.export.is_some() || self.path_prompt.is_some();
        let mut actions = vec![];
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::E) {
                actions.push("export");
            }
            if i.consume_key(Modifiers::COMMAND, Key::O) {
                actions.push("open");
            }
            if typing || modal {
                return;
            }
            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y)
            {
                actions.push("redo");
            }
            if i.consume_key(Modifiers::COMMAND, Key::Z) {
                actions.push("undo");
            }
            if i.consume_key(Modifiers::NONE, Key::Space) {
                actions.push("play");
            }
            if i.consume_key(Modifiers::NONE, Key::I) {
                actions.push("in");
            }
            if i.consume_key(Modifiers::NONE, Key::O) {
                actions.push("out");
            }
            if i.consume_key(Modifiers::NONE, Key::Delete)
                || i.consume_key(Modifiers::NONE, Key::Backspace)
            {
                actions.push("delete");
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                actions.push("prev-frame");
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                actions.push("next-frame");
            }
            if i.consume_key(Modifiers::SHIFT, Key::ArrowLeft) {
                actions.push("back-1s");
            }
            if i.consume_key(Modifiers::SHIFT, Key::ArrowRight) {
                actions.push("fwd-1s");
            }
            if i.consume_key(Modifiers::NONE, Key::Home) {
                actions.push("home");
            }
            if i.consume_key(Modifiers::NONE, Key::End) {
                actions.push("end");
            }
            if i.consume_key(Modifiers::NONE, Key::A) {
                actions.push("add");
            }
        });
        for a in actions {
            match a {
                "export" => self.open_export(),
                "open" => self.open_dialog(),
                "undo" => self.undo(),
                "redo" => self.redo(),
                "play" => self.toggle_play(),
                "in" | "out" => {
                    if let Some((clip, t)) = self.playhead {
                        self.checkpoint();
                        if a == "in" {
                            self.project.set_in(clip, t);
                        } else {
                            self.project.set_out(clip, t);
                        }
                        self.was_in_kept = self.project.is_kept(clip, t);
                    }
                }
                "delete" => {
                    if let Some(s) = self.project.selected {
                        self.checkpoint();
                        self.project.remove_part(s);
                    }
                }
                "prev-frame" => self.frame_step(-1.0),
                "next-frame" => self.frame_step(1.0),
                "back-1s" | "fwd-1s" => {
                    if let Some((clip, t)) = self.playhead {
                        let dur = self
                            .project
                            .clip(clip)
                            .map(|c| c.duration)
                            .unwrap_or_default();
                        let d = if a == "fwd-1s" { 1.0 } else { -1.0 };
                        self.seek(
                            clip,
                            Timestamp::from_secs(t.secs() + d).clamp(Timestamp::ZERO, dur),
                        );
                    }
                }
                "home" => {
                    if let Some(c) = self.project.clips.first().map(|c| c.id) {
                        self.seek(c, Timestamp::ZERO);
                    }
                }
                "end" => {
                    if let Some(c) = self.project.clips.last() {
                        let (id, d) = (c.id, c.duration);
                        self.seek(id, d);
                    }
                }
                "add" => self.add_part_at_playhead(),
                _ => {}
            }
        }
    }

    pub fn add_part_at_playhead(&mut self) {
        let Some((clip, t)) = self
            .playhead
            .or_else(|| self.project.clips.first().map(|c| (c.id, Timestamp::ZERO)))
        else {
            return;
        };
        self.checkpoint();
        if self.project.add_part(clip, t).is_none() {
            self.undo.pop();
            self.toast("There's no room for another part in this video.".into());
        }
    }

    // ---------- drawing ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(48.0);
            if ui
                .add(theme::quiet("Open"))
                .on_hover_text("Open videos (Ctrl+O)")
                .clicked()
            {
                self.open_dialog();
            }
            let title = match self.project.clips.len() {
                0 => String::new(),
                1 => self.project.clips[0].name(),
                n => format!("{} videos", n),
            };
            let export_w = 110.0;
            let avail = ui.available_width() - export_w - 16.0;
            ui.allocate_ui_with_layout(
                egui::vec2(avail.max(0.0), 40.0),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    ui.label(egui::RichText::new(title).color(MUTED));
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = !self.project.is_empty() && self.tools.is_some();
                if ui
                    .add_enabled(enabled, theme::primary("Export"))
                    .on_hover_text("Ctrl+E")
                    .clicked()
                {
                    self.open_export();
                }
            });
        });
    }

    fn preview_area(&mut self, ui: &mut egui::Ui, height: f32) {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 8, Color32::from_rgb(0x16, 0x16, 0x14));
        if self.project.is_empty() && self.pending.is_empty() {
            painter.rect_filled(rect, 8, theme::WELL);
            painter.rect_stroke(
                rect,
                8,
                Stroke::new(1.0, theme::BORDER),
                egui::StrokeKind::Inside,
            );
            let c = rect.center();
            painter.text(
                c - egui::vec2(0.0, 28.0),
                egui::Align2::CENTER_CENTER,
                "Drop videos here",
                egui::FontId::proportional(20.0),
                INK,
            );
            painter.text(
                c + egui::vec2(0.0, 2.0),
                egui::Align2::CENTER_CENTER,
                "or press Ctrl+O to open",
                egui::FontId::proportional(14.0),
                MUTED,
            );
            let b =
                egui::Rect::from_center_size(c + egui::vec2(0.0, 44.0), egui::vec2(120.0, 40.0));
            // A child ui so the button doesn't push the rest of the layout down.
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(b));
            if child.add_sized(b.size(), theme::primary("Open…")).clicked() {
                self.open_dialog();
            }
            return;
        }
        if self.project.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Opening…",
                egui::FontId::proportional(16.0),
                Color32::from_gray(200),
            );
            return;
        }
        // Render the next frame at the on-screen pixel size.
        let ppp = ui.ctx().pixels_per_point();
        let (mw, mh) = (
            (rect.width() * ppp) as usize,
            (rect.height() * ppp) as usize,
        );
        if let Some(p) = &mut self.player {
            if let Some(f) = p.update(mw.min(1920), mh.min(1080)) {
                let img = egui::ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba);
                self.preview_size = [f.width, f.height];
                match &mut self.preview {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => {
                        self.preview = Some(ui.ctx().load_texture(
                            "preview",
                            img,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                }
            }
        }
        if let Some(t) = &self.preview {
            let [w, h] = self.preview_size;
            let s = (rect.width() / w as f32).min(rect.height() / h as f32);
            let r =
                egui::Rect::from_center_size(rect.center(), egui::vec2(w as f32 * s, h as f32 * s));
            painter.image(
                t.id(),
                r,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        // Show which part of the video is cut out right now.
        if let Some((clip, t)) = self.playhead {
            if !self.project.is_kept(clip, t) {
                let badge = egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(12.0, 12.0),
                    egui::vec2(150.0, 26.0),
                );
                painter.rect_filled(badge, 13, Color32::from_black_alpha(170));
                painter.text(
                    badge.center(),
                    egui::Align2::CENTER_CENTER,
                    "Cut out of the export",
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
        }
        if resp.clicked() {
            self.toggle_play();
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.playing();
            let (r, resp) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::click());
            let hovered = resp.hovered();
            let p = ui.painter();
            p.circle_filled(
                r.center(),
                21.5,
                if hovered { theme::WELL } else { theme::SURFACE },
            );
            p.circle_stroke(r.center(), 21.5, Stroke::new(1.0, theme::BORDER));
            let c = r.center();
            if playing {
                for dx in [-4.0, 4.0] {
                    p.rect_filled(
                        egui::Rect::from_center_size(
                            c + egui::vec2(dx, 0.0),
                            egui::vec2(4.0, 13.0),
                        ),
                        1,
                        INK,
                    );
                }
            } else {
                p.add(egui::Shape::convex_polygon(
                    vec![
                        c + egui::vec2(-4.0, -7.0),
                        c + egui::vec2(7.0, 0.0),
                        c + egui::vec2(-4.0, 7.0),
                    ],
                    INK,
                    Stroke::NONE,
                ));
            }
            let resp = resp.on_hover_text(if playing {
                "Pause (Space)"
            } else {
                "Play (Space)"
            });
            if resp.clicked() {
                self.toggle_play();
            }
            let now = self
                .playhead
                .map(|(c, t)| self.project.clip_offset(c) + t)
                .unwrap_or_default();
            let total = self.project.total_duration();
            ui.label(
                egui::RichText::new(format!(
                    "{} / {}",
                    format::clock(now.secs()),
                    format::clock(total.secs())
                ))
                .monospace()
                .size(14.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(theme::small("Space play · I / O set start and end"));
                if let Some(p) = &self.player {
                    if !p.has_sound() {
                        ui.label(theme::small(
                            "Preview has no sound (install mpv for sound) ·",
                        ));
                    }
                }
            });
        });
    }

    fn part_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(theme::quiet("+ Add part"))
                .on_hover_text("Add a part at the playhead (A)")
                .clicked()
            {
                self.add_part_at_playhead();
            }
            ui.add_space(4.0);
            let keep = self.project.mode == PartMode::Keep;
            if theme::pill(ui, keep, "Keep selected")
                .on_hover_text("The marked parts are what you export")
                .clicked()
                && !keep
            {
                self.checkpoint();
                self.project.set_mode(PartMode::Keep);
            }
            if theme::pill(ui, !keep, "Remove selected")
                .on_hover_text("The marked parts are cut out")
                .clicked()
                && keep
            {
                self.checkpoint();
                self.project.set_mode(PartMode::Remove);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let segs = self.project.segments();
                let total: f64 = segs.iter().map(|s| s.len().secs()).sum();
                if self.project.is_empty() {
                    return;
                }
                ui.label(theme::small(if segs.is_empty() {
                    "Everything is cut out".to_string()
                } else {
                    format!(
                        "drag the handles to trim · export is {}",
                        format::length(total)
                    )
                }));
            });
        });
    }

    fn job_bar(&mut self, ui: &mut egui::Ui) {
        let waiting = self
            .jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Waiting))
            .count();
        let Some(idx) = self
            .jobs
            .iter()
            .rposition(|j| matches!(j.state, JobState::Running { .. }))
            .or_else(|| {
                self.jobs
                    .iter()
                    .rposition(|j| !j.dismissed && !matches!(j.state, JobState::Waiting))
            })
        else {
            return;
        };
        let mut cancel = None;
        let mut dismiss = false;
        let mut open_details = None;
        let mut show = None;
        let job = &self.jobs[idx];
        let name = |p: &PathBuf| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        egui::Frame::new()
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(8)
            .inner_margin(egui::Margin {
                left: 16,
                right: 8,
                top: 4,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.set_min_height(44.0);
                ui.horizontal(|ui| {
                    ui.set_min_height(44.0);
                    match &job.state {
                        JobState::Running {
                            fraction,
                            eta,
                            stage,
                        } => {
                            let mut text = format!("{stage} · {:.0}%", fraction * 100.0);
                            if let Some(e) = eta {
                                text.push_str(&format!(" · {}", format::time_left(*e)));
                            }
                            ui.label(text);
                            ui.label(theme::small(
                                job.outputs.first().map(name).unwrap_or_default(),
                            ));
                            ui.add(
                                egui::ProgressBar::new(*fraction as f32)
                                    .desired_width(120.0)
                                    .desired_height(6.0)
                                    .fill(theme::ACCENT),
                            );
                            if waiting > 0 {
                                ui.label(theme::small(format!("{waiting} more waiting")));
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.add(theme::quiet("Cancel")).clicked() {
                                        cancel = Some(job.id);
                                    }
                                },
                            );
                        }
                        JobState::Done { outputs, .. } => {
                            let bytes: u64 = outputs.iter().map(|o| o.bytes).sum();
                            ui.label(format!("Saved · {}", format::size(bytes)));
                            let label = if outputs.len() == 1 {
                                name(&outputs[0].path)
                            } else {
                                format!("{} videos", outputs.len())
                            };
                            ui.label(theme::small(label));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.add(theme::quiet("×")).on_hover_text("Dismiss").clicked()
                                    {
                                        dismiss = true;
                                    }
                                    if ui.add(theme::quiet("Details")).clicked() {
                                        open_details = Some(job.id);
                                    }
                                    if ui
                                        .add(
                                            theme::quiet("Show in folder")
                                                .min_size(egui::vec2(0.0, 36.0)),
                                        )
                                        .clicked()
                                    {
                                        show = outputs.first().map(|o| o.path.clone());
                                    }
                                },
                            );
                        }
                        JobState::Failed { message } => {
                            ui.label(
                                egui::RichText::new("Couldn't save")
                                    .color(theme::DANGER)
                                    .strong(),
                            );
                            ui.label(theme::small(message.clone()));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.add(theme::quiet("×")).on_hover_text("Dismiss").clicked()
                                    {
                                        dismiss = true;
                                    }
                                    if ui.add(theme::quiet("Details")).clicked() {
                                        open_details = Some(job.id);
                                    }
                                },
                            );
                        }
                        JobState::Canceled => {
                            ui.label("Canceled");
                            ui.label(theme::small("Nothing was saved."));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.add(theme::quiet("×")).clicked() {
                                        dismiss = true;
                                    }
                                },
                            );
                        }
                        JobState::Waiting => {}
                    }
                });
            });
        if let Some(id) = cancel {
            if let Some(e) = &self.engine {
                e.cancel(id);
            }
        }
        if dismiss {
            self.jobs[idx].dismissed = true;
        }
        if let Some(id) = open_details {
            self.details = Some(DetailsWindow { job: id });
        }
        if let Some(p) = show {
            show_in_folder(&p);
        }
    }

    fn drop_overlay(&mut self, ctx: &egui::Context) {
        let (hovering, dropped): (bool, Vec<PathBuf>) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .collect(),
            )
        });
        if !dropped.is_empty() {
            let mut files = vec![];
            for p in dropped {
                if p.is_dir() {
                    if let Ok(rd) = std::fs::read_dir(&p) {
                        files.extend(rd.flatten().map(|e| e.path()).filter(|f| {
                            f.extension()
                                .map(|e| {
                                    VIDEO_EXTS.contains(
                                        &e.to_string_lossy().to_ascii_lowercase().as_str(),
                                    )
                                })
                                .unwrap_or(false)
                        }));
                    }
                } else {
                    files.push(p);
                }
            }
            // Drops arrive in no particular order: sort `clip 2` before `clip 10`.
            files.sort_by_key(|a| natural_key(a));
            self.add_files(files);
        }
        if hovering {
            let screen = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop"),
            ));
            painter.rect_filled(
                screen,
                0,
                Color32::from_rgba_unmultiplied(232, 240, 252, 230),
            );
            painter.rect_stroke(
                screen.shrink(12.0),
                12,
                Stroke::new(3.0, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
            let text = if self.project.is_empty() {
                "Drop to open"
            } else {
                "Drop to add to the end of the timeline"
            };
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(22.0),
                INK,
            );
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.toast else { return };
        if Instant::now() > t.until {
            self.toast = None;
            return;
        }
        let text = t.text.clone();
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 64.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(INK)
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(text).color(Color32::WHITE));
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn path_prompt_ui(&mut self, ctx: &egui::Context) {
        let Some(text) = &mut self.path_prompt else {
            return;
        };
        let mut open = None;
        let mut close = false;
        egui::Modal::new(egui::Id::unique("path-prompt")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Open a video");
            ui.label(theme::small("Type or paste the path of a video file."));
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(f32::INFINITY)
                    .hint_text("/home/you/Videos/clip.mp4"),
            );
            r.request_focus();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(theme::primary("Open")).clicked()
                        || ui.input(|i| i.key_pressed(Key::Enter))
                    {
                        open = Some(PathBuf::from(text.trim()));
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                });
            });
        });
        if let Some(p) = open {
            self.path_prompt = None;
            let p = shellexpand_home(&p);
            if p.is_file() {
                self.add_files(vec![p]);
            } else {
                self.toast("There's no file at that path.".into());
            }
        } else if close {
            self.path_prompt = None;
        }
    }

    fn quit_guard(&mut self, ctx: &egui::Context) {
        let busy = self.engine.as_ref().map(|e| e.busy()).unwrap_or(false);
        if ctx.input(|i| i.viewport().close_requested()) {
            if busy && !self.confirm_quit {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_quit = true;
            } else {
                self.save_session_if_changed(true);
            }
        }
        if self.confirm_quit {
            egui::Modal::new(egui::Id::unique("quit")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Still saving");
                ui.label("If you quit now, the export stops and nothing is saved.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::primary("Keep saving")).clicked() {
                            self.confirm_quit = false;
                        }
                        if ui.button("Quit anyway").clicked() {
                            if let Some(e) = &self.engine {
                                e.cancel_all();
                            }
                            self.save_session_if_changed(true);
                            // Give the worker a moment to kill ffmpeg and clean up.
                            std::thread::sleep(Duration::from_millis(300));
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            });
        }
    }

    pub fn source_for(&self, clip: u64) -> Option<&MediaInfo> {
        self.project
            .clip(clip)
            .and_then(|c| self.sources.get(&c.path))
    }
}

fn shellexpand_home(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = std::env::var_os("HOME") {
            return PathBuf::from(h).join(rest);
        }
    }
    p.to_path_buf()
}

/// Sort `clip 2` before `clip 10` when several files are dropped at once.
fn natural_key(p: &Path) -> Vec<(String, u64)> {
    let s = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut out = vec![];
    let mut text = String::new();
    let mut num = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
        } else {
            if !num.is_empty() {
                out.push((std::mem::take(&mut text), num.parse().unwrap_or(0)));
                num.clear();
            }
            text.push(ch);
        }
    }
    out.push((text, num.parse().unwrap_or(0)));
    out
}

impl eframe::App for CutlineApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_probes();
        self.handle_engine();
        self.poll_file_dialog();
        self.pump_thumbnails(&ctx);
        self.tick_playback();
        self.shortcuts(&ctx);

        let title = match self.project.clips.first() {
            Some(c) => format!("{} — Cutline", c.name()),
            None => "Cutline".into(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        if let Some(err) = self.tools_error.clone() {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(120.0);
                    ui.heading("Cutline needs ffmpeg");
                    ui.label(err);
                });
            });
            return;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(16))
            .show(ui, |ui| {
                egui::Frame::new()
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(10)
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(16, 4))
                            .show(ui, |ui| self.top_bar(ui));
                        let r = ui.max_rect();
                        ui.painter().hline(
                            r.x_range(),
                            ui.cursor().top(),
                            Stroke::new(1.0, theme::HAIRLINE),
                        );
                        egui::Frame::new().inner_margin(16).show(ui, |ui| {
                            let job_h = if self
                                .jobs
                                .iter()
                                .any(|j| !j.dismissed && !matches!(j.state, JobState::Waiting))
                            {
                                1.0
                            } else {
                                0.0
                            };
                            let h = (ui.available_height() - self.below_h - 16.0).max(160.0);
                            self.preview_area(ui, h);
                            let below_top = ui.cursor().top();
                            self.transport(ui);
                            let action = timeline::show(ui, self);
                            match action {
                                TimelineAction::None => {}
                                TimelineAction::Seek(c, t) => self.seek(c, t),
                                TimelineAction::DragStart(c, t) => {
                                    if self.resume_after_drag.is_none() {
                                        self.checkpoint();
                                        let playing = self.playing();
                                        if let Some(p) = &mut self.player {
                                            p.set_paused(true);
                                        }
                                        self.resume_after_drag = Some(playing);
                                    }
                                    self.seek(c, t);
                                }
                                TimelineAction::Dragging(c, t) => self.seek(c, t),
                                TimelineAction::DragEnd(c, t, start_handle) => {
                                    self.seek(c, t);
                                    // Keep playing from a new start; stay on the last frame after moving an end.
                                    if let (Some(true), true) =
                                        (self.resume_after_drag.take(), start_handle)
                                    {
                                        if let Some(p) = &mut self.player {
                                            p.set_paused(false);
                                        }
                                    }
                                    self.resume_after_drag = None;
                                }
                                TimelineAction::Checkpoint => self.checkpoint(),
                                TimelineAction::RemoveClip(c) => {
                                    self.checkpoint();
                                    let was_loaded = self.loaded == Some(c);
                                    self.project.remove_clip(c);
                                    if was_loaded {
                                        self.loaded = None;
                                        self.after_project_swap_reload();
                                    }
                                }
                                TimelineAction::MoveClip(c, d) => {
                                    self.checkpoint();
                                    self.project.move_clip(c, d);
                                }
                            }
                            self.part_controls(ui);
                            if job_h > 0.0 {
                                ui.add_space(4.0);
                                self.job_bar(ui);
                            }
                            let measured = ui.min_rect().bottom() - below_top;
                            if (measured - self.below_h).abs() > 0.5 {
                                self.below_h = measured;
                                ui.ctx().request_repaint();
                            }
                        });
                    });
            });

        // Windows and dialogs on top.
        if let Some(mut d) = self.export.take() {
            match d.show(&ctx, self) {
                ExportOutcome::Open => self.export = Some(d),
                ExportOutcome::Closed => {}
                ExportOutcome::Advanced => {
                    self.advanced = Some(AdvancedEditor::new(self, d.preset_id()));
                    self.export = Some(d);
                }
            }
        }
        if let Some(mut a) = self.advanced.take() {
            if let Some(use_id) = a.show(&ctx, self) {
                if let Some(d) = &mut self.export {
                    d.use_preset(&use_id);
                }
            }
            if a.open {
                self.advanced = Some(a);
            }
        }
        if let Some(d) = self.details.take() {
            if d.show(&ctx, self) {
                self.details = Some(d);
            }
        }
        self.path_prompt_ui(&ctx);
        self.toast_ui(&ctx);
        self.drop_overlay(&ctx);
        self.quit_guard(&ctx);
        self.save_session_if_changed(false);

        if self.playing()
            || self
                .jobs
                .iter()
                .any(|j| matches!(j.state, JobState::Running { .. }))
        {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_session_if_changed(true);
        if let Some(e) = &self.engine {
            e.cancel_all();
        }
    }
}

impl CutlineApp {
    fn after_project_swap_reload(&mut self) {
        if let Some(c) = self.project.clips.first().map(|c| c.id) {
            self.load_clip(c, Timestamp::ZERO, false);
        } else {
            if let Some(p) = &mut self.player {
                p.close();
            }
            self.preview = None;
            self.playhead = None;
        }
    }
}
