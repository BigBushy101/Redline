use eframe::egui::{self, Align, CornerRadius, Layout, RichText, Sense, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icons;

use crate::app::{App, Page};
use crate::jobs::{Job, JobId, JobStatus};
use crate::system::AfterQueue;
use crate::theme;
use crate::tools;

enum Action {
    Cancel(JobId),
    Retry(JobId),
    RetryAnyway(JobId),
    Remove(JobId),
    Reveal(std::path::PathBuf),
}

pub fn show(ui: &mut Ui, app: &mut App) {
    let active = app.jobs.active_count();
    let finished = app.jobs.jobs.len() - active;
    theme::page_header(
        ui,
        "Queue",
        "Your queue",
        &format!("{active} in progress · {finished} finished"),
    );

    if app.jobs.jobs.is_empty() {
        empty_state(ui, app);
        return;
    }

    ui.horizontal(|ui| {
        if theme::ghost_button(ui, "Clear finished", finished > 0).clicked() {
            app.jobs.clear_finished();
        }
        if theme::ghost_button(ui, "Cancel all", active > 0).clicked() {
            app.jobs.cancel_all();
            app.after_queue = AfterQueue::Nothing;
        }
        ui.add_space(16.0);
        ui.label(theme::muted("When everything's done"));
        theme::chips(
            ui,
            &mut app.after_queue,
            &[
                (AfterQueue::Nothing, "Do nothing"),
                (AfterQueue::OpenFolder, "Open folder"),
                (AfterQueue::Sleep, "Sleep"),
                (AfterQueue::ShutDown, "Shut down"),
            ],
        );
    });
    if app.after_queue == AfterQueue::ShutDown {
        ui.label(
            RichText::new(format!(
                "{} Windows will shut down a minute after the last job, with a chance to cancel.",
                icons::INFO
            ))
            .small()
            .color(theme::MUTED),
        );
    }
    ui.add_space(16.0);

    let mut actions = Vec::new();
    // Newest first, like a recently-played list.
    for job in app.jobs.jobs.iter().rev() {
        job_row(ui, job, &mut actions);
        ui.add_space(4.0);
    }

    for action in actions {
        match action {
            Action::Cancel(id) => app.jobs.cancel(id),
            Action::Retry(id) => app.jobs.retry(id),
            Action::RetryAnyway(id) => app.jobs.retry_ignoring_archive(id),
            Action::Remove(id) => app.jobs.remove(id),
            Action::Reveal(path) => tools::reveal(&path),
        }
    }
}

fn empty_state(ui: &mut Ui, app: &mut App) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icons::QUEUE).size(64.0).color(theme::SUBTLE));
        ui.add_space(8.0);
        ui.label(theme::heading("Your queue is empty"));
        ui.label(theme::muted("Downloads and conversions you start will show up here."));
        ui.add_space(16.0);
        if theme::small_pill_button(ui, "Find something to download", true).clicked() {
            app.page = Page::Download;
        }
    });
}

fn job_row(ui: &mut Ui, job: &Job, actions: &mut Vec<Action>) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 68.0), Sense::hover());
    let hovered = ui.rect_contains_pointer(rect);
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(6),
        if hovered { theme::CARD_HOVER } else { theme::CARD },
    );

    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(14.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let running = matches!(job.status, JobStatus::Running { .. } | JobStatus::Processing(_));
    let icon = if job.is_download() {
        icons::DOWNLOAD_SIMPLE
    } else {
        icons::ARROWS_LEFT_RIGHT
    };
    theme::art_tile(
        &mut row,
        44.0,
        icon,
        if running { theme::ACCENT } else { theme::MUTED },
    );
    row.add_space(10.0);

    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // Actions
        match &job.status {
            JobStatus::Queued | JobStatus::Running { .. } | JobStatus::Processing(_) => {
                if theme::icon_button(ui, icons::X, "Cancel").clicked() {
                    actions.push(Action::Cancel(job.id));
                }
            }
            JobStatus::Done(path) => {
                if theme::icon_button(ui, icons::TRASH, "Remove from list").clicked() {
                    actions.push(Action::Remove(job.id));
                }
                let target = path.clone().or_else(|| job.output_dir());
                if let Some(target) = target
                    && theme::icon_button(ui, icons::FOLDER_OPEN, "Show in folder").clicked()
                {
                    actions.push(Action::Reveal(target));
                }
            }
            JobStatus::Skipped(_) => {
                if theme::icon_button(ui, icons::TRASH, "Remove from list").clicked() {
                    actions.push(Action::Remove(job.id));
                }
                if theme::icon_button(ui, icons::ARROW_CLOCKWISE, "Download anyway").clicked() {
                    actions.push(Action::RetryAnyway(job.id));
                }
            }
            JobStatus::Failed(_) | JobStatus::Cancelled => {
                if theme::icon_button(ui, icons::TRASH, "Remove from list").clicked() {
                    actions.push(Action::Remove(job.id));
                }
                if theme::icon_button(ui, icons::ARROW_CLOCKWISE, "Retry").clicked() {
                    actions.push(Action::Retry(job.id));
                }
            }
        }
        ui.add_space(12.0);

        // Status column
        let status_w = (ui.available_width() * 0.42).clamp(180.0, 340.0);
        ui.allocate_ui_with_layout(
            vec2(status_w, 48.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(status_w);
                status_cell(ui, job, status_w);
            },
        );
        ui.add_space(16.0);

        // Title column takes the rest
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.add(egui::Label::new(theme::strong(&job.title)).truncate())
                    .on_hover_text(&job.title);
                ui.add(egui::Label::new(RichText::new(&job.subtitle).small().color(theme::MUTED)).truncate());
            });
        });
    });
}

fn status_cell(ui: &mut Ui, job: &Job, width: f32) {
    let line = |ui: &mut Ui, text: RichText| {
        ui.add(egui::Label::new(text.small()).truncate());
    };
    match &job.status {
        JobStatus::Queued => {
            ui.add_space(14.0);
            let text = match job.not_before {
                Some(t) => format!("{} {}", icons::CLOCK, starts_at(t)),
                None => "Waiting…".to_owned(),
            };
            line(ui, RichText::new(text).color(theme::MUTED));
        }
        JobStatus::Running { progress, detail } => {
            ui.add_space(6.0);
            line(ui, RichText::new(detail).color(theme::TEXT));
            theme::progress_line(ui, width, *progress);
        }
        JobStatus::Processing(msg) => {
            ui.add_space(6.0);
            line(ui, RichText::new(msg).color(theme::TEXT));
            theme::progress_line(ui, width, None);
        }
        JobStatus::Done(path) => {
            ui.add_space(6.0);
            line(
                ui,
                RichText::new(format!("{} Done", icons::CHECK_CIRCLE)).color(theme::TEXT),
            );
            if let Some(name) = path.as_ref().and_then(|p| p.file_name()) {
                line(ui, RichText::new(name.to_string_lossy()).color(theme::MUTED));
            }
        }
        JobStatus::Failed(err) => {
            ui.add_space(6.0);
            line(
                ui,
                RichText::new(format!("{} Failed", icons::WARNING_CIRCLE)).color(theme::ACCENT_HOVER),
            );
            let first = err.lines().next().unwrap_or("");
            ui.add(egui::Label::new(RichText::new(first).small().color(theme::MUTED)).truncate())
                .on_hover_text(err);
        }
        JobStatus::Skipped(why) => {
            ui.add_space(14.0);
            line(
                ui,
                RichText::new(format!("{} {why}", icons::CHECK_CIRCLE)).color(theme::MUTED),
            );
        }
        JobStatus::Cancelled => {
            ui.add_space(14.0);
            line(ui, RichText::new("Cancelled").color(theme::MUTED));
        }
    }
}

/// "Starts at 14:05", "Starts tomorrow at 02:00" or a full date further out.
pub fn starts_at(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(ts, 0).single() else {
        return "Scheduled".into();
    };
    let today = Local::now().date_naive();
    let day = t.date_naive();
    if day == today {
        format!("Starts at {}", t.format("%H:%M"))
    } else if day == today.succ_opt().unwrap_or(today) {
        format!("Starts tomorrow at {}", t.format("%H:%M"))
    } else {
        format!("Starts {}", t.format("%b %-d at %H:%M"))
    }
}
