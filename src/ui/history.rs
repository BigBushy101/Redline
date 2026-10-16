use eframe::egui::{
    self, Align, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, TextEdit, Ui,
    UiBuilder, vec2,
};
use egui_phosphor::regular as icons;

use crate::app::{App, Page};
use crate::format::fmt_timestamp;
use crate::history::HistoryEntry;
use crate::jobs::JobSpec;
use crate::theme;
use redline_core::library::{PlaylistId, Track};
use redline_core::player::Source;

use super::playlists::{AddTarget, add_menu, add_to, playlist_names};
use crate::tools;

#[derive(Default)]
pub struct HistoryPage {
    search: String,
}

enum Action {
    Again(usize),
    Remove(usize),
    Open(std::path::PathBuf),
    Play(Track),
    Add(AddTarget, Track),
    Reveal(std::path::PathBuf),
    Save(String),
}

pub fn show(ui: &mut Ui, app: &mut App) {
    let n = app.history.entries.len();
    theme::page_header(
        ui,
        "History",
        "Recently finished",
        &format!("{n} item{}", if n == 1 { "" } else { "s" }),
    );

    if n == 0 {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(icons::CLOCK_COUNTER_CLOCKWISE).size(64.0).color(theme::SUBTLE));
            ui.add_space(8.0);
            ui.label(theme::heading("Nothing here yet"));
            ui.label(theme::muted("Finished downloads and conversions are kept here."));
        });
        return;
    }

    ui.horizontal(|ui| {
        Frame::new()
            .fill(theme::INPUT)
            .corner_radius(255)
            .inner_margin(Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.set_width(320.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::MAGNIFYING_GLASS).size(16.0).color(theme::MUTED));
                    ui.add(
                        TextEdit::singleline(&mut app.history_page.search)
                            .frame(Frame::NONE)
                            .hint_text("Search titles, links and files")
                            .font(FontId::proportional(14.0))
                            .desired_width(f32::INFINITY),
                    );
                });
            });
        ui.add_space(8.0);
        if theme::ghost_button(ui, "Clear history", true).clicked() {
            app.history.clear();
        }
    });
    ui.add_space(16.0);

    let query = app.history_page.search.trim().to_owned();
    let mut actions = Vec::new();
    let playlists = playlist_names(&app.library);
    let mut shown = 0;
    for (idx, entry) in app.history.entries.iter().enumerate() {
        if !query.is_empty() && !entry.matches(&query) {
            continue;
        }
        shown += 1;
        entry_row(ui, idx, entry, &playlists, &mut actions);
        ui.add_space(4.0);
    }
    if shown == 0 {
        ui.label(theme::muted("No matches."));
    }

    for action in actions {
        match action {
            Action::Again(idx) => {
                let entry = app.history.entries[idx].clone();
                let mut spec = entry.spec;
                // Asked for explicitly, so don't let the archive skip it.
                if let JobSpec::Download(r) = &mut spec {
                    r.archive = None;
                }
                app.jobs.push(entry.title, entry.subtitle, spec);
                app.page = Page::Queue;
            }
            Action::Remove(idx) => app.history.remove(idx),
            Action::Open(path) => {
                let _ = open::that_detached(path);
            }
            Action::Play(track) => app.play_tracks(vec![track], 0, None),
            Action::Add(target, track) => add_to(app, target, vec![track]),
            Action::Reveal(path) => tools::reveal(&path),
            Action::Save(url) => super::saved::start_save(app, &url),
        }
    }
}

/// The entry as a playlist track: a download's link (which plays its file while that's
/// there), or a conversion's output.
fn as_track(entry: &HistoryEntry, output: Option<&std::path::PathBuf>) -> Option<Track> {
    let source = match (entry.url(), output) {
        (Some(url), _) => Source::Link(url.to_owned()),
        (None, Some(p)) if super::playable(p) => Source::Local(p.clone()),
        _ => return None,
    };
    let title = match &entry.spec {
        JobSpec::Download(_) => entry.title.clone(),
        _ => source.label(),
    };
    Some(Track::new(source, title))
}

fn entry_row(
    ui: &mut Ui,
    idx: usize,
    entry: &HistoryEntry,
    playlists: &[(PlaylistId, String)],
    actions: &mut Vec<Action>,
) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
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
    let icon = match entry.spec {
        JobSpec::Download(_) => icons::DOWNLOAD_SIMPLE,
        _ => icons::ARROWS_LEFT_RIGHT,
    };
    theme::art_tile(&mut row, 40.0, icon, theme::MUTED);
    row.add_space(10.0);

    let output = entry.output.clone().filter(|p| p.exists());
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if theme::icon_button(ui, icons::TRASH, "Remove from history").clicked() {
            actions.push(Action::Remove(idx));
        }
        let again = match entry.spec {
            JobSpec::Download(_) => "Download again",
            _ => "Convert again",
        };
        if theme::icon_button(ui, icons::ARROW_CLOCKWISE, again).clicked() {
            actions.push(Action::Again(idx));
        }
        if let Some(path) = &output {
            if theme::icon_button(ui, icons::FOLDER_OPEN, "Show in folder").clicked() {
                actions.push(Action::Reveal(path.clone()));
            }
            if path.is_file() && theme::icon_button(ui, icons::ARROW_SQUARE_OUT, "Open in its app").clicked() {
                actions.push(Action::Open(path.clone()));
            }
        }
        if let Some(url) = entry.url()
            && theme::icon_button(ui, icons::BOOKMARK_SIMPLE, "Save").clicked()
        {
            actions.push(Action::Save(url.to_owned()));
        }
        if let Some(track) = as_track(entry, output.as_ref()) {
            if let Some(target) = add_menu(ui, playlists) {
                actions.push(Action::Add(target, track.clone()));
            }
            // The downloaded file if it's still there, otherwise stream the link.
            let tip = if output.as_ref().is_some_and(|p| super::playable(p)) { "Play" } else { "Stream" };
            if theme::icon_button(ui, icons::PLAY, tip).clicked() {
                actions.push(Action::Play(track));
            }
        }
        ui.add_space(12.0);
        ui.add_sized(
            [140.0, 20.0],
            egui::Label::new(RichText::new(fmt_timestamp(entry.finished_at)).small().color(theme::MUTED)),
        );
        ui.add_space(12.0);

        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.add(egui::Label::new(theme::strong(&entry.title)).truncate())
                    .on_hover_text(entry.url().unwrap_or(&entry.title));
                let mut sub = entry.subtitle.clone();
                if entry.output.is_some() && output.is_none() {
                    sub.push_str(" · file moved or deleted");
                }
                ui.add(egui::Label::new(RichText::new(sub).small().color(theme::MUTED)).truncate());
            });
        });
    });
}
