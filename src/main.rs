#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod task;
mod theme;
mod ui;

use redline_core::{format, history, jobs, presets, settings, system, tools};

use std::sync::Arc;

use eframe::egui;

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
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
