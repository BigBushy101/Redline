//! Bottom bar: the player while a track is loaded, otherwise the current job. Styled like
//! Spotify's player bar.

use eframe::egui::{self, Align, Layout, Rect, RichText, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icons;
use redline_core::player::{Command, Status};

use crate::app::{App, Page};
use crate::format::fmt_duration;
use crate::jobs::JobStatus;
use crate::theme;

pub fn show(ui: &mut Ui, app: &mut App) {
    if app.player_state.is_active() {
        player_bar(ui, app);
    } else {
        job_bar(ui, app);
    }
}

/// Left, centre and right thirds of the bar, centre slightly wider.
fn columns(full: Rect) -> (Rect, Rect, Rect) {
    let third = full.width() / 3.0;
    let left = Rect::from_min_size(full.min, vec2(third * 1.1, full.height()));
    let center = Rect::from_min_size(egui::pos2(full.min.x + third * 1.1, full.min.y), vec2(third * 1.1, full.height()));
    let right = Rect::from_min_max(egui::pos2(center.max.x, full.min.y), full.max);
    (left, center, right)
}

fn player_bar(ui: &mut Ui, app: &mut App) {
    let st = app.player_state.clone();
    let (left, center, right) = columns(ui.max_rect());

    // Left: art + title + artist (or what's going on)
    let mut l = ui.new_child(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)));
    let track = st.track.clone().unwrap_or_default();
    theme::cover(&mut l, 56.0, track.thumbnail.as_deref(), icons::MUSIC_NOTES);
    l.add_space(8.0);
    l.vertical(|ui| {
        ui.add_space(12.0);
        ui.add(egui::Label::new(theme::strong(&track.title)).truncate())
            .on_hover_text(&track.title);
        let (line, color) = match &st.status {
            Status::Loading => ("Loading…".to_owned(), theme::MUTED),
            Status::Error(e) => (e.lines().next().unwrap_or("").to_owned(), theme::ACCENT_HOVER),
            _ if st.buffering => ("Buffering…".to_owned(), theme::MUTED),
            _ => (track.artist.clone().unwrap_or_default(), theme::MUTED),
        };
        let resp = ui.add(egui::Label::new(RichText::new(line).small().color(color)).truncate());
        if let Status::Error(e) = &st.status {
            resp.on_hover_text(e);
        }
    });

    // Centre: transport + seek bar
    let mut c = ui.new_child(
        UiBuilder::new()
            .max_rect(center.shrink2(vec2(8.0, 4.0)))
            .layout(Layout::top_down(Align::Center)),
    );
    c.spacing_mut().item_spacing.y = 2.0;
    let playing = st.is_playing();
    c.horizontal(|ui| {
        let buttons_w = 32.0 * 2.0 + 36.0 + 16.0 * 2.0;
        ui.add_space(((ui.available_width() - buttons_w) / 2.0).max(0.0));
        ui.spacing_mut().item_spacing.x = 16.0;
        if theme::icon_button(ui, icons::SKIP_BACK, "Back to start").clicked() {
            app.player.send(Command::Seek(0.0));
        }
        let can_play = !matches!(st.status, Status::Idle);
        if theme::play_button(ui, playing, can_play)
            .on_hover_text(if playing { "Pause" } else { "Play" })
            .clicked()
        {
            app.player.send(Command::Toggle);
        }
        ui.add_enabled_ui(false, |ui| theme::icon_button(ui, icons::SKIP_FORWARD, "Next"));
    });
    c.horizontal(|ui| {
        let time_w = 44.0;
        let time = |ui: &mut Ui, t: Option<f64>, align: Align| {
            let text = t.map_or("-:--".to_owned(), fmt_duration);
            ui.allocate_ui_with_layout(vec2(time_w, 16.0), Layout::left_to_right(Align::Center).with_main_align(align), |ui| {
                ui.label(RichText::new(text).small().color(theme::MUTED));
            });
        };
        time(ui, Some(st.position), Align::Max);
        let w = (ui.available_width() - time_w - 8.0).max(40.0);
        let frac = st.duration.filter(|d| *d > 0.0).map(|d| (st.position / d) as f32);
        if let Some(f) = theme::seek_bar(ui, w, frac)
            && let Some(d) = st.duration
        {
            let to = f as f64 * d;
            app.player.send(Command::Seek(to));
            // Show the new spot straight away instead of jumping back for a frame.
            app.player_state.position = to;
        }
        time(ui, st.duration, Align::Min);
    });

    // Right: jobs indicator + volume
    let mut r = ui.new_child(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)));
    let mut volume = st.volume;
    if theme::line_slider(&mut r, 96.0, &mut volume).changed() {
        app.player.send(Command::SetVolume(volume));
        app.player_state.volume = volume;
    }
    let speaker = match volume {
        v if v <= 0.0 => icons::SPEAKER_X,
        v if v < 0.5 => icons::SPEAKER_LOW,
        _ => icons::SPEAKER_HIGH,
    };
    if theme::icon_button(&mut r, speaker, if volume > 0.0 { "Mute" } else { "Unmute" }).clicked() {
        let v = if volume > 0.0 { 0.0 } else { app.unmuted_volume };
        if volume > 0.0 {
            app.unmuted_volume = volume;
        }
        app.player.send(Command::SetVolume(v));
    }
    r.add_space(12.0);
    jobs_indicator(&mut r, app);
}

/// Compact "jobs running" chip so downloads stay visible while music plays.
fn jobs_indicator(ui: &mut Ui, app: &mut App) {
    let active = app.jobs.active_count();
    if active == 0 {
        return;
    }
    let progress = app.jobs.current().and_then(|j| match &j.status {
        JobStatus::Running { progress, .. } => *progress,
        _ => None,
    });
    let label = match (active, progress) {
        (1, Some(p)) => format!("{}  1 job · {:.0}%", icons::QUEUE, p * 100.0),
        (1, None) => format!("{}  1 job", icons::QUEUE),
        (n, _) => format!("{}  {n} jobs", icons::QUEUE),
    };
    let resp = ui
        .add(egui::Button::new(RichText::new(label).small().color(theme::MUTED)).frame(false))
        .on_hover_text(app.jobs.current().map_or(String::new(), |j| j.title.clone()));
    if resp.clicked() {
        app.page = Page::Queue;
    }
}

fn job_bar(ui: &mut Ui, app: &mut App) {
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

    let (left, center, right) = columns(full);

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
        JobStatus::Queued => (
            job.not_before
                .map_or("Waiting to start…".to_owned(), crate::format::starts_at),
            Some(0.0),
        ),
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
