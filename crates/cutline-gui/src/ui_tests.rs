//! UI tests: the real window, driven headlessly through egui_kittest the way
//! a person would (clicks and key presses on widgets found by their
//! accessible names). Every `run()` also checks that each control has a name.
//!
//! No ffmpeg, player or export engine runs: the app is built without its
//! background helpers and a video is put on the timeline as if probed.

use crate::app::CutlineApp;
use crate::modal::{Dialog, Kind};
use crate::theme;
use cutline_core::{PartMode, Timestamp};
use cutline_ffmpeg::{AudioStream, Capabilities, MediaInfo, Tools, VideoStream};
use eframe::egui::{self, accesskit::Role, Key, Modifiers};
use egui_kittest::kittest::{By, NodeT, Queryable};
use egui_kittest::{Harness, Node};

// ---------- setup ----------

fn tools() -> Tools {
    Tools {
        ffmpeg: "/nonexistent/ffmpeg".into(),
        ffprobe: "/nonexistent/ffprobe".into(),
        caps: Capabilities {
            version: "7.1".into(),
            major: Some(7),
            libx265: true,
            libx264: true,
            libopus: true,
            aac: true,
            smart_cut: true,
        },
    }
}

/// A 16 s, 1080p, 120 fps H.265 video with one AAC track, in its own
/// (real, empty) folder so an export there can go through.
fn video() -> MediaInfo {
    let dir = std::env::temp_dir().join(format!(
        "cutline-ui-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    MediaInfo {
        path: dir.join("clip.mp4"),
        size_bytes: 32_000_000,
        duration: 16.088,
        start_time: 0.0,
        format_name: "mov,mp4,m4a,3gp,3g2,mj2".into(),
        bit_rate: Some(16_000_000),
        video: Some(VideoStream {
            index: 0,
            codec: "hevc".into(),
            profile: Some("Main".into()),
            width: 1920,
            height: 1080,
            pix_fmt: "yuv420p".into(),
            fps: 120.0,
            fps_num: 120,
            fps_den: 1,
            bit_rate: Some(15_800_000),
            color_primaries: None,
            color_trc: None,
            color_space: None,
            color_range: None,
            rotation: 0,
            bit_depth: 8,
        }),
        audio: vec![AudioStream {
            index: 1,
            codec: "aac".into(),
            channels: 2,
            sample_rate: 48_000,
            bit_rate: Some(192_000),
            language: None,
            title: None,
        }],
    }
}

fn harness_for(app: CutlineApp) -> Harness<'static, CutlineApp> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_context_setup(theme::apply_for_tests)
        .build_ui_state(|ui, app: &mut CutlineApp| app.frame(ui), app);
    h.run();
    h
}

/// The window with one video open, playhead at the start.
fn editor() -> Harness<'static, CutlineApp> {
    let mut app = CutlineApp::for_tests(tools());
    app.add_probed_for_tests(video());
    harness_for(app)
}

fn empty_editor() -> Harness<'static, CutlineApp> {
    harness_for(CutlineApp::for_tests(tools()))
}

fn dialogs(h: &Harness<'_, CutlineApp>) -> Vec<Kind> {
    h.state().dialog_kinds()
}

/// The control called `label`, skipping plain text that says the same
/// (a form's `Name` caption next to its `Name` box). When the header and a
/// dialog both have one (Export), the dialog's comes last.
fn control<'t>(h: &'t Harness<'_, CutlineApp>, label: &'t str) -> Node<'t> {
    let mut all: Vec<Node<'t>> = h
        .get_all(
            By::new()
                .label(label)
                .predicate(|n| n.role() != Role::Label),
        )
        .collect();
    all.pop()
        .unwrap_or_else(|| panic!("no control labelled {label:?}"))
}

fn has_control(h: &Harness<'_, CutlineApp>, label: &str) -> bool {
    h.query_all(
        By::new()
            .label(label)
            .predicate(|n| n.role() != Role::Label),
    )
    .next()
    .is_some()
}

fn shows_text(h: &Harness<'_, CutlineApp>, text: &str) -> bool {
    h.query_all_by_label(text).next().is_some()
}

fn is_disabled(n: &Node<'_>) -> bool {
    n.accesskit_node().is_disabled()
}

/// A radio row or segment that's on.
fn is_on(n: &Node<'_>) -> bool {
    n.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

fn press(h: &mut Harness<'_, CutlineApp>, key: Key) {
    h.key_press(key);
    h.run();
}

fn click(h: &mut Harness<'_, CutlineApp>, label: &str) {
    control(h, label).click();
    h.run();
}

fn open_export(h: &mut Harness<'_, CutlineApp>) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::E);
    h.run();
    assert_eq!(dialogs(h), [Kind::Export]);
}

fn open_new_profile(h: &mut Harness<'_, CutlineApp>) {
    open_export(h);
    click(h, "New profile…");
    assert_eq!(dialogs(h), [Kind::Export, Kind::Profile]);
}

fn type_into(h: &mut Harness<'_, CutlineApp>, label: &str, text: &str) {
    let field = h.get_by_role_and_label(Role::TextInput, label);
    field.click();
    h.run();
    h.get_by_role_and_label(Role::TextInput, label)
        .type_text(text);
    h.run();
}

fn parts(h: &Harness<'_, CutlineApp>) -> Vec<(f64, f64)> {
    h.state().project.clips[0]
        .parts
        .iter()
        .map(|p| (p.start.secs(), p.end.secs()))
        .collect()
}

// ---------- opening and closing dialogs ----------

#[test]
fn ctrl_e_opens_export_and_esc_closes_it() {
    let mut h = editor();
    open_export(&mut h);
    assert!(has_control(&h, "Original quality"));
    press(&mut h, Key::Escape);
    assert!(dialogs(&h).is_empty());
    assert!(!has_control(&h, "Original quality"));
}

#[test]
fn header_export_button_opens_export() {
    let mut h = editor();
    click(&mut h, "Export");
    assert_eq!(dialogs(&h), [Kind::Export]);
}

#[test]
fn export_is_unavailable_without_a_video() {
    let mut h = empty_editor();
    assert!(is_disabled(&control(&h, "Export")));
    h.key_press_modifiers(Modifiers::COMMAND, Key::E);
    h.run();
    assert!(dialogs(&h).is_empty());
}

#[test]
fn ctrl_e_never_stacks_a_second_export() {
    let mut h = editor();
    open_export(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::E);
    h.run();
    assert_eq!(dialogs(&h), [Kind::Export]);
    click(&mut h, "New profile…");
    h.key_press_modifiers(Modifiers::COMMAND, Key::E);
    h.run();
    assert_eq!(dialogs(&h), [Kind::Export, Kind::Profile]);
}

#[test]
fn close_button_and_cancel_close_export() {
    let mut h = editor();
    open_export(&mut h);
    click(&mut h, "Close");
    assert!(dialogs(&h).is_empty());
    open_export(&mut h);
    click(&mut h, "Cancel");
    assert!(dialogs(&h).is_empty());
}

#[test]
fn clicking_the_backdrop_keeps_the_dialog() {
    let mut h = editor();
    open_export(&mut h);
    h.hover_at(egui::pos2(60.0, 400.0));
    h.run();
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(60.0, 400.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(60.0, 400.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    assert_eq!(dialogs(&h), [Kind::Export]);
}

// ---------- two dialogs on top of each other ----------

#[test]
fn new_profile_covers_export_and_esc_goes_back_to_it() {
    let mut h = editor();
    open_new_profile(&mut h);
    // Only the top dialog is shown.
    assert!(has_control(&h, "Save profile"));
    assert!(!has_control(&h, "Original quality"));
    press(&mut h, Key::Escape);
    assert_eq!(dialogs(&h), [Kind::Export]);
    assert!(has_control(&h, "Original quality"));
    assert!(!has_control(&h, "Save profile"));
    press(&mut h, Key::Escape);
    assert!(dialogs(&h).is_empty());
}

#[test]
fn esc_right_after_the_second_dialog_opens_closes_only_that_one() {
    // egui only learns which modal is on top a frame late; Esc on the very
    // next frame must still close the new one, not the one under it.
    let mut h = editor();
    open_export(&mut h);
    control(&h, "New profile…").click();
    h.step();
    assert_eq!(dialogs(&h), [Kind::Export, Kind::Profile]);
    h.key_press(Key::Escape);
    h.step();
    assert_eq!(dialogs(&h), [Kind::Export]);
    h.run();
    assert_eq!(dialogs(&h), [Kind::Export]);
}

#[test]
fn the_dialog_under_shows_on_the_same_frame_the_top_one_closes() {
    let mut h = editor();
    open_new_profile(&mut h);
    h.key_press(Key::Escape);
    h.step();
    assert!(has_control(&h, "Original quality"));
}

#[test]
fn two_escapes_close_two_dialogs() {
    let mut h = editor();
    open_new_profile(&mut h);
    // Both arrive in the same frame.
    h.key_press(Key::Escape);
    h.key_press(Key::Escape);
    h.run();
    assert!(dialogs(&h).is_empty());
}

#[test]
fn cancel_in_new_profile_goes_back_to_export_only() {
    // New profile's Cancel sits where Export's Cancel is: the click must
    // not carry through and close Export too.
    let mut h = editor();
    open_new_profile(&mut h);
    click(&mut h, "Cancel");
    assert_eq!(dialogs(&h), [Kind::Export]);
    h.run_steps(3);
    assert_eq!(dialogs(&h), [Kind::Export]);
}

#[test]
fn close_button_in_new_profile_goes_back_to_export() {
    let mut h = editor();
    open_new_profile(&mut h);
    click(&mut h, "Close");
    assert_eq!(dialogs(&h), [Kind::Export]);
}

#[test]
fn esc_with_a_dropdown_open_closes_just_the_dropdown() {
    let mut h = editor();
    open_new_profile(&mut h);
    click(&mut h, "Video");
    assert!(shows_text(&h, "H.264 (AVC)"), "the menu should be open");
    press(&mut h, Key::Escape);
    assert!(!shows_text(&h, "H.264 (AVC)"), "the menu should be closed");
    assert_eq!(dialogs(&h), [Kind::Export, Kind::Profile]);
    press(&mut h, Key::Escape);
    assert_eq!(dialogs(&h), [Kind::Export]);
}

#[test]
fn picking_from_a_dropdown_changes_the_profile() {
    let mut h = editor();
    open_new_profile(&mut h);
    click(&mut h, "Video");
    h.get_by_label("No video").click();
    h.run();
    // No picture: the picture rows go away and the container follows.
    assert!(!has_control(&h, "Resolution"));
    assert!(is_on(&control(&h, "M4A")));
    assert_eq!(dialogs(&h), [Kind::Export, Kind::Profile]);
}

// ---------- saving a profile ----------

#[test]
fn saved_profile_shows_under_yours_and_is_picked() {
    let mut h = editor();
    open_new_profile(&mut h);
    type_into(&mut h, "Name", "Clips for Discord");
    click(&mut h, "Under a size");
    click(&mut h, "Save profile");
    assert_eq!(dialogs(&h), [Kind::Export]);
    let user = &h.state().profiles.user;
    assert_eq!(user.len(), 1);
    assert_eq!(user[0].name, "Clips for Discord");
    assert!(shows_text(&h, "YOURS"));
    assert!(is_on(&control(&h, "Clips for Discord")));
    let Some(Dialog::Export(d)) = h.state().dialogs.last() else {
        panic!("Export should be open");
    };
    assert_eq!(d.picked(), user[0].id);
}

#[test]
fn save_profile_needs_a_name() {
    let mut h = editor();
    open_new_profile(&mut h);
    assert!(is_disabled(&control(&h, "Save profile")));
    press(&mut h, Key::Enter);
    assert_eq!(dialogs(&h), [Kind::Export, Kind::Profile]);
    assert!(h.state().profiles.user.is_empty());
}

#[test]
fn enter_in_new_profile_saves_it_and_does_not_export() {
    let mut h = editor();
    open_new_profile(&mut h);
    type_into(&mut h, "Name", "Small");
    press(&mut h, Key::Enter);
    assert_eq!(dialogs(&h), [Kind::Export]);
    assert_eq!(h.state().profiles.user.len(), 1);
    assert!(
        h.state().submitted.is_empty(),
        "Enter must not reach Export"
    );
    h.run_steps(3);
    assert_eq!(dialogs(&h), [Kind::Export]);
}

#[test]
fn your_profile_can_be_edited_and_deleted_from_its_menu() {
    let mut h = editor();
    open_new_profile(&mut h);
    type_into(&mut h, "Name", "Mine");
    click(&mut h, "Save profile");
    control(&h, "Mine").click_secondary();
    h.run();
    click(&mut h, "Edit…");
    assert_eq!(dialogs(&h), [Kind::Export, Kind::Profile]);
    assert!(has_control(&h, "Save profile"));
    press(&mut h, Key::Escape);
    control(&h, "Mine").click_secondary();
    h.run();
    click(&mut h, "Delete");
    assert!(h.state().profiles.user.is_empty());
    assert!(!has_control(&h, "Mine"));
    assert_eq!(dialogs(&h), [Kind::Export]);
    // The picked row falls back to Original quality.
    assert!(is_on(&control(&h, "Original quality")));
}

// ---------- the export dialog itself ----------

#[test]
fn clicking_a_row_picks_it() {
    let mut h = editor();
    open_export(&mut h);
    let selected = |h: &Harness<'_, CutlineApp>, l: &str| is_on(&control(h, l));
    assert!(selected(&h, "Original quality"));
    click(&mut h, "Smaller file");
    assert!(selected(&h, "Smaller file"));
    assert!(!selected(&h, "Original quality"));
    // Typing a size picks Fit to size.
    type_into(&mut h, "Target size in MB", "0");
    assert!(selected(&h, "Fit to size"));
}

#[test]
fn enter_in_export_exports_and_closes() {
    let mut h = editor();
    open_export(&mut h);
    press(&mut h, Key::Enter);
    assert!(dialogs(&h).is_empty());
    let out = &h.state().submitted;
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0][0].file_name().unwrap().to_string_lossy(),
        "clip (cut).mp4"
    );
}

#[test]
fn a_bad_folder_keeps_export_open_with_a_message() {
    let mut h = editor();
    open_export(&mut h);
    let folder = h.get_by_role_and_label(Role::TextInput, "Save to");
    folder.click();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role_and_label(Role::TextInput, "Save to")
        .type_text("/no/such/folder");
    h.run();
    click(&mut h, "Export");
    assert_eq!(dialogs(&h), [Kind::Export]);
    assert!(h.state().submitted.is_empty());
    assert!(shows_text(
        &h,
        "That folder doesn't exist. Pick one with Choose…."
    ));
}

// ---------- the editor while a dialog is open ----------

#[test]
fn editor_keys_do_nothing_while_a_dialog_is_open() {
    let mut h = editor();
    h.state_mut().playhead = Some((h.state().project.clips[0].id, Timestamp::from_secs(4.0)));
    let before = parts(&h);
    open_export(&mut h);
    for key in [
        Key::S,
        Key::Delete,
        Key::I,
        Key::O,
        Key::A,
        Key::Space,
        Key::M,
    ] {
        press(&mut h, key);
    }
    assert_eq!(parts(&h), before);
    assert!(!h.state().muted);
    assert_eq!(dialogs(&h), [Kind::Export]);
    // Back in the editor, they work again.
    press(&mut h, Key::Escape);
    press(&mut h, Key::S);
    assert_eq!(parts(&h).len(), 2);
}

#[test]
fn typing_in_a_dialog_doesnt_trigger_editor_shortcuts() {
    let mut h = editor();
    open_new_profile(&mut h);
    let before = parts(&h);
    type_into(&mut h, "Name", "Sim AO");
    assert_eq!(parts(&h), before);
    assert!(!h.state().muted);
}

#[test]
fn other_dialogs_close_on_esc_too() {
    let mut h = editor();
    h.state_mut().push_dialog(Dialog::OpenPath(String::new()));
    h.run();
    assert!(has_control(&h, "Path"));
    press(&mut h, Key::Escape);
    assert!(dialogs(&h).is_empty());

    // Esc on "Still saving" means keep saving.
    h.state_mut().push_dialog(Dialog::ConfirmQuit);
    h.run();
    assert!(has_control(&h, "Keep saving"));
    press(&mut h, Key::Escape);
    assert!(dialogs(&h).is_empty());
}

// ---------- the timeline toolbar ----------

#[test]
fn split_then_delete_cuts_from_the_playhead_to_the_end() {
    let mut h = editor();
    let clip = h.state().project.clips[0].id;
    h.state_mut().playhead = Some((clip, Timestamp::from_secs(4.0)));
    h.run();
    click(&mut h, "Split");
    assert_eq!(parts(&h).len(), 2);
    assert!(shows_text(&h, "2 parts"));
    click(&mut h, "Delete part");
    let p = parts(&h);
    assert_eq!(p.len(), 1);
    assert!((p[0].1 - 4.0).abs() < 0.01, "{p:?}");
    assert!(shows_text(&h, "1 part"));
    assert!(shows_text(&h, "· exports 4 s of 16 s"));
}

#[test]
fn split_and_delete_are_off_when_they_cant_act() {
    let mut h = editor();
    // At the very start of a part there's nothing to split off.
    assert!(is_disabled(&control(&h, "Split")));
    // Nothing selected: nothing to delete.
    h.state_mut().project.selected = None;
    h.run();
    assert!(is_disabled(&control(&h, "Delete part")));
    let empty = empty_editor();
    assert!(is_disabled(&control(&empty, "Split")));
    assert!(is_disabled(&control(&empty, "Delete part")));
}

#[test]
fn undo_and_redo_follow_the_history() {
    let mut h = editor();
    assert!(is_disabled(&control(&h, "Undo")));
    assert!(is_disabled(&control(&h, "Redo")));
    let clip = h.state().project.clips[0].id;
    h.state_mut().playhead = Some((clip, Timestamp::from_secs(8.0)));
    press(&mut h, Key::S);
    assert_eq!(parts(&h).len(), 2);
    assert!(!is_disabled(&control(&h, "Undo")));
    click(&mut h, "Undo");
    assert_eq!(parts(&h).len(), 1);
    assert!(!is_disabled(&control(&h, "Redo")));
    click(&mut h, "Redo");
    assert_eq!(parts(&h).len(), 2);
}

#[test]
fn remove_mode_still_reads_right() {
    let mut h = editor();
    h.state_mut().project.set_mode(PartMode::Remove);
    h.run();
    // Nothing marked to cut: everything is exported.
    assert!(shows_text(&h, "0 cuts"));
    assert!(shows_text(&h, "· exports 16 s of 16 s"));
}

#[test]
fn split_outside_a_part_says_why() {
    let mut h = editor();
    let clip = h.state().project.clips[0].id;
    h.state_mut().playhead = Some((clip, Timestamp::from_secs(4.0)));
    press(&mut h, Key::S);
    press(&mut h, Key::Delete);
    // The playhead now sits in the cut-out stretch.
    press(&mut h, Key::S);
    assert_eq!(parts(&h).len(), 1);
    assert!(h
        .state()
        .toast_text()
        .is_some_and(|t| t.contains("inside a part")));
}

// ---------- how things are drawn ----------

/// The colour each piece of text in the last frame was actually drawn in.
fn drawn_text_colors(h: &Harness<'_, CutlineApp>) -> Vec<(String, egui::Color32)> {
    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
        match shape {
            egui::Shape::Text(t) => {
                for s in &t.galley.job.sections {
                    let c = t
                        .override_text_color
                        .or((s.format.color != egui::Color32::PLACEHOLDER)
                            .then_some(s.format.color))
                        .unwrap_or(t.fallback_color);
                    out.push((
                        t.galley.job.text[s.byte_range.start.0..s.byte_range.end.0].to_string(),
                        c,
                    ));
                }
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    for s in &h.output().shapes {
        walk(&s.shape, &mut out);
    }
    out
}

#[test]
fn export_button_text_is_dark_on_blue_like_its_icon() {
    let h = editor();
    let colors = drawn_text_colors(&h);
    let export: Vec<_> = colors.iter().filter(|(t, _)| t == "Export").collect();
    assert!(!export.is_empty(), "the header's Export label wasn't drawn");
    for (_, c) in export {
        assert_eq!(*c, theme::ON_ACCENT);
    }
}

#[test]
fn unpicked_segments_are_muted() {
    let mut h = editor();
    open_new_profile(&mut h);
    let colors = drawn_text_colors(&h);
    let color_of = |label: &str| colors.iter().find(|(t, _)| t == label).map(|(_, c)| *c);
    assert_eq!(color_of("MP4"), Some(theme::TEXT));
    assert_eq!(color_of("MKV"), Some(theme::MUTED));
}
