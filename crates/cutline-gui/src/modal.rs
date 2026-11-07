//! Dialogs, kept as a stack. Only the top one is drawn and only it hears the
//! keyboard: Esc closes it and shows the one under it again (New profile →
//! back to Export), Enter confirms it. The editor's own shortcuts are off
//! while any dialog is open.

use crate::app::{CutlineApp, JobState};
use crate::export::ExportDialog;
use crate::icons::Icon;
use crate::profile::ProfileDialog;
use crate::theme::{self, MUTED};
use cutline_core::format;
use cutline_engine::job::JobId;
use eframe::egui::{self, Key, Modifiers, RichText};
use std::path::PathBuf;

pub enum Dialog {
    Export(Box<ExportDialog>),
    Profile(Box<ProfileDialog>),
    Details(JobId),
    /// No file chooser on this system: type a path instead.
    OpenPath(String),
    /// Closing the window while an export runs.
    ConfirmQuit,
}

/// What a dialog asks for after a frame.
pub enum Outcome {
    Stay,
    Close,
    /// Open another dialog on top of this one.
    Push(Dialog),
    /// A profile was saved: close, and pick it in the Export dialog below.
    ProfileSaved(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Export,
    Profile,
    Details,
    OpenPath,
    ConfirmQuit,
}

impl Dialog {
    pub fn kind(&self) -> Kind {
        match self {
            Dialog::Export(_) => Kind::Export,
            Dialog::Profile(_) => Kind::Profile,
            Dialog::Details(_) => Kind::Details,
            Dialog::OpenPath(_) => Kind::OpenPath,
            Dialog::ConfirmQuit => Kind::ConfirmQuit,
        }
    }

    fn show(&mut self, ctx: &egui::Context, app: &mut CutlineApp, enter: bool) -> Outcome {
        match self {
            Dialog::Export(d) => d.show(ctx, app, enter),
            Dialog::Profile(d) => d.show(ctx, app, enter),
            Dialog::Details(job) => details(ctx, app, *job),
            Dialog::OpenPath(text) => open_path(ctx, app, text, enter),
            Dialog::ConfirmQuit => confirm_quit(ctx, app, enter),
        }
    }
}

impl CutlineApp {
    pub fn push_dialog(&mut self, d: Dialog) {
        self.dialogs.push(d);
    }

    pub fn has_dialog(&self, kind: Kind) -> bool {
        self.dialogs.iter().any(|d| d.kind() == kind)
    }

    /// The open dialogs, bottom first.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dialog_kinds(&self) -> Vec<Kind> {
        self.dialogs.iter().map(Dialog::kind).collect()
    }

    /// Draw the top dialog and act on what it asks for.
    pub fn show_dialogs(&mut self, ctx: &egui::Context) {
        if self.dialogs.is_empty() {
            return;
        }
        // An open menu inside the dialog (a dropdown) takes Esc and Enter
        // first; egui closes it itself.
        let popup = egui::Popup::is_any_open(ctx);
        let (esc, enter) = if popup {
            (0, false)
        } else {
            ctx.input_mut(|i| {
                (
                    i.count_and_consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            })
        };
        // Each Esc closes one dialog, the top one.
        for _ in 0..esc {
            if self.dialogs.pop().is_none() {
                break;
            }
        }
        // When the top one closes, draw the one it uncovers this same
        // frame, so the dialog under it doesn't blink.
        if self.draw_top(ctx, enter) {
            self.draw_top(ctx, false);
        }
    }

    /// Draw the top dialog; true when it closed and another one is under it.
    fn draw_top(&mut self, ctx: &egui::Context, enter: bool) -> bool {
        let Some(mut top) = self.dialogs.pop() else {
            return false;
        };
        let below = self.dialogs.len();
        let outcome = top.show(ctx, self, enter);
        // Anything the app opened while the dialog was drawn goes above it.
        let opened = self.dialogs.split_off(below);
        let closed = match outcome {
            Outcome::Stay => {
                self.dialogs.push(top);
                false
            }
            Outcome::Close => true,
            Outcome::Push(d) => {
                self.dialogs.push(top);
                self.dialogs.push(d);
                false
            }
            Outcome::ProfileSaved(id) => {
                if let Some(Dialog::Export(e)) = self
                    .dialogs
                    .iter_mut()
                    .rev()
                    .find(|d| d.kind() == Kind::Export)
                {
                    e.select(&id);
                }
                true
            }
        };
        let uncovered = closed && opened.is_empty() && !self.dialogs.is_empty();
        self.dialogs.extend(opened);
        if closed {
            ctx.request_repaint();
        }
        uncovered
    }
}

/// A centred dialog with the app's frame and a dimmed backdrop. Clicking
/// the backdrop does nothing: dialogs close with Esc, × or Cancel, so a
/// stray click never throws away what was typed.
pub fn modal<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let width = width.min(ctx.content_rect().width() - 48.0).max(280.0);
    egui::Modal::new(egui::Id::unique(("dialog", id)))
        .backdrop_color(theme::BACKDROP)
        .frame(theme::dialog_frame())
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

/// Title, an optional line under it, and a × on the right. True when × was clicked.
pub fn header(ui: &mut egui::Ui, title: &str, subtitle: Option<&str>) -> bool {
    let mut close = false;
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 20,
            right: 14,
            top: 16,
            bottom: 14,
        })
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.add_space(2.0);
                    ui.label(theme::strong(title).size(15.5));
                    if let Some(s) = subtitle {
                        ui.label(theme::small(s).size(12.5));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    close = theme::icon_button(
                        ui,
                        Icon::Close,
                        egui::vec2(28.0, 28.0),
                        "Close",
                        "Close (Esc)",
                    )
                    .clicked();
                });
            });
        });
    close
}

/// The bottom row: a hairline above, buttons laid out by `add`.
pub fn footer(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let r = ui.max_rect();
    ui.painter().hline(
        r.x_range(),
        ui.cursor().top(),
        egui::Stroke::new(1.0, theme::HAIRLINE),
    );
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(20, 12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                add(ui)
            });
        });
}

/// Padding for a dialog's body.
pub fn body<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 0,
            bottom: 18,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            add(ui)
        })
        .inner
}

fn details(ctx: &egui::Context, app: &mut CutlineApp, id: JobId) -> Outcome {
    let Some(job) = app.jobs.iter().find(|j| j.id == id) else {
        return Outcome::Close;
    };
    let max_h = (ctx.content_rect().height() - 220.0).max(160.0);
    modal(ctx, "details", 760.0, |ui| {
        let mut out = Outcome::Stay;
        if header(
            ui,
            "Details",
            Some("What ffmpeg was asked to do, and what it said"),
        ) {
            out = Outcome::Close;
        }
        body(ui, |ui| {
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
            ui.horizontal(|ui| {
                ui.label(theme::strong("ffmpeg commands"));
                if ui.add(theme::ghost("Copy").height(24.0)).clicked() {
                    ui.ctx().copy_text(job.details.commands.join("\n"));
                }
            });
            let cmds = job.details.commands.join("\n\n");
            egui::ScrollArea::vertical()
                .id_salt("cmds")
                .max_height(max_h * 0.4)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut cmds.as_str())
                            .code_editor()
                            .desired_width(f32::INFINITY),
                    );
                });
            ui.horizontal(|ui| {
                ui.label(theme::strong("Log"));
                if ui.add(theme::ghost("Copy").height(24.0)).clicked() {
                    ui.ctx().copy_text(job.details.log.clone());
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("log")
                .max_height(max_h * 0.6)
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
        footer(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(theme::secondary("Close details")).clicked() {
                    out = Outcome::Close;
                }
            });
        });
        out
    })
}

fn open_path(ctx: &egui::Context, app: &mut CutlineApp, text: &mut String, enter: bool) -> Outcome {
    let mut open = enter;
    let mut out = modal(ctx, "open-path", 480.0, |ui| {
        let mut out = Outcome::Stay;
        if header(
            ui,
            "Open a video",
            Some("Type or paste the path of a video file"),
        ) {
            out = Outcome::Close;
        }
        body(ui, |ui| {
            ui.add(
                theme::Field::new(text, "Path")
                    .hint("/home/you/Videos/clip.mp4")
                    .focus(true),
            );
        });
        footer(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(theme::primary("Open")).clicked() {
                    open = true;
                }
                if ui.add(theme::secondary("Cancel")).clicked() {
                    out = Outcome::Close;
                }
            });
        });
        out
    });
    if open {
        let p = shellexpand_home(text.trim());
        if p.is_file() {
            app.add_files(vec![p]);
            out = Outcome::Close;
        } else {
            app.toast("There's no file at that path.".into());
        }
    }
    out
}

fn confirm_quit(ctx: &egui::Context, app: &mut CutlineApp, enter: bool) -> Outcome {
    modal(ctx, "quit", 400.0, |ui| {
        // Esc and × keep saving, like the safe button.
        let mut out = Outcome::Stay;
        if header(ui, "Still saving", None) {
            out = Outcome::Close;
        }
        body(ui, |ui| {
            ui.label(
                RichText::new("If you quit now, the export stops and nothing is saved.")
                    .color(MUTED),
            );
        });
        footer(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(theme::primary("Keep saving")).clicked() || enter {
                    out = Outcome::Close;
                }
                if ui.add(theme::secondary("Quit anyway")).clicked() {
                    app.quit_now(ui.ctx());
                    out = Outcome::Close;
                }
            });
        });
        out
    })
}

pub fn shellexpand_home(s: &str) -> PathBuf {
    match (s.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(h)) => PathBuf::from(h).join(rest),
        _ => PathBuf::from(s),
    }
}
