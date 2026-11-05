#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod task;
mod theme;
mod ui;

use redline_core::{format, history, jobs, presets, settings, system, tools};

use std::sync::Arc;

use eframe::egui;

fn main() -> eframe::Result {
    // A second window would save its own copy of the library over the first one's changes.
    // The OS lets go of the lock when Redline exits, even if it crashes.
    let _ = std::fs::create_dir_all(settings::data_dir());
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(settings::data_dir().join("redline.lock"));

    if lock.as_ref().is_ok_and(|f| f.try_lock().is_err()) {
        rfd::MessageDialog::new()
            .set_title("Redline")
            .set_description("Redline is already open.")
            .show();
        return Ok(());
    }
    let mut viewport
 = egui::ViewportBuilder::default()
        .with_title("Redline")
        .with_inner_size([1200.0, 780.0])
        .with_min_inner_size([900.0, 600.0])
        .with_drag_and_drop(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/redline-256.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Redline",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
