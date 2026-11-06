//! The two places technical words are allowed: the Advanced profile editor
//! and a finished job's Details.

use crate::app::{CutlineApp, JobState};
use crate::theme::{self, MUTED};
use cutline_core::{format, AudioCodec, Container, Profile, RateControl, VideoCodec};
use cutline_engine::job::JobId;
use eframe::egui::{self, RichText};

pub struct AdvancedEditor {
    pub open: bool,
    selected: String,
}

const PRESETS: [&str; 9] = [
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
];
const HEIGHTS: [Option<u32>; 8] = [
    None,
    Some(2160),
    Some(1440),
    Some(1080),
    Some(720),
    Some(540),
    Some(480),
    Some(360),
];
const RATES: [Option<f64>; 6] = [
    None,
    Some(60.0),
    Some(50.0),
    Some(30.0),
    Some(25.0),
    Some(24.0),
];

impl AdvancedEditor {
    pub fn new(_app: &CutlineApp, preset: Option<String>) -> Self {
        AdvancedEditor {
            open: true,
            selected: preset.unwrap_or_else(|| "original".into()),
        }
    }

    /// Returns a profile id when the user picks "Use for this export".
    pub fn show(&mut self, ctx: &egui::Context, app: &mut CutlineApp) -> Option<String> {
        let mut use_it = None;
        let mut open = self.open;
        let mut builtins = vec![Profile::original(), Profile::fit(25.0)];
        builtins.extend(Profile::presets());
        let centre = ctx.content_rect().center();
        egui::Window::new("Advanced · Export profiles")
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(centre)
            .default_size([720.0, 520.0])
            .max_width(760.0)
            .collapsible(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(200.0);
                        ui.label(theme::small("Built in"));
                        for p in &builtins {
                            ui.selectable_value(&mut self.selected, p.id.clone(), p.name.clone());
                        }
                        ui.add_space(8.0);
                        ui.label(theme::small("Yours"));
                        for p in &app.profiles.user {
                            ui.selectable_value(&mut self.selected, p.id.clone(), p.name.clone());
                        }
                        if app.profiles.user.is_empty() {
                            ui.label(theme::small("Duplicate a profile to make your own."));
                        }
                        ui.add_space(8.0);
                        if ui.button("New profile").clicked() {
                            let mut p = Profile::presets()[0].clone();
                            p.id = app.profiles.new_id();
                            p.name = "My profile".into();
                            p.builtin = false;
                            self.selected = p.id.clone();
                            app.profiles.upsert(p);
                            let _ = app.profiles.save();
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        if let Some(p) = builtins.iter().find(|p| p.id == self.selected).cloned() {
                            builtin_view(ui, &p);
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui.button("Duplicate to edit").clicked() {
                                    let mut q = p.clone();
                                    q.id = app.profiles.new_id();
                                    q.name = format!("{} (copy)", p.name);
                                    q.builtin = false;
                                    if q.id == "original" || q.is_copy() {
                                        q.video.codec = VideoCodec::H265;
                                        q.video.rate = RateControl::Crf(22);
                                    }
                                    self.selected = q.id.clone();
                                    app.profiles.upsert(q);
                                    let _ = app.profiles.save();
                                }
                                if p.id != "original"
                                    && p.id != "fit"
                                    && ui.button("Use for this export").clicked()
                                {
                                    use_it = Some(p.id.clone());
                                }
                            });
                        } else if let Some(mut p) = app
                            .profiles
                            .user
                            .iter()
                            .find(|p| p.id == self.selected)
                            .cloned()
                        {
                            let before = p.clone();
                            edit_view(ui, &mut p);
                            ui.add_space(8.0);
                            let mut delete = false;
                            ui.horizontal(|ui| {
                                if ui.button("Use for this export").clicked() {
                                    use_it = Some(p.id.clone());
                                }
                                if ui
                                    .button(RichText::new("Delete").color(theme::DANGER))
                                    .clicked()
                                {
                                    delete = true;
                                }
                            });
                            if delete {
                                app.profiles.remove(&p.id);
                                let _ = app.profiles.save();
                                self.selected = "original".into();
                            } else if p != before {
                                app.profiles.upsert(p);
                                let _ = app.profiles.save();
                            }
                        }
                    });
                });
            });
        self.open = open && use_it.is_none();
        use_it
    }
}

fn row(ui: &mut egui::Ui, k: &str, v: impl Into<String>) {
    ui.label(RichText::new(k).color(MUTED));
    ui.add(egui::Label::new(v.into()).wrap());
    ui.end_row();
}

fn rate_text(r: &RateControl) -> String {
    match r {
        RateControl::Crf(q) => format!("Constant quality, CRF {q}"),
        RateControl::Bitrate(k) => format!("{k} kbit/s, two-pass"),
        RateControl::TargetSize(_) => {
            "Solved per export from the size you type (two-pass, size-checked)".into()
        }
    }
}

fn builtin_view(ui: &mut egui::Ui, p: &Profile) {
    ui.heading(&p.name);
    ui.label(theme::small(p.description.clone()));
    ui.add_space(6.0);
    egui::Grid::new("builtin").num_columns(2).spacing([16.0, 6.0]).max_col_width(400.0).show(ui, |ui| {
        row(ui, "Container", p.container.label());
        row(ui, "Video", p.video.codec.label());
        if p.is_copy() {
            row(ui, "Cuts", "Frame-exact: frames from each cut to the next keyframe are re-encoded (H.265/H.264), everything else is copied");
        } else if p.video.codec != VideoCodec::None {
            row(ui, "Rate control", rate_text(&p.video.rate));
            row(ui, "Encoder preset", p.video.preset.clone());
            row(ui, "Max height", p.video.max_height.map(|h| format!("{h}p")).unwrap_or("Source".into()));
            row(ui, "Max frame rate", p.video.max_fps.map(|f| format!("{f}")).unwrap_or("Source".into()));
            if let Some(g) = p.video.keyframe_interval {
                row(ui, "Keyframe interval", format!("{g} frames"));
            }
        }
        row(ui, "Audio", p.audio.codec.label());
        if !matches!(p.audio.codec, AudioCodec::Copy | AudioCodec::None) {
            row(ui, "Audio bitrate", format!("{} kbit/s per track", p.audio.bitrate_kbps));
        }
    });
    if p.is_fit() {
        ui.add_space(6.0);
        ui.label(theme::small(
            "Fit to size reserves 3% for rate-control error plus the container's index, gives audio a share that shrinks with the budget, \
             then picks the largest picture (and full or half frame rate) at which the remaining bitrate still looks good. \
             It encodes H.265 two-pass, checks the real size and re-runs the second pass lower if it came out too big.",
        ));
    }
}

fn edit_view(ui: &mut egui::Ui, p: &mut Profile) {
    egui::Grid::new("edit")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Name").color(MUTED));
            ui.text_edit_singleline(&mut p.name);
            ui.end_row();
            ui.label(RichText::new("Description").color(MUTED));
            ui.text_edit_singleline(&mut p.description);
            ui.end_row();

            ui.label(RichText::new("Container").color(MUTED));
            egui::ComboBox::from_id_salt("container")
                .selected_text(p.container.label())
                .show_ui(ui, |ui| {
                    for c in Container::ALL {
                        ui.selectable_value(&mut p.container, c, c.label());
                    }
                });
            ui.end_row();

            ui.label(RichText::new("Video").color(MUTED));
            egui::ComboBox::from_id_salt("vcodec")
                .selected_text(p.video.codec.label())
                .show_ui(ui, |ui| {
                    for c in VideoCodec::ALL {
                        ui.selectable_value(&mut p.video.codec, c, c.label());
                    }
                });
            ui.end_row();

            if !matches!(p.video.codec, VideoCodec::Copy | VideoCodec::None) {
                ui.label(RichText::new("Rate control").color(MUTED));
                ui.horizontal(|ui| {
                    let mut mode = match p.video.rate {
                        RateControl::Crf(_) => 0,
                        RateControl::Bitrate(_) => 1,
                        RateControl::TargetSize(_) => 2,
                    };
                    let old = mode;
                    ui.radio_value(&mut mode, 0, "CRF");
                    ui.radio_value(&mut mode, 1, "Bitrate");
                    ui.radio_value(&mut mode, 2, "Target size");
                    if mode != old {
                        p.video.rate = match mode {
                            0 => RateControl::Crf(23),
                            1 => RateControl::Bitrate(4000),
                            _ => RateControl::TargetSize(25.0),
                        };
                    }
                    match &mut p.video.rate {
                        RateControl::Crf(q) => {
                            ui.add(egui::DragValue::new(q).range(0..=51));
                        }
                        RateControl::Bitrate(k) => {
                            ui.add(
                                egui::DragValue::new(k)
                                    .range(50..=200_000)
                                    .suffix(" kbit/s"),
                            );
                        }
                        RateControl::TargetSize(mb) => {
                            ui.add(
                                egui::DragValue::new(mb)
                                    .range(0.5..=100_000.0)
                                    .suffix(" MB"),
                            );
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Encoder preset").color(MUTED));
                egui::ComboBox::from_id_salt("preset")
                    .selected_text(p.video.preset.clone())
                    .show_ui(ui, |ui| {
                        for s in PRESETS {
                            ui.selectable_value(&mut p.video.preset, s.to_string(), s);
                        }
                    });
                ui.end_row();

                ui.label(RichText::new("Max height").color(MUTED));
                let txt = |h: Option<u32>| h.map(|h| format!("{h}p")).unwrap_or("Source".into());
                egui::ComboBox::from_id_salt("height")
                    .selected_text(txt(p.video.max_height))
                    .show_ui(ui, |ui| {
                        for h in HEIGHTS {
                            ui.selectable_value(&mut p.video.max_height, h, txt(h));
                        }
                    });
                ui.end_row();

                ui.label(RichText::new("Max frame rate").color(MUTED));
                let ftxt = |f: Option<f64>| f.map(|f| format!("{f}")).unwrap_or("Source".into());
                egui::ComboBox::from_id_salt("fps")
                    .selected_text(ftxt(p.video.max_fps))
                    .show_ui(ui, |ui| {
                        for f in RATES {
                            ui.selectable_value(&mut p.video.max_fps, f, ftxt(f));
                        }
                    });
                ui.end_row();

                ui.label(RichText::new("Keyframe every").color(MUTED));
                ui.horizontal(|ui| {
                    let mut on = p.video.keyframe_interval.is_some();
                    ui.checkbox(&mut on, "");
                    if on {
                        let mut g = p.video.keyframe_interval.unwrap_or(60);
                        ui.add(
                            egui::DragValue::new(&mut g)
                                .range(1..=1000)
                                .suffix(" frames"),
                        );
                        p.video.keyframe_interval = Some(g);
                    } else {
                        p.video.keyframe_interval = None;
                        ui.label(theme::small("encoder default"));
                    }
                });
                ui.end_row();
            }

            ui.label(RichText::new("Audio").color(MUTED));
            egui::ComboBox::from_id_salt("acodec")
                .selected_text(p.audio.codec.label())
                .show_ui(ui, |ui| {
                    for c in AudioCodec::ALL {
                        ui.selectable_value(&mut p.audio.codec, c, c.label());
                    }
                });
            ui.end_row();
            if matches!(p.audio.codec, AudioCodec::Aac | AudioCodec::Opus) {
                ui.label(RichText::new("Audio bitrate").color(MUTED));
                ui.add(
                    egui::DragValue::new(&mut p.audio.bitrate_kbps)
                        .range(16..=512)
                        .suffix(" kbit/s"),
                );
                ui.end_row();
                ui.label(RichText::new("Channels").color(MUTED));
                let ctxt = |c: Option<u32>| match c {
                    None => "Source",
                    Some(1) => "Mono",
                    Some(2) => "Stereo",
                    _ => "Up to 7.1",
                };
                egui::ComboBox::from_id_salt("ch")
                    .selected_text(ctxt(p.audio.max_channels))
                    .show_ui(ui, |ui| {
                        for c in [None, Some(2), Some(1)] {
                            ui.selectable_value(&mut p.audio.max_channels, c, ctxt(c));
                        }
                    });
                ui.end_row();
            }
        });
    if p.video.codec == VideoCodec::Copy {
        ui.label(theme::small(
            "Copy keeps the original video; only the frames at each cut are re-encoded.",
        ));
    }
}

pub struct DetailsWindow {
    pub job: JobId,
}

impl DetailsWindow {
    /// Returns false when closed.
    pub fn show(&self, ctx: &egui::Context, app: &mut CutlineApp) -> bool {
        let Some(job) = app.jobs.iter().find(|j| j.id == self.job) else {
            return false;
        };
        let mut open = true;
        let centre = ctx.content_rect().center();
        egui::Window::new("Details")
            .open(&mut open)
            .default_size([760.0, 520.0])
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(centre)
            .collapsible(false)
            .show(ctx, |ui| {
                match &job.state {
                    JobState::Done { outputs, seconds } => {
                        for o in outputs {
                            ui.label(format!(
                                "Saved {} · {}",
                                o.path.display(),
                                format::size(o.bytes)
                            ));
                        }
                        ui.label(theme::small(format!("Took {}", format::length(*seconds))));
                    }
                    JobState::Failed { message } => {
                        ui.label(RichText::new(message.clone()).color(theme::DANGER));
                    }
                    _ => {}
                }
                for n in &job.details.notes {
                    ui.label(theme::small(format!("• {n}")));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ffmpeg commands").strong());
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(job.details.commands.join("\n"));
                    }
                });
                let cmds = job.details.commands.join("\n\n");
                egui::ScrollArea::vertical()
                    .id_salt("cmds")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut cmds.as_str())
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Log").strong());
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(job.details.log.clone());
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("log")
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let log = if job.details.log.trim().is_empty() {
                            "(ffmpeg printed nothing)"
                        } else {
                            job.details.log.as_str()
                        };
                        ui.add(
                            egui::TextEdit::multiline(&mut &*log)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
            });
        open
    }
}
