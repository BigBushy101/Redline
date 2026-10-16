//! Library page: playlists of links and local files, and the Now playing queue.

use std::path::PathBuf;

use eframe::egui::{
    self, Align, CornerRadius, Frame, Key, Layout, Margin, RichText, Sense, Spinner, TextEdit, Ui,
    UiBuilder, vec2,
};
use egui_phosphor::regular as icons;
use redline_core::library::{self, PlaylistId, Repeat, Track};
use redline_core::player::Source;

use crate::app::App;
use crate::format::fmt_duration;
use crate::task::Task;
use crate::theme;
use crate::tools::ytdlp;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    NowPlaying,
    Playlist(PlaylistId),
}

/// Tracks being looked up or scanned for a playlist.
struct Adding {
    playlist: PlaylistId,
    what: String,
    task: Task<Result<Vec<Track>, String>>,
}

#[derive(Default)]
pub struct LibraryPage {
    pub view: Option<View>,
    link: String,
    adding: Vec<Adding>,
    error: Option<String>,
    /// Playlist being renamed, and the name so far.
    renaming: Option<(PlaylistId, String)>,
    /// Delete was clicked once; the next click deletes.
    confirm_delete: Option<PlaylistId>,
    search: String,
}

impl LibraryPage {
    /// Takes in finished lookups and scans. Called every frame, whichever page is showing.
    pub fn poll(&mut self, lib: &mut library::Library) {
        let mut i = 0;
        while i < self.adding.len() {
            match self.adding[i].task.take() {
                Some(result) => {
                    let a = self.adding.remove(i);
                    match result {
                        Ok(tracks) if tracks.is_empty() => {
                            self.error = Some(format!("Nothing playable in {}.", a.what));
                        }
                        Ok(tracks) => lib.add(a.playlist, tracks),
                        Err(e) => self.error = Some(e),
                    }
                }
                None => i += 1,
            }
        }
    }

    fn scan(&mut self, app_wake: &redline_core::Wake, tools: &crate::tools::ToolPaths, playlist: PlaylistId, paths: Vec<PathBuf>) {
        let what = match paths.as_slice() {
            [one] => one.file_name().map_or_else(|| one.display().to_string(), |n| n.to_string_lossy().into_owned()),
            many => format!("{} items", many.len()),
        };
        let tools = tools.clone();
        self.error = None;
        self.adding.push(Adding {
            playlist,
            what,
            task: Task::spawn(app_wake, move || Ok(library::scan(&tools, &paths))),
        });
    }
}

/// Adds dropped files to the playlist on screen. Returns false if no playlist is showing.
pub fn drop_files(app: &mut App, paths: Vec<PathBuf>) -> bool {
    let Some(View::Playlist(id)) = app.library_page.view else {
        return false;
    };
    if app.library.get(id).is_none() {
        return false;
    }
    let (wake, tools) = (app.wake.clone(), app.tools.clone());
    app.library_page.scan(&wake, &tools, id, paths);
    true
}

pub fn show(ui: &mut Ui, app: &mut App) {
    let lib = &app.library;
    let total: usize = lib.playlists.iter().map(|p| p.tracks.len()).sum();
    theme::page_header(
        ui,
        "Library",
        "Your playlists",
        &format!(
            "{} playlist{} · {total} track{}",
            lib.playlists.len(),
            if lib.playlists.len() == 1 { "" } else { "s" },
            if total == 1 { "" } else { "s" },
        ),
    );

    // Fall back to something that exists.
    let page = &mut app.library_page;
    let valid = |v: &View| match v {
        View::NowPlaying => true,
        View::Playlist(id) => app.library.get(*id).is_some(),
    };
    if !page.view.as_ref().is_some_and(valid) {
        page.view = app
            .library
            .playlists
            .first()
            .map(|p| View::Playlist(p.id))
            .or((!app.now.is_empty()).then_some(View::NowPlaying));
    }

    // Playlist chips
    ui.horizontal_wrapped(|ui| {
        let mut options: Vec<(Option<View>, String)> = Vec::new();
        if !app.now.is_empty() {
            options.push((Some(View::NowPlaying), format!("{}  Now playing", icons::SPEAKER_HIGH)));
        }
        for p in &app.library.playlists {
            options.push((Some(View::Playlist(p.id)), p.name.clone()));
        }
        let opts: Vec<(Option<View>, &str)> = options.iter().map(|(v, l)| (*v, l.as_str())).collect();
        theme::chips(ui, &mut app.library_page.view, &opts);
        ui.add_space(4.0);
        if theme::ghost_button(ui, &format!("{}  New playlist", icons::PLUS), true).clicked() {
            let id = app.library.create("");
            let name = app.library.get(id).map(|p| p.name.clone()).unwrap_or_default();
            app.library_page.view = Some(View::Playlist(id));
            app.library_page.renaming = Some((id, name));
        }
    });
    ui.add_space(16.0);

    match app.library_page.view {
        None => empty_state(ui),
        Some(View::NowPlaying) => now_playing(ui, app),
        Some(View::Playlist(id)) => playlist(ui, app, id),
    }
}

fn empty_state(ui: &mut Ui) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icons::PLAYLIST).size(64.0).color(theme::SUBTLE));
        ui.add_space(8.0);
        ui.label(theme::heading("Make your first playlist"));
        ui.label(theme::muted(
            "Mix links from YouTube, SoundCloud and more with songs on this PC. Links stream when you play them.",
        ));
    });
}

enum RowAction {
    Play(usize),
    Up(usize),
    Down(usize),
    Remove(usize),
    Offline(usize),
}

fn playlist(ui: &mut Ui, app: &mut App, id: PlaylistId) {
    let Some(pl) = app.library.get(id) else { return };
    let (name, count, secs) = (pl.name.clone(), pl.tracks.len(), pl.duration());

    // Header: art, name, meta, actions
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            theme::art_tile(ui, 96.0, icons::PLAYLIST, theme::accent().base);
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(theme::eyebrow("Playlist"));
                let page = &mut app.library_page;
                match &mut page.renaming {
                    Some((rid, text)) if *rid == id => {
                        let r = ui.add(
                            TextEdit::singleline(text)
                                .font(egui::FontId::new(26.0, theme::bold()))
                                .desired_width(360.0),
                        );
                        if !r.has_focus() && !r.lost_focus() {
                            r.request_focus();
                        }
                        if r.lost_focus() {
                            let text = text.clone();
                            page.renaming = None;
                            if !ui.input(|i| i.key_pressed(Key::Escape)) {
                                app.library.rename(id, &text);
                            }
                        }
                    }
                    _ => {
                        let r = ui.add(
                            egui::Label::new(RichText::new(&name).family(theme::bold()).size(26.0).color(theme::TEXT))
                                .sense(Sense::click()),
                        );
                        if r.on_hover_text("Double-click to rename").double_clicked() {
                            page.renaming = Some((id, name.clone()));
                        }
                    }
                }
                let mut meta = format!("{count} track{}", if count == 1 { "" } else { "s" });
                if secs > 0.0 {
                    meta.push_str(&format!(" · {}", fmt_duration(secs)));
                }
                ui.label(theme::muted(meta));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::small_pill_button(ui, &format!("{}  Play", icons::PLAY), count > 0).clicked() {
                        app.play_playlist(id, 0, false);
                    }
                    if theme::ghost_button(ui, &format!("{}  Shuffle", icons::SHUFFLE), count > 0).clicked() {
                        app.play_playlist(id, 0, true);
                    }
                    if theme::icon_button(ui, icons::PENCIL_SIMPLE, "Rename").clicked() {
                        app.library_page.renaming = Some((id, name.clone()));
                    }
                    let confirming = app.library_page.confirm_delete == Some(id);
                    let tip = if confirming { "Click again to delete" } else { "Delete playlist" };
                    let icon = if confirming { icons::WARNING } else { icons::TRASH };
                    if theme::icon_button(ui, icon, tip).clicked() {
                        if confirming {
                            app.library.delete(id);
                            app.library_page.confirm_delete = None;
                            app.library_page.view = None;
                        } else {
                            app.library_page.confirm_delete = Some(id);
                        }
                    }
                    if confirming {
                        ui.label(RichText::new("Click again to delete").small().color(theme::accent().hover));
                    }
                });
            });
        });
    });
    if app.library.get(id).is_none() {
        return;
    }
    ui.add_space(12.0);
    add_bar(ui, app, id);
    ui.add_space(12.0);

    let page = &app.library_page;
    let query = page.search.trim().to_lowercase();
    let current = app.now_playing_source();
    let tracks = app.library.get(id).map(|p| p.tracks.clone()).unwrap_or_default();
    if tracks.is_empty() {
        ui.label(theme::muted("Add songs with the buttons above, or drop files and folders here."));
        return;
    }
    if tracks.len() > 8 {
        search_box(ui, &mut app.library_page.search);
        ui.add_space(8.0);
    }
    list_header(ui);
    let mut action = None;
    let n = tracks.len();
    for (idx, t) in tracks.iter().enumerate() {
        if !query.is_empty() && !t.matches(&query) {
            continue;
        }
        let local = library::playable_source(t, &app.history);
        let playing = current.as_ref().is_some_and(|c| *c == t.source || *c == local);
        let offline = t.is_link() && matches!(local, Source::Local(_));
        let row = track_row(ui, idx, t, playing, offline, |ui| {
            let mut a = None;
            if theme::icon_button(ui, icons::X, "Remove from playlist").clicked() {
                a = Some(RowAction::Remove(idx));
            }
            if t.is_link()
                && !offline
                && theme::icon_button(ui, icons::DOWNLOAD_SIMPLE, "Download for offline").clicked()
            {
                a = Some(RowAction::Offline(idx));
            }
            if idx + 1 < n && theme::icon_button(ui, icons::CARET_DOWN, "Move down").clicked() {
                a = Some(RowAction::Down(idx));
            }
            if idx > 0 && theme::icon_button(ui, icons::CARET_UP, "Move up").clicked() {
                a = Some(RowAction::Up(idx));
            }
            a
        });
        action = action.or(row);
    }
    match action {
        Some(RowAction::Play(i)) => app.play_playlist(id, i, false),
        Some(RowAction::Up(i)) => app.library.move_track(id, i, i - 1),
        Some(RowAction::Down(i)) => app.library.move_track(id, i, i + 1),
        Some(RowAction::Remove(i)) => app.library.remove_track(id, i),
        Some(RowAction::Offline(i)) => {
            if let Some(job) = library::offline_download(&tracks[i], &app.settings) {
                app.jobs.push(job.title, job.subtitle, job.spec);
            }
        }
        None => {}
    }
}

fn search_box(ui: &mut Ui, text: &mut String) {
    Frame::new()
        .fill(theme::INPUT)
        .corner_radius(255)
        .inner_margin(Margin::symmetric(16, 8))
        .show(ui, |ui| {
            ui.set_width(300.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::MAGNIFYING_GLASS).size(16.0).color(theme::MUTED));
                ui.add(
                    TextEdit::singleline(text)
                        .frame(Frame::NONE)
                        .hint_text("Find in playlist")
                        .desired_width(f32::INFINITY),
                );
            });
        });
}

/// Add files, add a folder, or paste a link (a playlist link adds every entry).
fn add_bar(ui: &mut Ui, app: &mut App, id: PlaylistId) {
    let mut add_link = false;
    ui.horizontal(|ui| {
        if theme::ghost_button(ui, &format!("{}  Add files", icons::FILE_PLUS), true).clicked()
            && let Some(files) = rfd::FileDialog::new().pick_files()
        {
            let (wake, tools) = (app.wake.clone(), app.tools.clone());
            app.library_page.scan(&wake, &tools, id, files);
        }
        if theme::ghost_button(ui, &format!("{}  Add folder", icons::FOLDER_PLUS), true).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            let (wake, tools) = (app.wake.clone(), app.tools.clone());
            app.library_page.scan(&wake, &tools, id, vec![dir]);
        }
        ui.add_space(8.0);
        Frame::new()
            .fill(theme::INPUT)
            .corner_radius(255)
            .inner_margin(Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.set_width(340.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::LINK).size(16.0).color(theme::MUTED));
                    let r = ui.add(
                        TextEdit::singleline(&mut app.library_page.link)
                            .frame(Frame::NONE)
                            .hint_text("Paste a song or playlist link")
                            .desired_width(f32::INFINITY),
                    );
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        add_link = true;
                    }
                });
            });
        let can = app.tools.ytdlp.is_some() && !app.library_page.link.trim().is_empty();
        if theme::ghost_button(ui, "Add link", can).clicked() {
            add_link = true;
        }
    });
    let url = app.library_page.link.trim().to_owned();
    if add_link && !url.is_empty() && app.tools.ytdlp.is_some() {
        let (tools, net) = (app.tools.clone(), app.settings.network.clone());
        let fetch = url.clone();
        let page = &mut app.library_page;
        page.error = None;
        page.link.clear();
        page.adding.push(Adding {
            playlist: id,
            what: url,
            task: Task::spawn(&app.wake, move || {
                ytdlp::fetch_info(&tools, &net, &fetch)
                    .map(|info| library::tracks_from_info(&fetch, &info))
                    .map_err(|e| format!("{e:#}"))
            }),
        });
    }
    let page = &mut app.library_page;
    for a in page.adding.iter().filter(|a| a.playlist == id) {
        ui.horizontal(|ui| {
            ui.add(Spinner::new().size(14.0).color(theme::accent().base));
            ui.add(egui::Label::new(theme::muted(format!("Adding {}…", a.what))).truncate());
        });
    }
    if let Some(e) = page.error.clone() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::WARNING_CIRCLE).color(theme::accent().base));
            ui.add(egui::Label::new(RichText::new(e).color(theme::TEXT)).wrap());
            if ui.link(RichText::new("Dismiss").small().color(theme::SUBTLE)).clicked() {
                page.error = None;
            }
        });
    }
}

fn now_playing(ui: &mut Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label(theme::heading(format!("Now playing · {}", app.now.tracks.len())));
        ui.add_space(12.0);
        let from = app.now.playlist.and_then(|id| app.library.get(id)).map(|p| p.name.clone());
        if let Some(name) = from {
            ui.label(theme::muted(format!("From {name}")));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::ghost_button(ui, "Clear", true).clicked() {
                app.stop_queue();
            }
            let repeat = match app.now.repeat {
                Repeat::Off => "Repeat off",
                Repeat::All => "Repeat all",
                Repeat::One => "Repeat one",
            };
            ui.label(theme::muted(format!(
                "{} · {repeat}",
                if app.now.shuffle { "Shuffle on" } else { "In order" }
            )));
        });
    });
    ui.add_space(8.0);
    list_header(ui);
    let current = app.now.current_index();
    let rows: Vec<(usize, Track)> = app.now.in_order().map(|(i, t)| (i, t.clone())).collect();
    let mut jump = None;
    let mut remove = None;
    for (n, (idx, t)) in rows.iter().enumerate() {
        let offline = t.is_link() && matches!(library::playable_source(t, &app.history), Source::Local(_));
        let r = track_row(ui, n, t, current == Some(*idx), offline, |ui| {
            (current != Some(*idx) && theme::icon_button(ui, icons::X, "Remove from queue").clicked())
                .then_some(RowAction::Remove(*idx))
        });
        match r {
            Some(RowAction::Play(_)) => jump = Some(*idx),
            Some(RowAction::Remove(i)) => remove = Some(i),
            _ => {}
        }
    }
    if let Some(i) = jump {
        app.jump_to(i);
    }
    if let Some(i) = remove {
        app.now.remove(i);
    }
}

fn list_header(ui: &mut Ui) {
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.add_sized([28.0, 16.0], egui::Label::new(theme::eyebrow("#")));
        ui.add_space(52.0);
        ui.label(theme::eyebrow("Title"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(170.0);
            ui.add_sized([60.0, 16.0], egui::Label::new(theme::eyebrow("Time")));
            ui.add_sized([96.0, 16.0], egui::Label::new(theme::eyebrow("Source")));
        });
    });
    let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(sep, 0.0, egui::Color32::from_gray(0x2A));
    ui.add_space(4.0);
}

/// A playlist-style row. Double-click asks to play it; `actions` draws the buttons on the
/// right, right to left.
fn track_row(
    ui: &mut Ui,
    idx: usize,
    t: &Track,
    playing: bool,
    offline: bool,
    actions: impl FnOnce(&mut Ui) -> Option<RowAction>,
) -> Option<RowAction> {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 56.0), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    if hovered || playing {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(4),
            if playing { theme::ELEVATED } else { theme::CARD_HOVER },
        );
    }
    let mut action = resp.double_clicked().then_some(RowAction::Play(idx));
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(12.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let number = if playing {
        RichText::new(icons::SPEAKER_HIGH).color(theme::accent().base)
    } else {
        theme::muted((idx + 1).to_string())
    };
    row.add_sized([28.0, 20.0], egui::Label::new(number));
    row.add_space(4.0);
    let icon = if t.is_link() { icons::LINK } else { icons::MUSIC_NOTES };
    theme::cover(&mut row, 40.0, t.thumbnail.as_deref(), icon);
    row.add_space(8.0);
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.allocate_ui_with_layout(vec2(160.0, 40.0), Layout::right_to_left(Align::Center), |ui| {
            // Fixed width, so the columns line up whatever buttons a row has.
            ui.set_min_width(160.0);
            if let Some(a) = actions(ui) {
                action = Some(a);
            }
        });
        ui.add_space(10.0);
        let time = t.duration.map_or("".to_owned(), fmt_duration);
        ui.add_sized([60.0, 20.0], egui::Label::new(theme::muted(time)));
        let (src_icon, src) = match (&t.source, offline) {
            (Source::Link(_), true) => (icons::CHECK_CIRCLE, "Downloaded"),
            (Source::Link(_), false) => (icons::GLOBE, "Stream"),
            (Source::Local(_), _) => (icons::HARD_DRIVE, "On this PC"),
        };
        let hover = match &t.source {
            Source::Link(u) => u.clone(),
            Source::Local(p) => p.display().to_string(),
        };
        ui.add_sized(
            [96.0, 20.0],
            egui::Label::new(RichText::new(format!("{src_icon} {src}")).small().color(theme::MUTED)),
        )
        .on_hover_text(hover);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.vertical(|ui| {
                ui.add_space(2.0);
                let title = if playing {
                    RichText::new(&t.title).family(theme::bold()).color(theme::accent().hover)
                } else {
                    theme::strong(&t.title)
                };
                ui.add(egui::Label::new(title).truncate());
                ui.add(
                    egui::Label::new(RichText::new(t.artist.clone().unwrap_or_default()).small().color(theme::MUTED))
                        .truncate(),
                );
            });
        });
    });
    action
}

/// Where the "Add to playlist" menu puts tracks.
#[derive(Clone, Copy)]
pub enum AddTarget {
    Queue,
    Playlist(PlaylistId),
    NewPlaylist,
}

/// Playlist ids and names, for [`add_menu`].
pub fn playlist_names(lib: &library::Library) -> Vec<(PlaylistId, String)> {
    lib.playlists.iter().map(|p| (p.id, p.name.clone())).collect()
}

/// "Add to playlist" button with a menu of playlists. Returns the pick.
pub fn add_menu(ui: &mut Ui, playlists: &[(PlaylistId, String)]) -> Option<AddTarget> {
    let resp = theme::icon_button(ui, icons::PLUS_CIRCLE, "Add to playlist…");
    let mut pick = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);
        if ui.button(format!("{}  Now playing queue", icons::QUEUE)).clicked() {
            pick = Some(AddTarget::Queue);
        }
        ui.separator();
        for (id, name) in playlists {
            if ui.button(format!("{}  {name}", icons::PLAYLIST)).clicked() {
                pick = Some(AddTarget::Playlist(*id));
            }
        }
        if ui.button(format!("{}  New playlist", icons::PLUS)).clicked() {
            pick = Some(AddTarget::NewPlaylist);
        }
    });
    pick
}

/// Puts `tracks` where the menu said.
pub fn add_to(app: &mut App, target: AddTarget, tracks: Vec<Track>) {
    match target {
        AddTarget::Queue => app.queue_tracks(tracks),
        AddTarget::Playlist(id) => app.library.add(id, tracks),
        AddTarget::NewPlaylist => {
            let id = app.library.create("");
            app.library.add(id, tracks);
        }
    }
}
