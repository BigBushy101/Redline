#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod history;
mod jobs;
mod settings;
mod task;
mod theme;
mod tools;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Redline")
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Redline",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
