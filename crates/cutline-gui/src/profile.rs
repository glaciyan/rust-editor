//! New profile: the one place codecs, sizes and frame rates show up. A saved
//! profile is listed under Yours in Export.

use crate::app::CutlineApp;
use crate::icons::Icon;
use crate::modal::{self, Outcome};
use crate::theme::{self, MUTED};
use cutline_core::{format, AudioCodec, Container, Profile, RateControl, VideoCodec};
use cutline_engine::estimate::{self, FitEstimate};
use cutline_engine::request::ExportRequest;
use eframe::egui;
use std::path::PathBuf;

pub struct ProfileDialog {
    /// The profile's id when editing one of yours.
    editing: Option<String>,
    p: Profile,
    size_text: String,
    crf_text: String,
    kbps_text: String,
    focus_once: bool,
}

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
const AUDIO: [(AudioCodec, u32); 11] = [
    (AudioCodec::Copy, 0),
    (AudioCodec::Aac, 96),
    (AudioCodec::Aac, 128),
    (AudioCodec::Aac, 160),
    (AudioCodec::Aac, 192),
    (AudioCodec::Aac, 256),
    (AudioCodec::Opus, 64),
    (AudioCodec::Opus, 96),
    (AudioCodec::Opus, 128),
    (AudioCodec::Opus, 160),
    (AudioCodec::None, 0),
];

fn container_label(c: Container) -> &'static str {
    match c {
        Container::Source => "Same",
        Container::Mp4 => "MP4",
        Container::Mkv => "MKV",
        Container::Mov => "MOV",
        Container::M4a => "M4A",
    }
}

fn video_label(c: VideoCodec) -> &'static str {
    match c {
        VideoCodec::Copy => "Original (no re-encode)",
        VideoCodec::H265 => "H.265 (HEVC)",
        VideoCodec::H264 => "H.264 (AVC)",
        VideoCodec::None => "No video",
    }
}

fn height_label(h: Option<u32>) -> String {
    match h {
        None => "Same as the video".into(),
        Some(h) => format!("{}×{h}", (h as f64 * 16.0 / 9.0 / 2.0).round() as u32 * 2),
    }
}

fn fps_label(f: Option<f64>) -> String {
    match f {
        None => "Same as the video".into(),
        Some(f) => format!("{f} fps"),
    }
}

fn audio_label(codec: AudioCodec, kbps: u32) -> String {
    match codec {
        AudioCodec::Copy => "Original (no re-encode)".into(),
        AudioCodec::None => "No audio".into(),
        c => format!("{} · {kbps} kbps", c.label()),
    }
}

/// The one line shown under the name in Export: `H.265 · 720p · 60 fps · under 10 MB`.
pub fn summary(p: &Profile) -> String {
    let mut bits = vec![];
    match p.video.codec {
        VideoCodec::Copy => bits.push("Original video".to_string()),
        VideoCodec::None => bits.push("Audio only".to_string()),
        c => {
            bits.push(
                if c == VideoCodec::H265 {
                    "H.265"
                } else {
                    "H.264"
                }
                .to_string(),
            );
            if let Some(h) = p.video.max_height {
                bits.push(format!("{h}p"));
            }
            if let Some(f) = p.video.max_fps {
                bits.push(format!("{f} fps"));
            }
            bits.push(match p.video.rate {
                RateControl::TargetSize(mb) => format!("under {} MB", trim(mb)),
                RateControl::Crf(q) => format!("quality {q}"),
                RateControl::Bitrate(k) => format!("{k} kbps"),
            });
        }
    }
    if p.video.codec == VideoCodec::None || p.audio.codec != AudioCodec::Copy {
        bits.push(audio_label(p.audio.codec, p.audio.bitrate_kbps));
    }
    bits.join(" · ")
}

fn trim(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

impl ProfileDialog {
    /// A new profile, or one of yours to edit.
    pub fn new(app: &CutlineApp, editing: Option<&str>) -> Self {
        let existing = editing.and_then(|id| app.profiles.user.iter().find(|p| p.id == id));
        let p = existing.cloned().unwrap_or_else(|| {
            let mut p = Profile::presets()[0].clone();
            p.id = String::new();
            p.name = String::new();
            p.description = String::new();
            p.builtin = false;
            p.video.rate = RateControl::Crf(24);
            p
        });
        let (size, crf, kbps) = match p.video.rate {
            RateControl::TargetSize(mb) => (trim(mb), "24".into(), "4000".into()),
            RateControl::Crf(q) => ("10".into(), q.to_string(), "4000".into()),
            RateControl::Bitrate(k) => ("10".into(), "24".into(), k.to_string()),
        };
        ProfileDialog {
            editing: existing.map(|p| p.id.clone()),
            p,
            size_text: size,
            crf_text: crf,
            kbps_text: kbps,
            focus_once: true,
        }
    }

    /// The profile as it would be saved, if every box holds a number.
    fn built(&self) -> Option<Profile> {
        let mut p = self.p.clone();
        let num = |s: &str| s.trim().replace(',', ".").parse::<f64>().ok();
        p.video.rate = match p.video.rate {
            RateControl::TargetSize(_) => {
                RateControl::TargetSize(num(&self.size_text).filter(|v| *v > 0.0)?)
            }
            RateControl::Crf(_) => {
                RateControl::Crf(num(&self.crf_text).filter(|v| (0.0..=51.0).contains(v))? as u8)
            }
            RateControl::Bitrate(_) => {
                RateControl::Bitrate(num(&self.kbps_text).filter(|v| *v >= 50.0)? as u32)
            }
        };
        p.name = p.name.trim().to_string();
        p.description = summary(&p);
        Some(p)
    }

    pub fn show(&mut self, ctx: &egui::Context, app: &mut CutlineApp, enter: bool) -> Outcome {
        let mut outcome = Outcome::Stay;
        let mut save = enter;
        let built = self.built();
        let estimate = built.as_ref().and_then(|p| estimate_line(app, p));
        let title = if self.editing.is_some() {
            "Edit profile"
        } else {
            "New profile"
        };
        let focus = std::mem::take(&mut self.focus_once);
        modal::modal(ctx, "profile", 520.0, |ui| {
            if modal::header(ui, title, Some("Shows up under Yours in Export")) {
                outcome = Outcome::Close;
            }
            modal::body(ui, |ui| {
                let field_w = ui.available_width() - 96.0 - 14.0;
                egui::Grid::new("profile-name")
                    .num_columns(2)
                    .spacing([14.0, 12.0])
                    .min_col_width(96.0)
                    .show(ui, |ui| {
                        ui.label(theme::form_label("Name"));
                        ui.add(
                            theme::Field::new(&mut self.p.name, "Name")
                                .hint("Clips for Discord")
                                .width(field_w)
                                .focus(focus),
                        );
                        ui.end_row();
                    });
                ui.add_space(4.0);
                let r = ui.available_rect_before_wrap();
                ui.painter().hline(
                    r.x_range(),
                    r.top(),
                    egui::Stroke::new(1.0, theme::HAIRLINE),
                );
                ui.add_space(12.0);
                egui::Grid::new("profile-fields")
                    .num_columns(2)
                    .spacing([14.0, 12.0])
                    .min_col_width(96.0)
                    .show(ui, |ui| self.fields(ui, field_w));
            });
            modal::footer(ui, |ui| {
                let (text, color) = estimate.clone().unwrap_or_default();
                ui.label(egui::RichText::new(text).size(12.5).color(color));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_save = built.is_some() && !self.p.name.trim().is_empty();
                    if ui
                        .add_enabled(can_save, theme::primary("Save profile"))
                        .on_disabled_hover_text("Give it a name first")
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
        if save && matches!(outcome, Outcome::Stay) {
            if let Some(mut p) = self.built().filter(|p| !p.name.is_empty()) {
                p.id = self
                    .editing
                    .clone()
                    .unwrap_or_else(|| app.profiles.new_id());
                p.builtin = false;
                let id = p.id.clone();
                app.profiles.upsert(p);
                app.save_profiles();
                return Outcome::ProfileSaved(id);
            }
        }
        outcome
    }

    fn fields(&mut self, ui: &mut egui::Ui, w: f32) {
        let p = &mut self.p;
        ui.label(theme::form_label("Container"));
        let options: Vec<&str> = Container::ALL.iter().map(|c| container_label(*c)).collect();
        let at = Container::ALL
            .iter()
            .position(|c| *c == p.container)
            .unwrap_or(1);
        if let Some(i) = theme::segmented(ui, "container", &options, at, Some(w)) {
            p.container = Container::ALL[i];
        }
        ui.end_row();

        ui.label(theme::form_label("Video"));
        let before = p.video.codec;
        dropdown(
            ui,
            "Video",
            w,
            video_label(p.video.codec).to_string(),
            |ui| {
                for c in VideoCodec::ALL {
                    ui.selectable_value(&mut p.video.codec, c, video_label(c));
                }
            },
        );
        // Keep the container sensible when the picture comes or goes.
        if p.video.codec != before {
            if p.video.codec == VideoCodec::None {
                p.container = Container::M4a;
            } else if p.container == Container::M4a {
                p.container = Container::Mp4;
            }
        }
        ui.end_row();

        if matches!(p.video.codec, VideoCodec::H265 | VideoCodec::H264) {
            ui.label(theme::form_label("Quality"));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let mut modes = vec!["Under a size", "Constant quality"];
                let bitrate = matches!(p.video.rate, RateControl::Bitrate(_));
                if bitrate {
                    modes.push("Bitrate");
                }
                let at = match p.video.rate {
                    RateControl::TargetSize(_) => 0,
                    RateControl::Crf(_) => 1,
                    RateControl::Bitrate(_) => 2,
                };
                let seg_w = w - 92.0 - 8.0;
                if let Some(i) = theme::segmented(ui, "quality", &modes, at, Some(seg_w)) {
                    p.video.rate = match i {
                        0 => RateControl::TargetSize(10.0),
                        1 => RateControl::Crf(24),
                        _ => RateControl::Bitrate(4000),
                    };
                }
                let (text, label, unit, hint) = match p.video.rate {
                    RateControl::TargetSize(_) => {
                        (&mut self.size_text, "Size limit in MB", "MB", "")
                    }
                    RateControl::Crf(_) => {
                        (&mut self.crf_text, "Quality, 0 best to 51 worst", "CRF", "")
                    }
                    RateControl::Bitrate(_) => (&mut self.kbps_text, "Bitrate in kbps", "kbps", ""),
                };
                let r = ui.add(
                    theme::Field::new(text, label)
                        .width(92.0)
                        .right_align()
                        .suffix(unit)
                        .hint(hint),
                );
                if matches!(p.video.rate, RateControl::Crf(_)) {
                    r.on_hover_text("Lower is better and bigger. 18–28 is the usual range.");
                }
            });
            ui.end_row();

            ui.label(theme::form_label("Resolution"));
            dropdown(
                ui,
                "Resolution",
                w,
                height_label(p.video.max_height),
                |ui| {
                    for h in HEIGHTS {
                        ui.selectable_value(&mut p.video.max_height, h, height_label(h));
                    }
                },
            )
            .on_hover_text("Videos bigger than this are scaled down; the shape stays the same.");
            ui.end_row();

            ui.label(theme::form_label("Frame rate"));
            dropdown(ui, "Frame rate", w, fps_label(p.video.max_fps), |ui| {
                for f in RATES {
                    ui.selectable_value(&mut p.video.max_fps, f, fps_label(f));
                }
            })
            .on_hover_text("Videos faster than this are slowed to it (or to half their rate).");
            ui.end_row();
        }

        ui.label(theme::form_label("Audio"));
        let now = (p.audio.codec, p.audio.bitrate_kbps);
        let mut choices: Vec<(AudioCodec, u32)> = AUDIO.to_vec();
        if !choices.iter().any(|c| same_audio(*c, now)) {
            choices.insert(1, now);
        }
        let mut picked = now;
        dropdown(ui, "Audio", w, audio_label(now.0, now.1), |ui| {
            for c in &choices {
                let on = same_audio(*c, picked);
                if ui.selectable_label(on, audio_label(c.0, c.1)).clicked() {
                    picked = *c;
                }
            }
        });
        if !same_audio(picked, now) {
            p.audio.codec = picked.0;
            if picked.1 > 0 {
                p.audio.bitrate_kbps = picked.1;
            }
        }
        ui.end_row();
    }
}

fn same_audio(a: (AudioCodec, u32), b: (AudioCodec, u32)) -> bool {
    a.0 == b.0 && (matches!(a.0, AudioCodec::Copy | AudioCodec::None) || a.1 == b.1)
}

/// A dropdown in the form style, named for screen readers.
fn dropdown(
    ui: &mut egui::Ui,
    label: &str,
    w: f32,
    selected: String,
    menu: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let r = egui::ComboBox::from_id_salt(label)
        .width(w)
        .height(320.0)
        .selected_text(selected)
        .icon(|ui, rect, _visuals, _open| {
            Icon::ChevronDown.paint(
                ui.painter(),
                egui::Rect::from_center_size(rect.center(), egui::vec2(14.0, 14.0)),
                MUTED,
            );
        })
        .show_ui(ui, menu);
    let id = r.response.id;
    let label = label.to_string();
    ui.ctx()
        .accesskit_node_builder(id, |node| node.set_label(label));
    r.response
}

/// `About 8 MB for this cut`, from the same estimates Export uses.
fn estimate_line(app: &CutlineApp, p: &Profile) -> Option<(String, egui::Color32)> {
    let tools = app.tools.as_ref()?;
    let segments = app.project.segments();
    if segments.is_empty() {
        return None;
    }
    let req = ExportRequest {
        segments,
        profile: p.clone(),
        output_dir: PathBuf::new(),
        name: String::new(),
        separate: false,
        audio_tracks: vec![0],
        overwrite: false,
    };
    let caps = &tools.caps;
    Some(match p.video.rate {
        RateControl::TargetSize(mb) if p.is_fit() => {
            match estimate::fit(&req, &app.sources, caps, mb, None) {
                FitEstimate::TooSmall { min_mb } => (
                    format!("Too small for this cut · try {min_mb} MB"),
                    theme::DANGER,
                ),
                FitEstimate::Original(_) => {
                    ("This cut already fits at original quality".into(), MUTED)
                }
                FitEstimate::Encode(e) => (
                    format!(
                        "Under {} MB for this cut · {}",
                        trim(mb),
                        format::ready_in(e.seconds)
                    ),
                    MUTED,
                ),
            }
        }
        _ => {
            let e = if p.is_copy() {
                estimate::original(&req, &app.sources, caps, None)
            } else {
                estimate::encode(&req, &app.sources, None)
            };
            (
                format!("{} for this cut", capitalise(&format::about_size(e.bytes))),
                MUTED,
            )
        }
    })
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reads_like_the_design() {
        let mut p = Profile::presets()[0].clone();
        p.video.codec = VideoCodec::H265;
        p.video.max_height = Some(720);
        p.video.max_fps = Some(60.0);
        p.video.rate = RateControl::TargetSize(10.0);
        p.audio.codec = AudioCodec::Aac;
        p.audio.bitrate_kbps = 128;
        assert_eq!(
            summary(&p),
            "H.265 · 720p · 60 fps · under 10 MB · AAC · 128 kbps"
        );
        assert_eq!(height_label(Some(720)), "1280×720");
        assert_eq!(height_label(Some(1080)), "1920×1080");
    }
}
