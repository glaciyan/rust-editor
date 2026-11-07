//! Cutline: open a video, drag two handles, press Export.

mod app;
mod export;
mod icons;
mod modal;
mod profile;
mod theme;
mod timeline;
#[cfg(test)]
mod ui_tests;

use eframe::egui;
use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .filter(|a| !a.to_string_lossy().starts_with('-'))
        .map(|a| {
            // `Open with` passes file:// URIs on some desktops.
            let s = a.to_string_lossy();
            match s.strip_prefix("file://") {
                Some(p) => PathBuf::from(app::percent_decode(p)),
                None => PathBuf::from(a),
            }
        })
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    if std::env::args().any(|a| a == "--version") {
        println!("cutline {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // Cutline draws its own title bar (window buttons in the header).
    // `CUTLINE_SYSTEM_TITLEBAR=1` keeps the desktop's instead.
    let csd = !std::env::var_os("CUTLINE_SYSTEM_TITLEBAR").is_some_and(|v| v != "0");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cutline")
            .with_app_id("cutline")
            .with_inner_size([1160.0, 780.0])
            .with_min_inner_size([720.0, 540.0])
            .with_drag_and_drop(true)
            .with_decorations(!csd)
            .with_icon(std::sync::Arc::new(icons::window_icon(64))),
        event_loop_builder: x11_for_drag_and_drop(),
        ..Default::default()
    };
    eframe::run_native(
        "Cutline",
        options,
        Box::new(move |cc| Ok(Box::new(app::CutlineApp::new(cc, args, csd)))),
    )
}

/// winit 0.30 only delivers dropped files on X11; its Wayland backend has no
/// drag-and-drop. When an X server is reachable (XWayland on every mainstream
/// desktop), run through it so dropping videos onto the window works.
/// `CUTLINE_WAYLAND=1` opts back into native Wayland.
#[cfg(target_os = "linux")]
fn x11_for_drag_and_drop() -> Option<eframe::EventLoopBuilderHook> {
    let has_x = std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty());
    let want_wayland = std::env::var_os("CUTLINE_WAYLAND").is_some_and(|v| v != "0");
    if !has_x || want_wayland {
        return None;
    }
    Some(Box::new(|builder| {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11();
    }))
}

#[cfg(not(target_os = "linux"))]
fn x11_for_drag_and_drop() -> Option<eframe::EventLoopBuilderHook> {
    None
}
