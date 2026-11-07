//! The export dialog: one list of profiles (the built-in ones, then yours),
//! each with how big it comes out and how long it takes, then a name and a
//! folder. Codecs only show up in New profile….

use crate::app::CutlineApp;
use crate::icons::Icon;
use crate::modal::{self, Dialog, Outcome};
use crate::profile::ProfileDialog;
use crate::theme::{self, ACCENT, BORDER, MUTED};
use cutline_core::{format, Profile, Quality, RateControl, FIT_ID, ORIGINAL_ID};
use cutline_engine::estimate::{self, calibration_key, Estimate, FitEstimate};
use cutline_engine::request::{default_name, free_name, ExportRequest};
use eframe::egui::{self, Color32, Role, Sense, Stroke, WidgetInfo};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

/// Width of the Choose… button, so Save to lines up with Name.
const CHOOSE_W: f32 = 100.0;

pub struct ExportDialog {
    /// The picked row: `original`, `fit`, or a preset's id.
    picked: String,
    fit_text: String,
    name: String,
    folder: String,
    separate: bool,
    audio: Vec<bool>,
    conflict: Option<Vec<PathBuf>>,
    folder_rx: Option<Receiver<Option<PathBuf>>>,
    error: Option<String>,
}

/// What a row shows on its right.
enum RowInfo {
    Estimate(Estimate),
    Fit(Option<FitEstimate>, f64),
}

impl ExportDialog {
    pub fn new(app: &CutlineApp) -> Self {
        let first = &app.project.clips[0].path;
        let folder = app
            .last_export_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| first.parent().map(|p| p.to_path_buf()))
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
            picked: ORIGINAL_ID.into(),
            fit_text: trim_num(fit),
            name: default_name(first),
            folder: folder.to_string_lossy().into_owned(),
            separate: false,
            audio: (0..tracks).map(|i| i == 0).collect(),
            conflict: None,
            folder_rx: None,
            error: None,
        }
    }

    /// Pick a row (a profile just made in New profile…).
    pub fn select(&mut self, id: &str) {
        self.picked = id.to_string();
        self.conflict = None;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn picked(&self) -> &str {
        &self.picked
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

    /// The profile Export will use, or None if it can't (e.g. size too small).
    fn chosen_profile(&self, app: &CutlineApp, fit: &Option<FitEstimate>) -> Option<Profile> {
        match self.picked.as_str() {
            ORIGINAL_ID => Some(Profile::original()),
            FIT_ID => match fit {
                Some(FitEstimate::Original(_)) => Some(Profile::original()),
                Some(FitEstimate::Encode(_)) => Some(Profile::fit(self.fit_mb()?)),
                _ => None,
            },
            id => app.profiles.presets().into_iter().find(|p| p.id == id),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, app: &mut CutlineApp, enter: bool) -> Outcome {
        let mut outcome = Outcome::Stay;
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
            return Outcome::Close;
        };
        let caps = &tools.caps;
        let segments = app.project.segments();
        let total: f64 = segments.iter().map(|s| s.len().secs()).sum();
        // A deleted profile can't stay picked.
        let presets = app.profiles.presets();
        if self.picked != ORIGINAL_ID
            && self.picked != FIT_ID
            && !presets.iter().any(|p| p.id == self.picked)
        {
            self.picked = ORIGINAL_ID.into();
        }

        // Estimates for every row (cheap; recomputed every frame so typing a size is live).
        let orig_req = self.request(app, Profile::original());
        let orig_possible = cutline_engine::plan::copy_possible(&orig_req, &app.sources, caps);
        let orig_key = calibration_key(&orig_req);
        let orig_cal = if orig_possible {
            None
        } else {
            app.calibrations.get(&orig_key).copied().flatten()
        };
        let orig = estimate::original(&orig_req, &app.sources, caps, orig_cal);
        let fit_mb = self.fit_mb();
        let fit_req = fit_mb.map(|mb| self.request(app, Profile::fit(mb)));
        let fit_key = fit_req.as_ref().map(fit_cal_key);
        let fit = fit_req.as_ref().map(|r| {
            let cal = fit_key.and_then(|k| app.calibrations.get(&k).copied().flatten());
            estimate::fit(r, &app.sources, caps, fit_mb.unwrap_or(1.0), cal)
        });
        let preset_rows: Vec<(Profile, RowInfo, u64, ExportRequest)> = presets
            .iter()
            .map(|p| {
                let req = self.request(app, p.clone());
                let (key, info) = match p.video.rate {
                    RateControl::TargetSize(mb) if p.is_fit() => {
                        let k = fit_cal_key(&req);
                        let cal = app.calibrations.get(&k).copied().flatten();
                        (
                            k,
                            RowInfo::Fit(
                                Some(estimate::fit(&req, &app.sources, caps, mb, cal)),
                                mb,
                            ),
                        )
                    }
                    _ => {
                        let k = calibration_key(&req);
                        let cal = app.calibrations.get(&k).copied().flatten();
                        let e = if p.is_copy() {
                            estimate::original(&req, &app.sources, caps, cal)
                        } else {
                            estimate::encode(&req, &app.sources, cal)
                        };
                        (k, RowInfo::Estimate(e))
                    }
                };
                (p.clone(), info, key, req)
            })
            .collect();

        // Calibrate only what's picked, so samples don't fight each other.
        match self.picked.as_str() {
            ORIGINAL_ID if !orig_possible => app.request_calibration(orig_key, orig_req.clone()),
            ORIGINAL_ID => {}
            FIT_ID => {
                if let (Some(k), Some(r), Some(FitEstimate::Encode(_))) =
                    (fit_key, fit_req.clone(), &fit)
                {
                    app.request_calibration(k, r);
                }
            }
            id => {
                if let Some((_, info, key, req)) = preset_rows.iter().find(|r| r.0.id == id) {
                    let wants = match info {
                        RowInfo::Fit(f, _) => matches!(f, Some(FitEstimate::Encode(_))),
                        RowInfo::Estimate(_) => true,
                    };
                    if wants {
                        app.request_calibration(*key, req.clone());
                    }
                }
            }
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
        let chosen = self.chosen_profile(app, &fit);
        let ext = {
            let p = chosen.clone().unwrap_or_else(|| Profile::fit(1.0));
            let req = self.request(app, p);
            format!(".{}", req.extension(app.sources.get(&segments[0].path)))
        };
        let n = segments.len();
        let subtitle = match (n, self.separate) {
            (1, _) => format!("1 part · {}", format::length(total)),
            (n, false) => format!("{n} parts joined · {}", format::length(total)),
            (n, true) => format!("{n} parts as separate videos · {}", format::length(total)),
        };
        let mut save = false;
        let mut save_mode: Option<bool> = None; // Some(true) = replace, Some(false) = keep both
        let mut edit_profile: Option<String> = None;
        let mut delete_profile: Option<String> = None;

        modal::modal(ctx, "export", 600.0, |ui| {
            if modal::header(ui, "Export", Some(&subtitle)) {
                outcome = Outcome::Close;
            }
            modal::body(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 14.0;
                // The profile list.
                let group = ui
                    .vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let fit_status = fit_status(&fit);
                        let builtin: Vec<&(Profile, RowInfo, u64, ExportRequest)> =
                            preset_rows.iter().filter(|r| r.0.builtin).collect();
                        let yours: Vec<&(Profile, RowInfo, u64, ExportRequest)> =
                            preset_rows.iter().filter(|r| !r.0.builtin).collect();
                        let last_builtin = yours.is_empty();
                        // Original quality.
                        let r = row(
                            ui,
                            RowSpec {
                                name: "Original quality",
                                desc: "Copies the video, re-encodes only around cuts",
                                selected: self.picked == ORIGINAL_ID,
                                first: true,
                                last: false,
                                tall: false,
                            },
                            |ui, r| estimate_lines(ui, r, &orig),
                        );
                        if let Some(note) = &orig.note {
                            r.clone().on_hover_text(note.clone());
                        }
                        if r.clicked() {
                            self.picked = ORIGINAL_ID.into();
                        }
                        // Fit to size, with its size box.
                        let mut focus_fit = false;
                        let r = row(
                            ui,
                            RowSpec {
                                name: "Fit to size",
                                desc: "Best quality that stays under a size",
                                selected: self.picked == FIT_ID,
                                first: false,
                                last: false,
                                tall: true,
                            },
                            |ui, r| {
                                // Size box and `MB` on top, the verdict under them.
                                let (text, color) = fit_status;
                                let status = ui.painter().layout_no_wrap(
                                    text,
                                    egui::FontId::proportional(11.5),
                                    color,
                                );
                                let mb = ui.painter().layout_no_wrap(
                                    "MB".into(),
                                    egui::FontId::proportional(12.5),
                                    MUTED,
                                );
                                let field_h = 26.0;
                                let block = field_h + 3.0 + status.size().y;
                                let top = r.center().y - block / 2.0;
                                let mb_x = r.right() - mb.size().x;
                                let mb_h = mb.size().y;
                                ui.painter().galley(
                                    egui::pos2(mb_x, top + (field_h - mb_h) / 2.0),
                                    mb,
                                    MUTED,
                                );
                                let field = egui::Rect::from_min_size(
                                    egui::pos2(mb_x - 6.0 - 54.0, top),
                                    egui::vec2(54.0, field_h),
                                );
                                let mut child = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(field)
                                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                );
                                let f = child.add(
                                    theme::Field::new(&mut self.fit_text, "Target size in MB")
                                        .width(54.0)
                                        .height(field_h)
                                        .right_align(),
                                );
                                if f.gained_focus() || f.changed() || f.clicked() {
                                    focus_fit = true;
                                }
                                let sw = status.size().x;
                                ui.painter().galley(
                                    egui::pos2(r.right() - sw, top + field_h + 3.0),
                                    status,
                                    color,
                                );
                            },
                        );
                        if r.clicked() || focus_fit {
                            self.picked = FIT_ID.into();
                        }
                        for (i, (p, info, _, _)) in
                            builtin.iter().map(|r| (&r.0, &r.1, r.2, &r.3)).enumerate()
                        {
                            let r = row(
                                ui,
                                RowSpec {
                                    name: &p.name,
                                    desc: p.description.trim_end_matches('.'),
                                    selected: self.picked == p.id,
                                    first: false,
                                    last: last_builtin && i + 1 == builtin.len(),
                                    tall: false,
                                },
                                |ui, r| info_lines(ui, r, info),
                            );
                            if r.clicked() {
                                self.picked = p.id.clone();
                            }
                        }
                        if !yours.is_empty() {
                            section(ui, "Yours");
                            for (i, (p, info, _, _)) in
                                yours.iter().map(|r| (&r.0, &r.1, r.2, &r.3)).enumerate()
                            {
                                let r = row(
                                    ui,
                                    RowSpec {
                                        name: &p.name,
                                        desc: &p.description,
                                        selected: self.picked == p.id,
                                        first: false,
                                        last: i + 1 == yours.len(),
                                        tall: false,
                                    },
                                    |ui, r| info_lines(ui, r, info),
                                );
                                if r.clicked() {
                                    self.picked = p.id.clone();
                                }
                                r.context_menu(|ui| {
                                    theme::menu(ui);
                                    if theme::menu_item(ui, Icon::Pencil, "Edit…", None).clicked()
                                    {
                                        edit_profile = Some(p.id.clone());
                                        ui.close();
                                    }
                                    if theme::menu_danger(ui, Icon::Trash, "Delete", None).clicked()
                                    {
                                        delete_profile = Some(p.id.clone());
                                        ui.close();
                                    }
                                });
                            }
                        }
                    })
                    .response
                    .rect;
                ui.painter().rect_stroke(
                    group,
                    8,
                    Stroke::new(1.0, BORDER),
                    egui::StrokeKind::Inside,
                );

                // Name and folder.
                let field_w = ui.available_width() - 64.0 - 10.0;
                egui::Grid::new("export-fields")
                    .num_columns(2)
                    .spacing([10.0, 10.0])
                    .min_col_width(64.0)
                    .show(ui, |ui| {
                        ui.label(theme::form_label("Name"));
                        let r = ui.add(
                            theme::Field::new(&mut self.name, "Name")
                                .suffix(&ext)
                                .width(field_w),
                        );
                        if r.changed() {
                            self.conflict = None;
                        }
                        ui.end_row();
                        ui.label(theme::form_label("Save to"));
                        ui.horizontal(|ui| {
                            let w = field_w - CHOOSE_W - ui.spacing().item_spacing.x;
                            if ui
                                .add(theme::Field::new(&mut self.folder, "Save to").width(w))
                                .changed()
                            {
                                self.conflict = None;
                            }
                            if ui
                                .add(
                                    theme::secondary("Choose…")
                                        .icon(Icon::Folder)
                                        .min_width(CHOOSE_W),
                                )
                                .clicked()
                                && self.folder_rx.is_none()
                            {
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
                        if n > 1 {
                            ui.label(theme::form_label("Parts"));
                            if let Some(i) = theme::segmented(
                                ui,
                                "parts",
                                &["One video", "Separate videos"],
                                self.separate as usize,
                                None,
                            ) {
                                self.separate = i == 1;
                                self.conflict = None;
                            }
                            ui.end_row();
                        }
                        if first_audio.len() > 1 {
                            ui.label(theme::form_label("Audio"));
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
                    egui::Frame::new()
                        .fill(theme::BG)
                        .stroke(Stroke::new(1.0, theme::FIELD_FOCUS))
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
                                        if ui.add(theme::secondary("Replace")).clicked() {
                                            save_mode = Some(true);
                                        }
                                        if ui.add(theme::secondary("Keep both")).clicked() {
                                            save_mode = Some(false);
                                        }
                                    },
                                );
                            });
                        });
                }
                if let Some(e) = &self.error {
                    ui.label(egui::RichText::new(e.clone()).color(theme::DANGER));
                }
            });
            modal::footer(ui, |ui| {
                ui.add_space(-10.0);
                if ui
                    .add(theme::ghost("New profile…").icon(Icon::Sliders))
                    .on_hover_text("Pick the codec, size and frame rate yourself")
                    .clicked()
                {
                    outcome =
                        Outcome::Push(Dialog::Profile(Box::new(ProfileDialog::new(app, None))));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_save = chosen.is_some() && !self.name.trim().is_empty();
                    if ui
                        .add_enabled(can_save, theme::primary("Export").icon(Icon::Check))
                        .clicked()
                    {
                        save = true;
                    }
                    if ui.add(theme::secondary("Cancel")).clicked() {
                        outcome = Outcome::Close;
                    }
                });
            });
        });

        if let Some(id) = edit_profile {
            outcome = Outcome::Push(Dialog::Profile(Box::new(ProfileDialog::new(
                app,
                Some(&id),
            ))));
        }
        if let Some(id) = delete_profile {
            app.profiles.remove(&id);
            app.save_profiles();
            if self.picked == id {
                self.picked = ORIGINAL_ID.into();
            }
        }
        if enter && self.conflict.is_none() {
            save = true;
        }
        if (save || save_mode.is_some()) && matches!(outcome, Outcome::Stay) {
            let Some(profile) = self.chosen_profile(app, &fit) else {
                return outcome;
            };
            match self.submit(app, profile, save_mode) {
                Ok(true) => return Outcome::Close,
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
        let folder = modal::shellexpand_home(self.folder.trim());
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
        app.submit_export(req, outputs);
        Ok(true)
    }
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
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

/// The line under Fit to size's box, and its colour.
fn fit_status(fit: &Option<FitEstimate>) -> (String, Color32) {
    match fit {
        None => ("Type a size in MB".into(), MUTED),
        Some(FitEstimate::TooSmall { min_mb }) => {
            (format!("Too small · try {min_mb} MB"), theme::DANGER)
        }
        Some(FitEstimate::Original(e)) => (
            format!("Already fits · {}", format::ready_in(e.seconds)),
            theme::SUCCESS,
        ),
        Some(FitEstimate::Encode(e)) => (
            format!(
                "{} · {}",
                quality_words(e.quality.unwrap_or(Quality::Good)),
                format::ready_in(e.seconds)
            ),
            theme::SUCCESS,
        ),
    }
}

fn quality_words(q: Quality) -> String {
    q.label().to_string()
}

struct RowSpec<'a> {
    name: &'a str,
    desc: &'a str,
    selected: bool,
    first: bool,
    last: bool,
    /// Room for Fit to size's box and the line under it.
    tall: bool,
}

/// One radio row: ring, name over a short description, and `right` on the
/// right. Clicking anywhere on it picks it; controls inside still get their
/// own clicks because they're laid on top.
fn row(
    ui: &mut egui::Ui,
    spec: RowSpec,
    right: impl FnOnce(&mut egui::Ui, egui::Rect),
) -> egui::Response {
    let h = if spec.tall { 64.0 } else { 52.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, enabled, spec.selected, spec.name));
    let p = ui.painter().clone();
    let radius = egui::CornerRadius {
        nw: if spec.first { 8 } else { 0 },
        ne: if spec.first { 8 } else { 0 },
        sw: if spec.last { 8 } else { 0 },
        se: if spec.last { 8 } else { 0 },
    };
    if spec.selected {
        p.rect_filled(rect, radius, theme::ACCENT_SOFT);
    } else if resp.hovered() {
        p.rect_filled(rect, radius, theme::RAISED);
    }
    if !spec.first {
        p.hline(rect.x_range(), rect.top(), Stroke::new(1.0, BORDER));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // Radio ring.
    let c = egui::pos2(rect.left() + 12.0 + 8.0, rect.center().y);
    p.circle_stroke(
        c,
        7.0,
        Stroke::new(
            2.0,
            if spec.selected {
                ACCENT
            } else {
                theme::BORDER_STRONG
            },
        ),
    );
    if spec.selected {
        p.circle_filled(c, 3.0, ACCENT);
    }
    // Name and description.
    let text_left = rect.left() + 12.0 + 18.0 + 12.0;
    let name = p.layout_no_wrap(
        spec.name.to_string(),
        egui::FontId::new(13.0, theme::semibold_family()),
        theme::TEXT,
    );
    let desc = p.layout_no_wrap(
        spec.desc.to_string(),
        egui::FontId::proportional(12.0),
        MUTED,
    );
    let block_h = name.size().y + 1.0 + desc.size().y;
    let top = rect.center().y - block_h / 2.0;
    let right_w = 190.0;
    let clip = egui::Rect::from_x_y_ranges(text_left..=rect.right() - right_w, rect.y_range());
    let tp = p.with_clip_rect(clip);
    let name_h = name.size().y;
    tp.galley(egui::pos2(text_left, top), name, theme::TEXT);
    tp.galley(egui::pos2(text_left, top + name_h + 1.0), desc, MUTED);
    // The right-hand side, inset from the row's edges.
    let r = egui::Rect::from_x_y_ranges(
        rect.right() - right_w..=rect.right() - 12.0,
        rect.top() + 8.0..=rect.bottom() - 8.0,
    );
    right(ui, r);
    resp
}

/// The `Yours` caption between built-in and own profiles.
fn section(ui: &mut egui::Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0, theme::SECTION);
    p.hline(rect.x_range(), rect.top(), Stroke::new(1.0, BORDER));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.label(
        egui::RichText::new(text.to_uppercase())
            .size(11.0)
            .extra_letter_spacing(0.8)
            .color(MUTED),
    );
}

/// Two right-aligned lines, centred in `r`: `about 21 MB` over `in seconds`.
fn two_lines(ui: &egui::Ui, r: egui::Rect, top: &str, bottom: &str, bottom_color: Color32) {
    let p = ui.painter();
    let t = p.layout_no_wrap(
        top.to_string(),
        egui::FontId::proportional(13.0),
        theme::TEXT,
    );
    let b = p.layout_no_wrap(
        bottom.to_string(),
        egui::FontId::proportional(11.5),
        bottom_color,
    );
    let h = t.size().y + 1.0 + b.size().y;
    let y = r.center().y - h / 2.0;
    let th = t.size().y;
    p.galley(egui::pos2(r.right() - t.size().x, y), t, theme::TEXT);
    p.galley(
        egui::pos2(r.right() - b.size().x, y + th + 1.0),
        b,
        bottom_color,
    );
}

fn estimate_lines(ui: &egui::Ui, r: egui::Rect, e: &Estimate) {
    two_lines(
        ui,
        r,
        &format::about_size(e.bytes),
        &format::ready_in(e.seconds),
        MUTED,
    );
}

fn info_lines(ui: &egui::Ui, r: egui::Rect, info: &RowInfo) {
    match info {
        RowInfo::Estimate(e) => estimate_lines(ui, r, e),
        RowInfo::Fit(f, mb) => {
            let (top, bottom) = match f {
                Some(FitEstimate::Encode(e)) => (
                    format!("under {} MB", trim_num(*mb)),
                    format::ready_in(e.seconds),
                ),
                Some(FitEstimate::Original(e)) => {
                    (format::about_size(e.bytes), format::ready_in(e.seconds))
                }
                Some(FitEstimate::TooSmall { min_mb }) => {
                    ("Too small".into(), format!("needs {min_mb} MB"))
                }
                None => (String::new(), String::new()),
            };
            two_lines(ui, r, &top, &bottom, MUTED);
        }
    }
}
