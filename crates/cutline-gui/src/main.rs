//! Cutline: open a video, drag two handles, press Export.

mod app;
mod dialogs;
mod export;
mod theme;
mod timeline;

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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cutline")
            .with_app_id("cutline")
            .with_inner_size([960.0, 760.0])
            .with_min_inner_size([640.0, 520.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Cutline",
        options,
        Box::new(move |cc| Ok(Box::new(app::CutlineApp::new(cc, args)))),
    )
}
