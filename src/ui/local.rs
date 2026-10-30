//! Local page: the songs on this PC, from the Music folder or the folders picked here.

use std::path::PathBuf;

use eframe::egui::{self, Frame, Margin, RichText, Spinner, Ui, vec2};
use egui_phosphor::regular as icons;
use redline_core::library::Track;
use redline_core::local;
use redline_core::player::Source;

use super::playlists::{add_menu, add_to, playlist_names};
use super::tracks::{RowAction, RowState, convert_button, list_header, search_box, track_row};
use crate::app::{App, Page};
use crate::format::fmt_duration;
use crate::task::Task;
use crate::theme;

#[derive(Default)]
pub struct LocalPage {
    pub tracks: Vec<Track>,
    scan: Option<Task<Vec<Track>>>,
    /// The folders and added files `tracks` (or the scan under way) came from; a change
    /// scans again.
    scanned: Option<(Vec<PathBuf>, Vec<PathBuf>)>,
    search: String,
}

impl LocalPage {
    /// Whether a scan is under way (the Saved card says so while it runs).
    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }
}

/// Scans when the folders change (and once at launch), and takes in finished scans. Called
/// every frame, whichever page is showing.
pub fn poll(app: &mut App) {
    let page = &mut app.local_page;
    if let Some(tracks) = page.scan.as_ref().and_then(Task::take) {
        page.tracks = tracks;
        page.scan = None;
    }
    let want = (app.settings.local_folders.clone(), app.settings.local_files.clone());
    if page.scan.is_none() && page.scanned.as_ref() != Some(&want) {
        let tools = app.tools.clone();
        page.scanned = Some(want.clone());
        page.scan = Some(Task::spawn(&app.wake, move || local::scan(&tools, &want.0, &want.1)));
    }
}

pub fn show(ui: &mut Ui, app: &mut App) {
    let mut back = false;
    ui.horizontal(|ui| {
        if theme::ghost_button(ui, &format!("{}  Saved", icons::ARROW_LEFT), true)
            .on_hover_text("Back to Saved")
            .clicked()
        {
            back = true;
        }
    });
    if back {
        app.page = Page::Saved(None);
        app.saved_page.open = None;
        return;
    }
    ui.add_space(4.0);
    let secs: f64 = app.local_page.tracks.iter().filter_map(|t| t.duration).sum();
    let n = app.local_page.tracks.len();
    let mut meta = format!("{n} song{}", if n == 1 { "" } else { "s" });
    if secs > 0.0 {
        meta.push_str(&format!(" · {}", fmt_duration(secs)));
    }
    theme::page_header(ui, "Local", "On this PC", &meta);

    folders(ui, app);
    ui.add_space(12.0);

    let count = app.local_page.tracks.len();
    ui.horizontal(|ui| {
        if theme::small_pill_button(ui, &format!("{}  Play", icons::PLAY), count > 0).clicked() {
            app.play_list(app.local_page.tracks.clone(), 0, false);
        }
        if theme::ghost_button(ui, &format!("{}  Shuffle", icons::SHUFFLE), count > 0).clicked() {
            app.play_list(app.local_page.tracks.clone(), 0, true);
        }
        let scanning = app.local_page.scan.is_some();
        if theme::ghost_button(ui, &format!("{}  Rescan", icons::ARROWS_CLOCKWISE), !scanning).clicked() {
            app.local_page.scanned = None;
        }
        if scanning {
            ui.add(Spinner::new().size(14.0).color(theme::accent().base));
            ui.label(theme::muted("Looking for songs…"));
        }
    });
    ui.add_space(12.0);

    if count == 0 {
        if app.local_page.scan.is_none() {
            empty_state(ui, app.settings.local_folders.is_empty());
        }
        return;
    }
    if count > 8 {
        search_box(ui, &mut app.local_page.search, "Find a song");
        ui.add_space(8.0);
    }
    list_header(ui);
    let query = app.local_page.search.trim().to_lowercase();
    let current = app.now_playing_source();
    let names = playlist_names(&app.library);
    let mut action = None;
    let mut add = None;
    let mut forget = None;
    let w = ui.available_width();
    let folders = &app.settings.local_folders;
    for (idx, t) in app.local_page.tracks.iter().enumerate() {
        if !query.is_empty() && !t.matches(&query) {
            continue;
        }
        // Only rows on screen are drawn; the rest just take their room.
        if !ui.is_rect_visible(egui::Rect::from_min_size(ui.cursor().min, vec2(w, 56.0))) {
            ui.allocate_space(vec2(w, 56.0));
            continue;
        }
        let state = RowState {
            playing: current.as_ref() == Some(&t.source),
            ..Default::default()
        };
        let added = match &t.source {
            Source::Local(p) => {
                app.settings.local_files.contains(p) && !folders.iter().any(|f| p.starts_with(f))
            }
            Source::Link(_) => false,
        };
        let row = track_row(ui, idx, t, state, |ui| {
            if added && theme::icon_button(ui, icons::X, "Remove from Local").clicked() {
                forget = Some(idx);
            }
            if let Some(target) = add_menu(ui, &names) {
                add = Some((target, idx));
            }
            convert_button(ui, idx, true)
        });
        action = action.or(row);
    }
    match action {
        Some(RowAction::Play(i)) => app.play_list(app.local_page.tracks.clone(), i, false),
        Some(RowAction::Convert(i)) => {
            if let Source::Local(p) = &app.local_page.tracks[i].source {
                app.convert_file(p.clone());
            }
        }
        _ => {}
    }
    if let Some(i) = forget
        && let Source::Local(p) = &app.local_page.tracks[i].source
    {
        let p = p.clone();
        app.settings.local_files.retain(|f| *f != p);
    }
    if let Some((target, i)) = add {
        let track = app.local_page.tracks[i].clone();
        let title = track.title.clone();
        add_to(app, target, vec![track]);
        app.toast(format!("Added “{title}”"));
    }
}

/// The folders searched, each with a remove button, the files added on Convert, and Add
/// folder.
fn folders(ui: &mut Ui, app: &mut App) {
    let mut remove = None;
    let mut clear = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::muted(format!("{}  From", icons::FOLDER_SIMPLE)));
        for (i, dir) in app.settings.local_folders.iter().enumerate() {
            Frame::new()
                .fill(theme::CARD)
                .corner_radius(255)
                .inner_margin(Margin { left: 12, right: 4, top: 2, bottom: 2 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let name = dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into_owned());
                        ui.label(theme::strong(name)).on_hover_text(dir.display().to_string());
                        if theme::icon_button(ui, icons::X, "Stop listing this folder").clicked() {
                            remove = Some(i);
                        }
                    });
                });
        }
        if theme::ghost_button(ui, &format!("{}  Add folder", icons::FOLDER_PLUS), true).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
            && !app.settings.local_folders.contains(&dir)
        {
            app.settings.local_folders.push(dir);
        }
        let added = app.settings.local_files.len();
        if added > 0 {
            Frame::new()
                .fill(theme::CARD)
                .corner_radius(255)
                .inner_margin(Margin { left: 12, right: 4, top: 2, bottom: 2 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label = format!("{}  Added files: {added}", icons::FILE_AUDIO);
                        ui.label(theme::strong(label))
                            .on_hover_text("Songs added on the Convert page");
                        if theme::icon_button(ui, icons::X, "Clear the added files").clicked() {
                            clear = true;
                        }
                    });
                });
        }
        let defaults = local::default_folders();
        if app.settings.local_folders != defaults
            && !defaults.is_empty()
            && ui.link(RichText::new("Reset to Music").small().color(theme::SUBTLE)).clicked()
        {
            app.settings.local_folders = defaults;
        }
    });
    if let Some(i) = remove {
        app.settings.local_folders.remove(i);
    }
    if clear {
        app.settings.local_files.clear();
    }
}

fn empty_state(ui: &mut Ui, no_folders: bool) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icons::MUSIC_NOTES).size(64.0).color(theme::SUBTLE));
        ui.add_space(8.0);
        if no_folders {
            ui.label(theme::heading("Pick a folder"));
            ui.label(theme::muted("Add the folders your music is in, and their songs show up here."));
        } else {
            ui.label(theme::heading("No songs here yet"));
            ui.label(theme::muted("Songs in these folders (MP3, FLAC, M4A, WAV, Ogg and more) show up here."));
        }
    });
}
