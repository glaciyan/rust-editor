//! The export dialog: three plain choices, then Save. Only megabytes and
//! minutes are shown; codecs and the like live in Advanced….

use crate::app::CutlineApp;
use crate::theme::{self, ACCENT, ACCENT_BG, BORDER, INK, MUTED};
use cutline_core::{format, Profile, Quality, RateControl};
use cutline_engine::estimate::{self, calibration_key, FitEstimate};
use cutline_engine::request::{default_name, free_name, ExportRequest};
use eframe::egui::{self, Key, Stroke};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    Original,
    Fit,
    Preset,
}

pub enum ExportOutcome {
    Open,
    Closed,
    Advanced,
}

pub struct ExportDialog {
    choice: Choice,
    fit_text: String,
    preset_id: String,
    name: String,
    folder: String,
    separate: bool,
    audio: Vec<bool>,
    conflict: Option<Vec<PathBuf>>,
    folder_rx: Option<Receiver<Option<PathBuf>>>,
    error: Option<String>,
    focus_once: bool,
}

const SIZE_CHIPS: [u32; 4] = [8, 10, 25, 50];

impl ExportDialog {
    pub fn new(app: &CutlineApp) -> Self {
        let first = &app.project.clips[0].path;
        let folder = first
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let tracks = app.sources.get(first).map(|m| m.audio.len()).unwrap_or(0);
        let original_mb = {
            let segs: Vec<_> = app.project.segments();
            let refs: Vec<_> = segs.iter().collect();
            cutline_engine::plan::estimate_copy_bytes(&refs, &app.sources, &[0]) / 1e6
        };
        // A sensible first size: the classic 25 MB, or half the original if that's already small.
        let fit = app.last_fit_mb.unwrap_or(if original_mb > 50.0 {
            25.0
        } else {
            (original_mb / 2.0).round().max(1.0)
        });
        ExportDialog {
            choice: Choice::Original,
            fit_text: trim_num(fit),
            preset_id: "smaller".into(),
            name: default_name(first),
            folder: folder.to_string_lossy().into_owned(),
            separate: false,
            audio: (0..tracks).map(|i| i == 0).collect(),
            conflict: None,
            folder_rx: None,
            error: None,
            focus_once: true,
        }
    }

    pub fn preset_id(&self) -> Option<String> {
        (self.choice == Choice::Preset).then(|| self.preset_id.clone())
    }

    pub fn use_preset(&mut self, id: &str) {
        self.preset_id = id.to_string();
        self.choice = Choice::Preset;
    }

    fn fit_mb(&self) -> Option<f64> {
        self.fit_text
            .trim()
            .trim_end_matches("MB")
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|v| *v > 0.0 && v.is_finite())
    }

    fn request(&self, app: &CutlineApp, profile: Profile) -> ExportRequest {
        ExportRequest {
            segments: app.project.segments(),
            profile,
            output_dir: PathBuf::from(self.folder.trim()),
            name: self.name.trim().to_string(),
            separate: self.separate,
            audio_tracks: self
                .audio
                .iter()
                .enumerate()
                .filter(|(_, on)| **on)
                .map(|(i, _)| i)
                .collect(),
            overwrite: false,
        }
    }

    fn preset(&self, app: &CutlineApp) -> Profile {
        let all = app.profiles.presets();
        all.iter()
            .find(|p| p.id == self.preset_id)
            .cloned()
            .unwrap_or_else(|| all[0].clone())
    }

    /// The profile Save will use, or None if it can't (e.g. size too small).
    fn chosen_profile(&self, app: &CutlineApp, fit: &Option<FitEstimate>) -> Option<Profile> {
        match self.choice {
            Choice::Original => Some(Profile::original()),
            Choice::Fit => match fit {
                Some(FitEstimate::Original(_)) => Some(Profile::original()),
                Some(FitEstimate::Encode(_)) => Some(Profile::fit(self.fit_mb()?)),
                _ => None,
            },
            Choice::Preset => Some(self.preset(app)),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, app: &mut CutlineApp) -> ExportOutcome {
        let mut outcome = ExportOutcome::Open;
        if let Some(rx) = &self.folder_rx {
            if let Ok(r) = rx.try_recv() {
                if let Some(p) = r {
                    self.folder = p.to_string_lossy().into_owned();
                    self.conflict = None;
                }
                self.folder_rx = None;
            }
        }
        let Some(tools) = app.tools.clone() else {
            return ExportOutcome::Closed;
        };
        let caps = &tools.caps;
        let segments = app.project.segments();
        let total: f64 = segments.iter().map(|s| s.len().secs()).sum();

        // Estimates (cheap; recomputed every frame so typing a size is live).
        let orig_req = self.request(app, Profile::original());
        let orig_possible = cutline_engine::plan::copy_possible(&orig_req, &app.sources, caps);
        let orig_cal = if orig_possible {
            None
        } else {
            app.calibrations
                .get(&calibration_key(&orig_req))
                .copied()
                .flatten()
        };
        let orig = estimate::original(&orig_req, &app.sources, caps, orig_cal);
        let fit_mb = self.fit_mb();
        let fit_req = fit_mb.map(|mb| self.request(app, Profile::fit(mb)));
        let fit_key = fit_req.as_ref().map(fit_cal_key);
        let fit = fit_req.as_ref().map(|r| {
            let cal = fit_key.and_then(|k| app.calibrations.get(&k).copied().flatten());
            estimate::fit(r, &app.sources, caps, fit_mb.unwrap_or(1.0), cal)
        });
        let preset = self.preset(app);
        let preset_req = self.request(app, preset.clone());
        let preset_key = calibration_key(&preset_req);
        let preset_est = estimate::encode(
            &preset_req,
            &app.sources,
            app.calibrations.get(&preset_key).copied().flatten(),
        );

        // Calibrate only what's selected, so samples don't fight each other.
        match self.choice {
            Choice::Original if !orig_possible => {
                app.request_calibration(calibration_key(&orig_req), orig_req.clone())
            }
            Choice::Fit => {
                if let (Some(k), Some(r), Some(FitEstimate::Encode(_))) =
                    (fit_key, fit_req.clone(), &fit)
                {
                    app.request_calibration(k, r);
                }
            }
            Choice::Preset => app.request_calibration(preset_key, preset_req.clone()),
            _ => {}
        }

        let first_audio: Vec<String> = app
            .sources
            .get(&app.project.clips[0].path)
            .map(|m| {
                m.audio
                    .iter()
                    .enumerate()
                    .map(|(i, a)| a.label(i))
                    .collect()
            })
            .unwrap_or_default();
        let presets = app.profiles.presets();
        let width = (ctx.content_rect().width() - 48.0).clamp(480.0, 800.0);
        let mut save = false;
        let mut save_mode: Option<bool> = None; // Some(true) = replace, Some(false) = keep both

        let modal = egui::Modal::new(egui::Id::unique("export")).frame(
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(10)
                .inner_margin(0),
        );
        let resp = modal.show(ctx, |ui| {
            ui.set_width(width);
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 20,
                    bottom: 0,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Export").size(17.0).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let n = segments.len();
                            ui.label(theme::small(format!(
                                "{} part{} · {}",
                                n,
                                if n == 1 { "" } else { "s" },
                                format::length(total)
                            )));
                        });
                    });
                });
            egui::Frame::new().inner_margin(16).show(ui, |ui| {
                ui.columns(3, |cols| {
                    // Original quality
                    self.choice = card(
                        &mut cols[0],
                        self.choice == Choice::Original,
                        Choice::Original,
                        |ui| {
                            let mut c = self.choice;
                            ui.radio_value(
                                &mut c,
                                Choice::Original,
                                egui::RichText::new("Original quality").strong(),
                            );
                            ui.label(
                                egui::RichText::new("Same quality as your video. Fastest.")
                                    .size(12.0),
                            );
                            if let Some(n) = &orig.note {
                                ui.label(theme::small(n.clone()));
                            }
                            bottom(ui, |ui| {
                                ui.label(format::about_size(orig.bytes));
                                ui.label(theme::small(format::ready_in(orig.seconds)));
                            });
                            c
                        },
                    );

                    // Fit to size
                    self.choice = card(
                        &mut cols[1],
                        self.choice == Choice::Fit,
                        Choice::Fit,
                        |ui| {
                            let mut c = self.choice;
                            ui.radio_value(
                                &mut c,
                                Choice::Fit,
                                egui::RichText::new("Fit to size").strong(),
                            );
                            ui.horizontal(|ui| {
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut self.fit_text)
                                        .desired_width(64.0)
                                        .font(egui::FontId::proportional(16.0))
                                        .margin(egui::vec2(8.0, 6.0)),
                                );
                                if r.gained_focus() || r.changed() {
                                    c = Choice::Fit;
                                }
                                ui.label("MB");
                            });
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                for (i, mb) in SIZE_CHIPS.iter().enumerate() {
                                    if i > 0 {
                                        ui.label(theme::small("·"));
                                    }
                                    let chip = egui::Button::new(
                                        egui::RichText::new(mb.to_string()).size(12.0).color(MUTED),
                                    )
                                    .frame(false);
                                    if ui.add(chip).on_hover_text(format!("{mb} MB")).clicked() {
                                        self.fit_text = mb.to_string();
                                        c = Choice::Fit;
                                    }
                                }
                                ui.label(theme::small("MB"));
                            });
                            bottom(ui, |ui| match &fit {
                                None => {
                                    ui.label(theme::small("Type a size in MB"));
                                }
                                Some(FitEstimate::TooSmall { min_mb }) => {
                                    ui.label(
                                        egui::RichText::new("Too small for this length")
                                            .color(theme::DANGER),
                                    );
                                    ui.label(theme::small(format!("Try at least {min_mb} MB")));
                                }
                                Some(FitEstimate::Original(e)) => {
                                    quality_row(ui, Quality::Great);
                                    ui.label(theme::small(format!(
                                        "Already fits · {}",
                                        format::ready_in(e.seconds)
                                    )));
                                }
                                Some(FitEstimate::Encode(e)) => {
                                    quality_row(ui, e.quality.unwrap_or(Quality::Good));
                                    ui.label(
                                        egui::RichText::new(format::ready_in(e.seconds))
                                            .size(12.0)
                                            .color(egui::Color32::from_rgb(0x4f, 0x4e, 0x4a)),
                                    );
                                }
                            });
                            c
                        },
                    );

                    // Presets
                    self.choice = card(
                        &mut cols[2],
                        self.choice == Choice::Preset,
                        Choice::Preset,
                        |ui| {
                            let mut c = self.choice;
                            ui.radio_value(
                                &mut c,
                                Choice::Preset,
                                egui::RichText::new("Presets").strong(),
                            );
                            let before = self.preset_id.clone();
                            let combo = egui::ComboBox::from_id_salt("preset")
                                .width(ui.available_width())
                                .selected_text(preset.name.clone())
                                .show_ui(ui, |ui| {
                                    for p in &presets {
                                        ui.selectable_value(
                                            &mut self.preset_id,
                                            p.id.clone(),
                                            p.name.clone(),
                                        );
                                    }
                                });
                            if self.preset_id != before || combo.response.clicked() {
                                c = Choice::Preset;
                            }
                            ui.label(egui::RichText::new(preset.description.clone()).size(12.0));
                            bottom(ui, |ui| {
                                ui.label(format::about_size(preset_est.bytes));
                                ui.label(theme::small(format::ready_in(preset_est.seconds)));
                            });
                            c
                        },
                    );
                });

                ui.add_space(12.0);
                egui::Grid::new("export-fields")
                    .num_columns(2)
                    .spacing([16.0, 12.0])
                    .min_col_width(80.0)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Name").color(MUTED));
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.name)
                                .desired_width(f32::INFINITY)
                                .margin(egui::vec2(10.0, 8.0)),
                        );
                        if self.focus_once {
                            r.request_focus();
                            self.focus_once = false;
                        }
                        if r.changed() {
                            self.conflict = None;
                        }
                        ui.end_row();
                        ui.label(egui::RichText::new("Save to").color(MUTED));
                        ui.horizontal(|ui| {
                            let w = ui.available_width() - 96.0;
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.folder)
                                        .desired_width(w)
                                        .margin(egui::vec2(10.0, 8.0)),
                                )
                                .changed()
                            {
                                self.conflict = None;
                            }
                            if ui.button("Choose…").clicked() && self.folder_rx.is_none() {
                                let (tx, rx) = channel();
                                let start = PathBuf::from(&self.folder);
                                std::thread::spawn(move || {
                                    let _ = tx.send(
                                        rfd::FileDialog::new()
                                            .set_title("Save to")
                                            .set_directory(start)
                                            .pick_folder(),
                                    );
                                });
                                self.folder_rx = Some(rx);
                            }
                        });
                        ui.end_row();
                        if segments.len() > 1 {
                            ui.label(egui::RichText::new("Parts").color(MUTED));
                            ui.horizontal(|ui| {
                                if theme::pill(ui, !self.separate, "One video").clicked() {
                                    self.separate = false;
                                    self.conflict = None;
                                }
                                if theme::pill(ui, self.separate, "Separate videos").clicked() {
                                    self.separate = true;
                                    self.conflict = None;
                                }
                            });
                            ui.end_row();
                        }
                        if first_audio.len() > 1 {
                            ui.label(egui::RichText::new("Audio").color(MUTED));
                            ui.horizontal(|ui| {
                                for (i, name) in first_audio.iter().enumerate() {
                                    if let Some(on) = self.audio.get_mut(i) {
                                        ui.checkbox(on, name.clone());
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    });

                if let Some(files) = &self.conflict {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(theme::WELL)
                        .corner_radius(8)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let what = if files.len() == 1 {
                                    format!(
                                        "“{}” already exists.",
                                        files[0].file_name().unwrap_or_default().to_string_lossy()
                                    )
                                } else {
                                    format!("{} of these files already exist.", files.len())
                                };
                                ui.label(what);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("Replace").clicked() {
                                            save_mode = Some(true);
                                        }
                                        if ui.button("Keep both").clicked() {
                                            save_mode = Some(false);
                                        }
                                    },
                                );
                            });
                        });
                }
                if let Some(e) = &self.error {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(e.clone()).color(theme::DANGER));
                }
            });
            let r = ui.max_rect();
            ui.painter().hline(
                r.x_range(),
                ui.cursor().top(),
                Stroke::new(1.0, theme::HAIRLINE),
            );
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(16, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Advanced…").size(13.0).color(MUTED),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            outcome = ExportOutcome::Advanced;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let can_save = self.chosen_profile(app, &fit).is_some()
                                && !self.name.trim().is_empty();
                            if ui
                                .add_enabled(
                                    can_save,
                                    theme::primary("Save").min_size(egui::vec2(104.0, 44.0)),
                                )
                                .clicked()
                            {
                                save = true;
                            }
                            if ui
                                .add(
                                    egui::Button::new("Cancel")
                                        .min_size(egui::vec2(96.0, 44.0))
                                        .corner_radius(8),
                                )
                                .clicked()
                            {
                                outcome = ExportOutcome::Closed;
                            }
                        });
                    });
                });
        });
        if resp.should_close() && !matches!(outcome, ExportOutcome::Advanced) {
            outcome = ExportOutcome::Closed;
        }
        if ctx.input(|i| i.key_pressed(Key::Enter))
            && self.conflict.is_none()
            && !ctx.any_popup_open()
        {
            save = true;
        }

        if save || save_mode.is_some() {
            let Some(profile) = self.chosen_profile(app, &fit) else {
                return outcome;
            };
            match self.submit(app, profile, save_mode) {
                Ok(true) => return ExportOutcome::Closed,
                Ok(false) => {}
                Err(e) => self.error = Some(e),
            }
        }
        outcome
    }

    /// Ok(true) when the job was queued; Ok(false) when waiting for the user to resolve a name clash.
    fn submit(
        &mut self,
        app: &mut CutlineApp,
        profile: Profile,
        mode: Option<bool>,
    ) -> Result<bool, String> {
        let name = self.name.trim();
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err("Give the video a name (without slashes).".into());
        }
        let folder = PathBuf::from(shell_home(self.folder.trim()));
        if !folder.is_dir() {
            return Err("That folder doesn't exist. Pick one with Choose….".into());
        }
        if self.audio.len() > 1 && !self.audio.iter().any(|a| *a) && profile.audio_only() {
            return Err("Pick at least one audio track for an audio file.".into());
        }
        let mut req = self.request(app, profile);
        req.output_dir = folder.clone();
        let first = app.sources.get(&req.segments[0].path);
        let ext = req.extension(first);
        let mut outputs = req.output_paths(&ext);
        // Never export over a source.
        let sources: Vec<PathBuf> = req.segments.iter().map(|s| s.path.clone()).collect();
        let clashes: Vec<PathBuf> = outputs.iter().filter(|p| p.exists()).cloned().collect();
        match mode {
            None if !clashes.is_empty() => {
                self.conflict = Some(clashes);
                return Ok(false);
            }
            Some(true) => {
                if outputs
                    .iter()
                    .any(|o| sources.iter().any(|s| same_file(s, o)))
                {
                    return Err(
                        "That would replace the video you're cutting. Pick another name.".into(),
                    );
                }
                req.overwrite = true;
            }
            Some(false) => {
                req.name = free_name(&folder, &req.name, &ext, req.separate, req.segments.len());
                self.name = req.name.clone();
                outputs = req.output_paths(&ext);
            }
            None => {}
        }
        if let RateControl::TargetSize(mb) = req.profile.video.rate {
            app.last_fit_mb = Some(mb);
        }
        app.last_export_dir = Some(folder);
        if let Some(e) = &app.engine {
            e.submit(req, outputs);
        }
        Ok(true)
    }
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn shell_home(s: &str) -> String {
    match (s.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => format!("{h}/{rest}"),
        _ => s.to_string(),
    }
}

/// Calibrations for fit depend on the picture size the solver picks, not the exact MB.
fn fit_cal_key(r: &ExportRequest) -> u64 {
    let mut n = r.clone();
    n.profile.video.rate = RateControl::TargetSize(0.0);
    calibration_key(&n) ^ 0x5157
}

fn trim_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// One of the three choice cards. Clicking anywhere on it selects it.
/// The background click area is registered before the contents (using last
/// frame's rect) so the radio, text box and menu inside still get their clicks.
fn card(
    ui: &mut egui::Ui,
    on: bool,
    me: Choice,
    add: impl FnOnce(&mut egui::Ui) -> Choice,
) -> Choice {
    let id = ui.scope_id().with(("card", me as u8));
    let prev: Option<egui::Rect> = ui.ctx().data(|d| d.get_temp(id));
    let bg_clicked = prev
        .map(|r| ui.interact(r, id, egui::Sense::click()).clicked())
        .unwrap_or(false);
    let frame = egui::Frame::new()
        .fill(if on { ACCENT_BG } else { theme::SURFACE })
        .stroke(if on {
            Stroke::new(2.0, ACCENT)
        } else {
            Stroke::new(1.0, BORDER)
        })
        .corner_radius(8)
        .inner_margin(if on { 15 } else { 16 });
    let r = frame.show(ui, |ui| {
        ui.set_min_height(CARD_H);
        ui.spacing_mut().item_spacing.y = 6.0;
        add(ui)
    });
    ui.ctx().data_mut(|d| d.insert_temp(id, r.response.rect));
    if bg_clicked {
        me
    } else {
        r.inner
    }
}

/// Inner height of every card, so the three line up.
const CARD_H: f32 = 172.0;

/// Push the estimate lines to the bottom of a card.
fn bottom(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    // Measure from the cursor: min_rect already includes the card's minimum height.
    let used = ui.cursor().top() - ui.min_rect().top();
    let space = (CARD_H - used - 46.0).max(4.0);
    ui.add_space(space);
    add(ui);
}

fn quality_row(ui: &mut egui::Ui, q: Quality) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(q.label()).strong().color(INK));
        theme::quality_bars(ui, q.bars());
    });
}
