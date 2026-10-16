//! Bottom bar showing the current job, styled like Spotify's player bar.

use eframe::egui::{self, Align, Layout, RichText, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icons;

use crate::app::{App, Page};
use crate::jobs::JobStatus;
use crate::theme;

pub fn show(ui: &mut Ui, app: &mut App) {
    let full = ui.max_rect();
    let Some(job) = app.jobs.current() else {
        ui.centered_and_justified(|ui| {
            ui.label(theme::muted(format!(
                "{}  Nothing queued. Paste a link or drop a file to get started.",
                icons::VINYL_RECORD
            )));
        });
        return;
    };

    let third = full.width() / 3.0;
    let left = egui::Rect::from_min_size(full.min, vec2(third * 1.1, full.height()));
    let center = egui::Rect::from_min_size(
        egui::pos2(full.min.x + third * 1.1, full.min.y),
        vec2(third * 1.1, full.height()),
    );
    let right = egui::Rect::from_min_max(egui::pos2(center.max.x, full.min.y), full.max);

    // Left: tile + title
    let mut l = ui.new_child(
        UiBuilder::new()
            .max_rect(left)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let running = matches!(job.status, JobStatus::Running { .. } | JobStatus::Processing(_));
    theme::art_tile(
        &mut l,
        52.0,
        if job.is_download() {
            icons::DOWNLOAD_SIMPLE
        } else {
            icons::ARROWS_LEFT_RIGHT
        },
        if running { theme::ACCENT } else { theme::MUTED },
    );
    l.add_space(8.0);
    l.vertical(|ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(theme::strong(&job.title)).truncate());
        ui.add(egui::Label::new(RichText::new(&job.subtitle).small().color(theme::MUTED)).truncate());
    });

    // Center: status + progress
    let mut c = ui.new_child(
        UiBuilder::new()
            .max_rect(center.shrink2(vec2(12.0, 14.0)))
            .layout(Layout::top_down(Align::Center)),
    );
    let (text, progress) = match &job.status {
        JobStatus::Queued => ("Waiting to start…".to_owned(), Some(0.0)),
        JobStatus::Running { progress, detail } => (detail.clone(), *progress),
        JobStatus::Processing(msg) => (msg.clone(), None),
        JobStatus::Done(_) => ("Finished".to_owned(), Some(1.0)),
        JobStatus::Skipped(why) => (why.clone(), Some(1.0)),
        JobStatus::Failed(e) => (
            format!("Failed: {}", e.lines().next().unwrap_or("")),
            Some(0.0),
        ),
        JobStatus::Cancelled => ("Cancelled".to_owned(), Some(0.0)),
    };
    c.add(egui::Label::new(RichText::new(text).small().color(theme::MUTED)).truncate());
    let w = c.available_width();
    theme::progress_line(&mut c, w, progress);

    // Right: queue summary + jump to queue
    let mut r = ui.new_child(
        UiBuilder::new()
            .max_rect(right)
            .layout(Layout::right_to_left(Align::Center)),
    );
    if theme::icon_button(&mut r, icons::QUEUE, "Open queue").clicked() {
        app.page = Page::Queue;
    }
    let active = app.jobs.active_count();
    let summary = match active {
        0 => "Queue idle".to_owned(),
        1 => "1 job running".to_owned(),
        n => format!("{n} jobs in queue"),
    };
    r.label(RichText::new(summary).small().color(theme::MUTED));
}
