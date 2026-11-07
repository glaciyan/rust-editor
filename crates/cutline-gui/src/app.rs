//! The main window: a header with Open, the file, undo/redo, Export and the
//! window buttons; the picture on black; the transport with volume; and the
//! timeline with its toolbar. Dialogs stack on top (see `modal`).

use crate::export::ExportDialog;
use crate::icons::Icon;
use crate::modal::{Dialog, Kind};
use crate::theme::{self, MUTED, TEXT};
use crate::timeline::{self, TimelineAction, TimelineView};
use cutline_core::{format, PartMode, Project, Timestamp};
use cutline_engine::estimate::Calibration;
use cutline_engine::job::{JobDetails, JobId, JobOutput};
use cutline_engine::request::ExportRequest;
use cutline_engine::store::{self, ProfileStore, Session};
use cutline_engine::{Engine, EngineEvent};
use cutline_ffmpeg::{probe, MediaInfo, Tools};
use cutline_preview::thumbs::{ThumbRequest, Thumbnailer};
use cutline_preview::Player;
use eframe::egui::{self, Color32, Key, Modifiers, Role, Sense, Stroke, WidgetInfo};
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
    /// Open dialogs, bottom first; only the last one is shown.
    pub dialogs: Vec<Dialog>,
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
    /// Quit anyway was pressed: let the next close request through.
    quitting: bool,
    title: String,
    /// Preview volume 0–100, and whether it's muted.
    pub volume: f64,
    pub muted: bool,
    /// Read and write the session and profiles on disk. Off in tests.
    live: bool,
    /// Draw our own title bar (window buttons, drag to move, edges to resize).
    csd: bool,
    viewer_rect: egui::Rect,
    /// Every export handed to the engine, for the tests.
    #[cfg(test)]
    pub submitted: Vec<Vec<PathBuf>>,
}

const HEADER_H: f32 = 48.0;
const TRANSPORT_H: f32 = 48.0;

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
    pub fn new(cc: &eframe::CreationContext<'_>, args: Vec<PathBuf>, csd: bool) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut app = Self::assemble(&cc.egui_ctx, Tools::locate(), true, csd);
        if let Some(s) = store::load_session() {
            app.volume = s.volume.clamp(0.0, 100.0);
            app.muted = s.muted;
        }
        app.apply_volume();
        if args.is_empty() {
            app.restore_session();
        } else {
            app.add_files(args);
        }
        app
    }

    /// The app without its background helpers (player, thumbnails, export
    /// engine) and without touching the disk: what the UI tests drive.
    #[cfg(test)]
    pub fn for_tests(tools: Tools) -> Self {
        Self::assemble(&egui::Context::default(), Ok(tools), false, true)
    }

    fn assemble(ctx: &egui::Context, tools: Result<Tools, String>, live: bool, csd: bool) -> Self {
        let waker: cutline_preview::Waker = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };
        let (tools, tools_error) = match tools {
            Ok(t) => (Some(Arc::new(t)), None),
            Err(e) => (None, Some(e)),
        };
        let helpers = tools.as_ref().filter(|_| live);
        let engine = helpers.map(|t| {
            let e = Engine::new((**t).clone());
            let ctx = ctx.clone();
            e.set_waker(move || ctx.request_repaint());
            e
        });
        let player = helpers.map(|t| Player::new(&t.ffmpeg, waker.clone()));
        let thumbs = helpers.map(|t| {
            Thumbnailer::new(
                t.ffmpeg.clone(),
                store::cache_dir().join("thumbs"),
                waker.clone(),
            )
        });
        let (probe_tx, probe_rx) = channel();
        CutlineApp {
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
            dialogs: vec![],
            jobs: vec![],
            profiles: if live {
                ProfileStore::load()
            } else {
                ProfileStore::default()
            },
            calibrations: HashMap::new(),
            calibrating: HashSet::new(),
            last_export_dir: None,
            last_fit_mb: None,
            toast: None,
            saved_session: None,
            last_session_check: Instant::now(),
            file_dialog_rx: None,
            quitting: false,
            title: String::new(),
            volume: 100.0,
            muted: false,
            live,
            csd,
            viewer_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            submitted: vec![],
        }
    }

    pub fn save_profiles(&mut self) {
        if self.live {
            if let Err(e) = self.profiles.save() {
                self.toast(format!("Couldn't save your profiles: {e}"));
            }
        }
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
            volume: self.volume,
            muted: self.muted,
        }
    }

    fn save_session_if_changed(&mut self, force: bool) {
        if !self.live {
            return;
        }
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
                    None if quick => self.push_dialog(Dialog::OpenPath(String::new())),
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
        if self.project.is_empty() || self.tools.is_none() || self.has_dialog(Kind::Export) {
            return;
        }
        if self.project.segments().is_empty() {
            self.toast("Everything is cut out. Keep at least one part to export.".into());
            return;
        }
        if let Some(p) = &mut self.player {
            p.set_paused(true);
        }
        let d = ExportDialog::new(self);
        self.push_dialog(Dialog::Export(Box::new(d)));
    }

    pub fn submit_export(&mut self, req: ExportRequest, outputs: Vec<PathBuf>) {
        #[cfg(test)]
        self.submitted.push(outputs.clone());
        if let Some(e) = &self.engine {
            e.submit(req, outputs);
        }
    }

    /// Quit anyway: stop the exports, keep the session, close.
    pub fn quit_now(&mut self, ctx: &egui::Context) {
        if let Some(e) = &self.engine {
            e.cancel_all();
        }
        self.save_session_if_changed(true);
        self.quitting = true;
        // Give the worker a moment to kill ffmpeg and clean up.
        if self.live {
            std::thread::sleep(Duration::from_millis(300));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
        // A dialog has the keyboard to itself (Esc, Enter, typing).
        if !self.dialogs.is_empty() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let mut actions = vec![];
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::E) {
                actions.push("export");
            }
            if i.consume_key(Modifiers::COMMAND, Key::O) {
                actions.push("open");
            }
            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y)
            {
                actions.push("redo");
            }
            if i.consume_key(Modifiers::COMMAND, Key::Z) {
                actions.push("undo");
            }
            let plain = [
                (Key::Space, "play"),
                (Key::I, "in"),
                (Key::O, "out"),
                (Key::S, "split"),
                (Key::A, "add"),
                (Key::Delete, "delete"),
                (Key::Backspace, "delete"),
                (Key::ArrowLeft, "prev-frame"),
                (Key::ArrowRight, "next-frame"),
                (Key::Home, "home"),
                (Key::End, "end"),
                (Key::M, "mute"),
                (Key::ArrowUp, "vol-up"),
                (Key::ArrowDown, "vol-down"),
            ];
            if i.consume_key(Modifiers::SHIFT, Key::ArrowLeft) {
                actions.push("back-1s");
            }
            if i.consume_key(Modifiers::SHIFT, Key::ArrowRight) {
                actions.push("fwd-1s");
            }
            for (key, action) in plain {
                if i.consume_key(Modifiers::NONE, key) {
                    actions.push(action);
                }
            }
        });
        for a in actions {
            self.run_action(a);
        }
    }

    fn run_action(&mut self, a: &str) {
        match a {
            "export" => self.open_export(),
            "open" => self.open_dialog(),
            "undo" => self.undo(),
            "redo" => self.redo(),
            "play" => self.toggle_play(),
            "in" | "out" => {
                if let Some((clip, t)) = self.playhead {
                    self.checkpoint();
                    let done = if a == "in" {
                        self.project.set_in(clip, t)
                    } else {
                        self.project.set_out(clip, t)
                    };
                    if done.is_none() {
                        self.undo.pop();
                    }
                    self.was_in_kept = self.project.is_kept(clip, t);
                }
            }
            "split" => self.split_at_playhead(),
            "delete" => self.delete_selected_part(),
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
            "mute" => self.toggle_mute(),
            "vol-up" => self.set_volume(self.shown_volume() + 5.0),
            "vol-down" => self.set_volume(self.shown_volume() - 5.0),
            _ => {}
        }
    }

    fn shown_volume(&self) -> f64 {
        if self.muted {
            0.0
        } else {
            self.volume
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

    pub fn split_at_playhead(&mut self) {
        let Some((clip, t)) = self.playhead else {
            return;
        };
        self.checkpoint();
        if self.project.split_part(clip, t).is_none() {
            self.undo.pop();
            self.toast("Put the playhead inside a part, away from its ends, to split it.".into());
        }
    }

    pub fn delete_selected_part(&mut self) {
        if let Some(s) = self.selected_part() {
            self.checkpoint();
            self.project.remove_part(s);
        }
    }

    fn selected_part(&self) -> Option<cutline_core::PartRef> {
        self.project
            .selected
            .filter(|r| self.project.part(*r).is_some())
    }

    // ---------- drawing ----------

    /// The title and the facts shown next to it in the header.
    fn file_facts(&self) -> (String, Vec<String>) {
        let total = self.project.total_duration().secs();
        match self.project.clips.len() {
            0 => (String::new(), vec![]),
            1 => {
                let c = &self.project.clips[0];
                let mut facts = vec![];
                if let Some(v) = self.sources.get(&c.path).and_then(|m| m.video.as_ref()) {
                    let (w, h) = v.display_size();
                    facts.push(format!("{w}×{h}"));
                    if v.fps > 0.0 {
                        facts.push(format!("{} fps", trim_fps(v.fps)));
                    }
                    facts.push(codec_name(&v.codec));
                }
                facts.push(format::clock(total));
                (c.name(), facts)
            }
            n => (format!("{n} videos"), vec![format::clock(total)]),
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        // Drag the window by its header; the buttons on it still get their clicks.
        if self.csd {
            let bar = ui.interact(
                full.expand2(egui::vec2(10.0, 0.0)),
                ui.scope_id().with("title-bar"),
                Sense::click_and_drag(),
            );
            if bar.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
        }
        let mut undo = false;
        let mut redo = false;

        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(full)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if left
            .add(theme::ghost("Open").icon(Icon::Open).height(32.0))
            .on_hover_text("Open videos (Ctrl+O)")
            .clicked()
        {
            self.open_dialog();
        }
        let left_end = left.min_rect().right();

        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(full)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right.spacing_mut().item_spacing.x = 4.0;
        if self.csd {
            if window_button(&mut right, Icon::Close, "Close window", true).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            let (icon, label) = if maximized {
                (Icon::Restore, "Restore window")
            } else {
                (Icon::Maximize, "Maximize window")
            };
            if window_button(&mut right, icon, label, false).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            if window_button(&mut right, Icon::Minimize, "Minimize window", false).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            theme::vsep(&mut right, 20.0);
        }
        let enabled = !self.project.is_empty() && self.tools.is_some();
        if right
            .add_enabled(enabled, theme::primary("Export").icon(Icon::Export))
            .on_hover_text("Export (Ctrl+E)")
            .clicked()
        {
            self.open_export();
        }
        right.add_space(6.0);
        right.add_enabled_ui(!self.redo.is_empty(), |ui| {
            redo = theme::icon_button(
                ui,
                Icon::Redo,
                egui::vec2(32.0, 32.0),
                "Redo",
                "Redo (Ctrl+Shift+Z)",
            )
            .clicked();
        });
        right.add_enabled_ui(!self.undo.is_empty(), |ui| {
            undo = theme::icon_button(
                ui,
                Icon::Undo,
                egui::vec2(32.0, 32.0),
                "Undo",
                "Undo (Ctrl+Z)",
            )
            .clicked();
        });
        let right_start = right.min_rect().left();
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }

        // File name and facts, centred in the window, dropping facts when tight.
        let (name, mut facts) = self.file_facts();
        if name.is_empty() {
            return;
        }
        let p = ui.painter();
        let name_g = p.layout_no_wrap(
            name,
            egui::FontId::new(13.5, theme::semibold_family()),
            TEXT,
        );
        let room = (right_start - left_end - 32.0).max(0.0);
        let facts_g = |facts: &Vec<String>| {
            p.layout_no_wrap(facts.join(" · "), egui::FontId::proportional(12.0), MUTED)
        };
        let width = |facts: &Vec<String>| {
            name_g.size().x
                + if facts.is_empty() {
                    0.0
                } else {
                    10.0 + facts_g(facts).size().x
                }
        };
        while !facts.is_empty() && width(&facts) > room {
            facts.remove(0);
        }
        let w = width(&facts).min(room);
        let x = (full.center().x - w / 2.0)
            .max(left_end + 16.0)
            .min(right_start - 16.0 - w);
        let cy = full.center().y;
        let clip =
            egui::Rect::from_x_y_ranges(left_end + 16.0..=right_start - 16.0, full.y_range());
        let p = p.with_clip_rect(clip);
        let nw = name_g.size();
        // Line the two up on their baselines.
        let base = cy + nw.y * 0.3;
        p.galley(egui::pos2(x, cy - nw.y / 2.0), name_g, TEXT);
        if !facts.is_empty() {
            let g = facts_g(&facts);
            let gy = base - g.size().y * 0.8;
            p.galley(egui::pos2(x + nw.x + 10.0, gy), g, MUTED);
        }
    }

    fn viewer(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
        self.viewer_rect = rect;
        let painter = ui.painter_at(rect);
        if self.project.is_empty() && self.pending.is_empty() {
            self.empty_state(ui, rect);
            return;
        }
        if self.project.is_empty() {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_center_size(
                        rect.center() - egui::vec2(0.0, 14.0),
                        egui::vec2(24.0, 24.0),
                    ))
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            child.add(egui::Spinner::new().size(22.0).color(MUTED));
            painter.text(
                rect.center() + egui::vec2(0.0, 18.0),
                egui::Align2::CENTER_CENTER,
                "Opening…",
                egui::FontId::proportional(13.5),
                MUTED,
            );
            return;
        }
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Preview"));
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
        // Say so when the picture under the playhead won't be in the export.
        if let Some((clip, t)) = self.playhead {
            if !self.project.is_kept(clip, t) {
                let g = painter.layout_no_wrap(
                    "Cut out of the export".into(),
                    egui::FontId::new(12.0, theme::semibold_family()),
                    Color32::WHITE,
                );
                let badge = egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(12.0, 12.0),
                    egui::vec2(g.size().x + 34.0, 28.0),
                );
                painter.rect(
                    badge,
                    14,
                    Color32::from_black_alpha(190),
                    Stroke::new(1.0, Color32::from_white_alpha(24)),
                    egui::StrokeKind::Inside,
                );
                painter.circle_filled(
                    egui::pos2(badge.left() + 14.0, badge.center().y),
                    4.0,
                    theme::PLAYHEAD,
                );
                painter.galley(
                    egui::pos2(badge.left() + 24.0, badge.center().y - g.size().y / 2.0),
                    g,
                    Color32::WHITE,
                );
            }
        }
        // A play hint over a paused picture.
        if resp.hovered() && !self.playing() {
            let c = rect.center();
            painter.circle_filled(c, 34.0, Color32::from_black_alpha(150));
            painter.circle_stroke(c, 34.0, Stroke::new(1.0, Color32::from_white_alpha(40)));
            Icon::Play.paint(
                &painter,
                egui::Rect::from_center_size(c + egui::vec2(2.0, 0.0), egui::vec2(28.0, 28.0)),
                Color32::WHITE,
            );
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            self.toggle_play();
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let p = ui.painter_at(rect);
        let r = rect.shrink(16.0);
        let pts = vec![
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        p.extend(egui::Shape::dashed_line(
            &pts,
            Stroke::new(1.2, theme::BORDER),
            7.0,
            6.0,
        ));
        let tall = rect.height() > 320.0;
        let c = rect.center() + egui::vec2(0.0, if tall { 18.0 } else { -4.0 });
        if tall {
            let ic = c - egui::vec2(0.0, 92.0);
            p.circle(ic, 32.0, theme::PANEL, Stroke::new(1.0, theme::BORDER));
            Icon::Film.paint(
                &p,
                egui::Rect::from_center_size(ic, egui::vec2(28.0, 28.0)),
                MUTED,
            );
        }
        p.text(
            c - egui::vec2(0.0, 36.0),
            egui::Align2::CENTER_CENTER,
            "Drop videos here",
            egui::FontId::new(20.0, theme::semibold_family()),
            TEXT,
        );
        p.text(
            c - egui::vec2(0.0, 10.0),
            egui::Align2::CENTER_CENTER,
            "MP4, MKV, MOV and more · H.265 and H.264 cut on the exact frame",
            egui::FontId::proportional(13.0),
            MUTED,
        );
        let b = egui::Rect::from_center_size(c + egui::vec2(0.0, 32.0), egui::vec2(160.0, 36.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(b).layout(
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        ));
        if child
            .add(
                theme::primary("Open videos…")
                    .icon(Icon::Open)
                    .height(36.0)
                    .min_width(160.0),
            )
            .clicked()
        {
            self.open_dialog();
        }
        p.text(
            c + egui::vec2(0.0, 70.0),
            egui::Align2::CENTER_CENTER,
            "or press Ctrl+O",
            egui::FontId::proportional(12.0),
            MUTED,
        );
    }

    fn current_fps(&self) -> f64 {
        self.playhead
            .and_then(|(c, _)| self.project.clip(c))
            .or_else(|| self.project.clips.first())
            .map(|c| c.fps)
            .filter(|f| *f > 0.0)
            .unwrap_or(30.0)
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        let row = ui.max_rect();
        let has_media = !self.project.is_empty();

        // Centre: previous frame, play, next frame in one track.
        let cr = egui::Rect::from_center_size(
            row.center(),
            egui::vec2(2.0 * 30.0 + 38.0 + 4.0 + 6.0, 34.0),
        );
        ui.painter().rect(
            cr,
            8,
            theme::BG,
            Stroke::new(1.0, theme::HAIRLINE),
            egui::StrokeKind::Inside,
        );
        let mut actions: Vec<&str> = vec![];
        let mut c = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(cr.shrink(3.0))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        c.spacing_mut().item_spacing.x = 2.0;
        c.add_enabled_ui(has_media, |ui| {
            let s = egui::vec2(30.0, 28.0);
            if theme::icon_button(
                ui,
                Icon::StepBack,
                s,
                "Previous frame",
                "Previous frame (←)",
            )
            .clicked()
            {
                actions.push("prev-frame");
            }
            if self.play_button(ui).clicked() {
                actions.push("play");
            }
            if theme::icon_button(ui, Icon::StepForward, s, "Next frame", "Next frame (→)")
                .clicked()
            {
                actions.push("next-frame");
            }
        });
        for a in actions {
            self.run_action(a);
        }

        // Left: where the playhead is, and the whole length.
        let fps = self.current_fps();
        let now = self
            .playhead
            .map(|(c, t)| self.project.clip_offset(c) + t)
            .unwrap_or_default();
        let total = self.project.total_duration();
        let p = ui.painter();
        let cur = p.layout_no_wrap(
            clock_ms(now.secs()),
            theme::MONO(15.0),
            if has_media { TEXT } else { MUTED },
        );
        let tot = p.layout_no_wrap(
            format!("/ {}", clock_ms(total.secs())),
            theme::MONO(12.5),
            MUTED,
        );
        let left_room = cr.left() - row.left() - 12.0;
        let cy = row.center().y;
        let tc_rect =
            egui::Rect::from_min_size(egui::pos2(row.left(), cy - cur.size().y / 2.0), cur.size());
        let cur_w = cur.size().x;
        if cur_w + 8.0 <= left_room {
            p.galley(tc_rect.min, cur, TEXT);
            if cur_w + 8.0 + tot.size().x <= left_room {
                p.galley(
                    egui::pos2(tc_rect.right() + 7.0, cy - tot.size().y / 2.0 + 1.0),
                    tot,
                    MUTED,
                );
            }
            let frame = (now.secs() * fps).round() as u64;
            ui.interact(tc_rect, ui.scope_id().with("tc"), Sense::hover())
                .on_hover_text(format!(
                    "Hours : minutes : seconds . milliseconds · frame {frame} at {} fps",
                    trim_fps(fps)
                ));
        }

        // Right: volume.
        let rr = egui::Rect::from_min_max(egui::pos2(cr.right() + 12.0, row.top()), row.max);
        let mut r = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rr)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        r.spacing_mut().item_spacing.x = 6.0;
        self.volume_control(&mut r);
    }

    /// Play / pause, the one lit button in the transport track.
    fn play_button(&self, ui: &mut egui::Ui) -> egui::Response {
        let playing = self.playing();
        let (r, resp) = ui.allocate_exact_size(egui::vec2(38.0, 28.0), Sense::click());
        let enabled = ui.is_enabled();
        let label = if playing { "Pause" } else { "Play" };
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
        let p = ui.painter();
        let fill = if !enabled {
            theme::RAISED
        } else if resp.is_pointer_button_down_on() {
            theme::BORDER
        } else if resp.hovered() {
            theme::SEGMENT_ON
        } else {
            theme::HOVER
        };
        p.rect_filled(r, 6, fill);
        let fg = if enabled { TEXT } else { theme::FAINT };
        let icon = if playing { Icon::Pause } else { Icon::Play };
        icon.paint(
            p,
            egui::Rect::from_center_size(r.center(), egui::vec2(14.0, 14.0)),
            fg,
        );
        if resp.hovered() && enabled {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp.on_hover_text("Play / pause (Space)")
    }

    pub fn set_volume(&mut self, v: f64) {
        self.volume = v.clamp(0.0, 100.0).round();
        if self.volume > 0.0 {
            self.muted = false;
        }
        self.apply_volume();
    }

    fn toggle_mute(&mut self) {
        if self.muted || self.volume <= 0.0 {
            self.muted = false;
            if self.volume <= 0.0 {
                self.volume = 50.0;
            }
        } else {
            self.muted = true;
        }
        self.apply_volume();
    }

    fn apply_volume(&mut self) {
        let v = if self.muted { 0.0 } else { self.volume };
        if let Some(p) = &mut self.player {
            p.set_volume(v);
        }
    }

    /// Mute button, a thin slider and the number, laid out right to left.
    fn volume_control(&mut self, ui: &mut egui::Ui) {
        let has_sound = self.player.as_ref().map(|p| p.has_sound()).unwrap_or(false);
        let shown = self.shown_volume();
        // The number.
        let (pr, _) = ui.allocate_exact_size(egui::vec2(24.0, 20.0), Sense::hover());
        ui.painter().text(
            pr.right_center(),
            egui::Align2::RIGHT_CENTER,
            if has_sound {
                format!("{shown:.0}")
            } else {
                "—".into()
            },
            theme::MONO(11.0),
            MUTED,
        );
        // Slider.
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(80.0, 24.0), Sense::click_and_drag());
        let track =
            egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 8.0, 2.0));
        if has_sound {
            if resp.is_pointer_button_down_on() || resp.dragged() || resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let f = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
                    self.set_volume(f as f64 * 100.0);
                }
            }
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
                if scroll != 0.0 {
                    self.set_volume(shown + scroll as f64 * 0.25);
                }
            }
        }
        let shown = self.shown_volume();
        resp.widget_info(|| WidgetInfo::slider(has_sound, shown, "Volume"));
        let active = has_sound && (resp.hovered() || resp.dragged());
        let p = ui.painter();
        p.rect_filled(track, 1, theme::BORDER);
        let kx = track.left() + track.width() * (shown / 100.0) as f32;
        let filled = egui::Rect::from_min_max(track.min, egui::pos2(kx, track.max.y));
        p.rect_filled(
            filled,
            1,
            if !has_sound {
                theme::BORDER
            } else if active {
                TEXT
            } else {
                MUTED
            },
        );
        if has_sound {
            p.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(kx, track.center().y),
                    egui::vec2(3.0, 12.0),
                ),
                1,
                TEXT,
            );
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            resp.on_hover_text(format!("Volume {shown:.0} · scroll or ↑ / ↓ to change"));
        } else {
            resp.on_hover_text("The preview has no sound. Install libmpv to hear it.");
        }
        // Mute toggle.
        let icon = if !has_sound || shown <= 0.0 {
            Icon::Muted
        } else if shown < 50.0 {
            Icon::Volume(1)
        } else {
            Icon::Volume(2)
        };
        let mut toggle = false;
        ui.add_enabled_ui(has_sound, |ui| {
            let (label, tip) = if self.muted {
                ("Unmute", "Unmute (M)")
            } else {
                ("Mute", "Mute (M)")
            };
            toggle = theme::icon_button(ui, icon, egui::vec2(28.0, 28.0), label, tip).clicked();
        });
        if toggle {
            self.toggle_mute();
        }
    }

    /// The row above the timeline: Split, Delete part, the I/O hint and the totals.
    fn edit_toolbar(&mut self, ui: &mut egui::Ui) {
        let has_media = !self.project.is_empty();
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            ui.spacing_mut().item_spacing.x = 8.0;
            let splittable = has_media
                && self
                    .playhead
                    .and_then(|(c, t)| {
                        let mut p = self.project.clone();
                        p.split_part(c, t)
                    })
                    .is_some();
            if ui
                .add_enabled(
                    splittable,
                    theme::secondary("Split")
                        .icon(Icon::Scissors)
                        .kbd("S")
                        .height(30.0),
                )
                .on_hover_text("Split the part at the playhead (S)")
                .on_disabled_hover_text("Put the playhead inside a part to split it")
                .clicked()
            {
                self.split_at_playhead();
            }
            if ui
                .add_enabled(
                    self.selected_part().is_some(),
                    theme::secondary("Delete part")
                        .icon(Icon::Trash)
                        .kbd("Del")
                        .height(30.0),
                )
                .on_hover_text("Delete the selected part (Del)")
                .on_disabled_hover_text("Click a part on the timeline to select it")
                .clicked()
            {
                self.delete_selected_part();
            }
            // Totals on the right; the hint fills what's left between, if it fits.
            let summary = self.part_summary();
            let summary_w = summary
                .as_ref()
                .map(|(a, b, _)| {
                    let p = ui.painter();
                    p.layout_no_wrap(
                        a.clone(),
                        egui::FontId::new(13.0, theme::semibold_family()),
                        TEXT,
                    )
                    .size()
                    .x + 6.0
                        + p.layout_no_wrap(b.clone(), egui::FontId::proportional(12.5), MUTED)
                            .size()
                            .x
                })
                .unwrap_or(0.0);
            if ui.available_width() - summary_w > 220.0 {
                ui.add_space(6.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(theme::small("Set start and end with"));
                theme::keycap(ui, "I");
                theme::keycap(ui, "O");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if let Some((count, rest, ok)) = summary {
                    if !ok {
                        ui.label(egui::RichText::new(count).color(theme::DANGER));
                        return;
                    }
                    if ui.available_width() < summary_w {
                        return;
                    }
                    ui.label(theme::small(rest).size(12.5));
                    ui.label(theme::strong(count).size(13.0));
                }
            });
        });
    }

    /// (`3 parts`, `· exports 11.4 s of 16.1 s`, anything exported).
    fn part_summary(&self) -> Option<(String, String, bool)> {
        if self.project.is_empty() {
            return None;
        }
        let segs = self.project.segments();
        if segs.is_empty() {
            return Some(("Everything is cut out".into(), String::new(), false));
        }
        let parts: usize = self.project.clips.iter().map(|c| c.parts.len()).sum();
        let noun = match self.project.mode {
            PartMode::Keep => "part",
            PartMode::Remove => "cut",
        };
        let out: f64 = segs.iter().map(|s| s.len().secs()).sum();
        let total = self.project.total_duration().secs();
        Some((
            format!("{parts} {noun}{}", if parts == 1 { "" } else { "s" }),
            format!(
                "· exports {} of {}",
                format::length(out),
                format::length(total)
            ),
            true,
        ))
    }

    /// Progress or result of the latest export, floating over the bottom
    /// right of the picture.
    fn job_card(&mut self, ctx: &egui::Context) {
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
        if self.viewer_rect == egui::Rect::NOTHING {
            return;
        }
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
        let small_btn =
            |ui: &mut egui::Ui, text: &str| ui.add(theme::ghost(text).height(26.0)).clicked();
        egui::Area::new(egui::Id::unique("job-card"))
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .fixed_pos(self.viewer_rect.right_bottom() - egui::vec2(12.0, 12.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(10)
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 6,
                        top: 6,
                        bottom: 6,
                    })
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(110),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width((self.viewer_rect.width() - 24.0).max(200.0));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            match &job.state {
                                JobState::Running {
                                    fraction,
                                    eta,
                                    stage,
                                } => {
                                    ui.add(egui::Spinner::new().size(12.0).color(theme::ACCENT));
                                    let mut text = format!("{stage} · {:.0}%", fraction * 100.0);
                                    if let Some(e) = eta {
                                        text.push_str(&format!(" · {}", format::time_left(*e)));
                                    }
                                    ui.label(egui::RichText::new(text).size(12.5));
                                    theme::progress_bar(ui, *fraction as f32, 120.0);
                                    ui.add(
                                        egui::Label::new(theme::small(
                                            job.outputs.first().map(name).unwrap_or_default(),
                                        ))
                                        .truncate(),
                                    );
                                    if waiting > 0 {
                                        ui.label(theme::small(format!("· {waiting} more waiting")));
                                    }
                                    if ui
                                        .add(
                                            theme::ghost("Cancel")
                                                .label("Cancel export")
                                                .height(26.0),
                                        )
                                        .clicked()
                                    {
                                        cancel = Some(job.id);
                                    }
                                }
                                JobState::Done { outputs, .. } => {
                                    let (r, _) = ui.allocate_exact_size(
                                        egui::vec2(14.0, 14.0),
                                        Sense::hover(),
                                    );
                                    Icon::Check.paint(ui.painter(), r, theme::SUCCESS);
                                    let bytes: u64 = outputs.iter().map(|o| o.bytes).sum();
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Saved · {}",
                                            format::size(bytes)
                                        ))
                                        .size(12.5),
                                    );
                                    let label = if outputs.len() == 1 {
                                        name(&outputs[0].path)
                                    } else {
                                        format!("{} videos", outputs.len())
                                    };
                                    ui.add(egui::Label::new(theme::small(label)).truncate());
                                    if small_btn(ui, "Show in folder") {
                                        show = outputs.first().map(|o| o.path.clone());
                                    }
                                    if small_btn(ui, "Details") {
                                        open_details = Some(job.id);
                                    }
                                    if theme::icon_button(
                                        ui,
                                        Icon::Close,
                                        egui::vec2(26.0, 26.0),
                                        "Dismiss",
                                        "Dismiss",
                                    )
                                    .clicked()
                                    {
                                        dismiss = true;
                                    }
                                }
                                JobState::Failed { message } => {
                                    let (r, _) = ui.allocate_exact_size(
                                        egui::vec2(14.0, 14.0),
                                        Sense::hover(),
                                    );
                                    Icon::Close.paint(ui.painter(), r, theme::DANGER);
                                    ui.label(
                                        egui::RichText::new("Couldn't save")
                                            .size(12.5)
                                            .color(theme::DANGER),
                                    );
                                    ui.add(
                                        egui::Label::new(theme::small(message.clone())).truncate(),
                                    );
                                    if small_btn(ui, "Details") {
                                        open_details = Some(job.id);
                                    }
                                    if theme::icon_button(
                                        ui,
                                        Icon::Close,
                                        egui::vec2(26.0, 26.0),
                                        "Dismiss",
                                        "Dismiss",
                                    )
                                    .clicked()
                                    {
                                        dismiss = true;
                                    }
                                }
                                JobState::Canceled => {
                                    ui.label(egui::RichText::new("Canceled").size(12.5));
                                    ui.label(theme::small("Nothing was saved."));
                                    if theme::icon_button(
                                        ui,
                                        Icon::Close,
                                        egui::vec2(26.0, 26.0),
                                        "Dismiss",
                                        "Dismiss",
                                    )
                                    .clicked()
                                    {
                                        dismiss = true;
                                    }
                                }
                                JobState::Waiting => {}
                            }
                        });
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
            if !self.has_dialog(Kind::Details) {
                self.push_dialog(Dialog::Details(id));
            }
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
            painter.rect_filled(screen, 0, Color32::from_black_alpha(200));
            painter.rect(
                screen.shrink(16.0),
                16,
                theme::ACCENT_SOFT,
                Stroke::new(2.0, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
            let c = screen.center();
            painter.circle_filled(c - egui::vec2(0.0, 44.0), 30.0, theme::ACCENT);
            Icon::Plus.paint(
                &painter,
                egui::Rect::from_center_size(c - egui::vec2(0.0, 44.0), egui::vec2(26.0, 26.0)),
                theme::ON_ACCENT,
            );
            let text = if self.project.is_empty() {
                "Drop to open"
            } else {
                "Drop to add to the end of the timeline"
            };
            painter.text(
                c + egui::vec2(0.0, 10.0),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::new(20.0, theme::semibold_family()),
                TEXT,
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
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, HEADER_H + 14.0))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::RAISED)
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(140),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(520.0);
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                            Icon::Info.paint(ui.painter(), r, theme::ACCENT);
                            ui.add(egui::Label::new(egui::RichText::new(text).color(TEXT)));
                        });
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|t| t.text.as_str())
    }

    fn quit_guard(&mut self, ctx: &egui::Context) {
        let busy = self.engine.as_ref().map(|e| e.busy()).unwrap_or(false);
        if ctx.input(|i| i.viewport().close_requested()) {
            if busy && !self.quitting {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if !self.has_dialog(Kind::ConfirmQuit) {
                    self.push_dialog(Dialog::ConfirmQuit);
                }
            } else {
                self.save_session_if_changed(true);
            }
        }
    }

    /// Without the system's title bar, the window's edges resize it.
    fn resize_edges(&self, ui: &mut egui::Ui) {
        use egui::viewport::ResizeDirection as D;
        use egui::CursorIcon as C;
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if !self.csd || maximized {
            return;
        }
        let r = ctx.content_rect();
        let (m, k) = (5.0, 14.0);
        let zones = [
            (
                egui::Rect::from_min_max(r.min, r.min + egui::vec2(k, k)),
                D::NorthWest,
                C::ResizeNorthWest,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(r.max.x - k, r.min.y),
                    egui::pos2(r.max.x, r.min.y + k),
                ),
                D::NorthEast,
                C::ResizeNorthEast,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(r.min.x, r.max.y - k),
                    egui::pos2(r.min.x + k, r.max.y),
                ),
                D::SouthWest,
                C::ResizeSouthWest,
            ),
            (
                egui::Rect::from_min_max(r.max - egui::vec2(k, k), r.max),
                D::SouthEast,
                C::ResizeSouthEast,
            ),
            (
                egui::Rect::from_min_max(r.min, egui::pos2(r.max.x, r.min.y + m)),
                D::North,
                C::ResizeNorth,
            ),
            (
                egui::Rect::from_min_max(egui::pos2(r.min.x, r.max.y - m), r.max),
                D::South,
                C::ResizeSouth,
            ),
            (
                egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + m, r.max.y)),
                D::West,
                C::ResizeWest,
            ),
            (
                egui::Rect::from_min_max(egui::pos2(r.max.x - m, r.min.y), r.max),
                D::East,
                C::ResizeEast,
            ),
        ];
        let pressed = ctx.input(|i| i.pointer.primary_pressed());
        let Some(pos) = ctx.pointer_hover_pos() else {
            return;
        };
        // Corners first: they overlap the edges.
        for (zone, dir, cursor) in zones {
            if zone.contains(pos) {
                ctx.set_cursor_icon(cursor);
                if pressed {
                    ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                }
                return;
            }
        }
    }

    pub fn source_for(&self, clip: u64) -> Option<&MediaInfo> {
        self.project
            .clip(clip)
            .and_then(|c| self.sources.get(&c.path))
    }

    /// One frame of the whole window.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
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

        egui::Panel::top("header")
            .exact_size(HEADER_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 8,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("timeline")
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 20,
                        top: 12,
                        bottom: 14,
                    }),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                self.edit_toolbar(ui);
                let action = timeline::show(ui, self);
                self.timeline_action(action);
            });
        egui::Panel::bottom("transport")
            .exact_size(TRANSPORT_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(20, 0)),
            )
            .show(ui, |ui| self.transport(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::VIEWER)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| self.viewer(ui));

        self.job_card(&ctx);
        self.show_dialogs(&ctx);
        self.toast_ui(&ctx);
        self.drop_overlay(&ctx);
        self.quit_guard(&ctx);
        self.resize_edges(ui);
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

    fn timeline_action(&mut self, action: TimelineAction) {
        match action {
            TimelineAction::None => {}
            // A held drag reports its position on every repaint, and each seek
            // wakes the UI again: re-seeking to where the preview already is
            // would keep mpv restarting the same seek while the pointer rests.
            TimelineAction::Seek(c, t) | TimelineAction::Dragging(c, t)
                if self.playhead == Some((c, t)) && !self.playing() => {}
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
                if let (Some(true), true) = (self.resume_after_drag.take(), start_handle) {
                    if let Some(p) = &mut self.player {
                        p.set_paused(false);
                    }
                }
                self.resume_after_drag = None;
            }
            TimelineAction::AddPart(c, t) => {
                self.checkpoint();
                if self.project.add_part(c, t).is_none() {
                    self.undo.pop();
                }
            }
            TimelineAction::SplitAt(c, t) => {
                self.checkpoint();
                if self.project.split_part(c, t).is_none() {
                    self.undo.pop();
                }
            }
            TimelineAction::RemovePart(r) => {
                self.checkpoint();
                self.project.remove_part(r);
            }
            TimelineAction::SetMode(m) => {
                self.checkpoint();
                self.project.set_mode(m);
            }
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
    }

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

/// A round window button in the title bar; Close turns red on hover.
fn window_button(ui: &mut egui::Ui, icon: Icon, label: &str, close: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    let hovered = resp.hovered();
    let (fill, fg) = match (close, hovered) {
        (true, true) => (theme::DANGER, theme::ON_ACCENT),
        (true, false) => (theme::HOVER, TEXT),
        (false, true) => (theme::HOVER, TEXT),
        (false, false) => (Color32::TRANSPARENT, MUTED),
    };
    let p = ui.painter();
    p.circle_filled(r.center(), 14.0, fill);
    icon.paint(
        p,
        egui::Rect::from_center_size(r.center(), egui::vec2(14.0, 14.0)),
        fg,
    );
    resp
}

/// `HH:MM:SS.mmm`: one width at every frame rate.
fn clock_ms(secs: f64) -> String {
    let s = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
    let ms = (s * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

fn trim_fps(fps: f64) -> String {
    let r = (fps * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r}")
    }
}

fn codec_name(codec: &str) -> String {
    match codec {
        "hevc" => "H.265".into(),
        "h264" => "H.264".into(),
        "av1" => "AV1".into(),
        "vp9" => "VP9".into(),
        "vp8" => "VP8".into(),
        "mpeg4" => "MPEG-4".into(),
        "prores" => "ProRes".into(),
        other => other.to_uppercase(),
    }
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
        self.frame(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_session_if_changed(true);
        if let Some(e) = &self.engine {
            e.cancel_all();
        }
    }
}

#[cfg(test)]
impl CutlineApp {
    /// Put a video on the timeline as if it had been probed, without ffprobe.
    pub fn add_probed_for_tests(&mut self, info: MediaInfo) -> u64 {
        let path = info.path.clone();
        let fps = info.video.as_ref().map(|v| v.fps).unwrap_or(0.0);
        let dur = Timestamp::from_secs(info.duration);
        self.sources.insert(path.clone(), info);
        let was_empty = self.project.is_empty();
        let id = self.project.add_clip(path, dur, fps);
        if was_empty {
            self.load_clip(id, Timestamp::ZERO, false);
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_keeps_one_width() {
        assert_eq!(clock_ms(0.0), "00:00:00.000");
        assert_eq!(clock_ms(6.832), "00:00:06.832");
        assert_eq!(clock_ms(3725.48), "01:02:05.480");
        assert_eq!(clock_ms(f64::NAN), "00:00:00.000");
    }

    #[test]
    fn codec_and_fps_labels() {
        assert_eq!(codec_name("hevc"), "H.265");
        assert_eq!(codec_name("mjpeg"), "MJPEG");
        assert_eq!(trim_fps(30.0), "30");
        assert_eq!(trim_fps(29.97002997), "29.97");
    }
}
