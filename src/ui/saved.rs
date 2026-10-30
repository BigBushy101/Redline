//! Saved page: music saved from each platform, grouped by platform. Platform playlists stay
//! in step with the original; single links sit beside them.

use eframe::egui::{self, Align, CornerRadius, Frame, Key, Layout, Margin, RichText, Sense, Spinner, TextEdit, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icons;
use redline_core::format::ago;
use redline_core::library::{self, Track};
use redline_core::linkcheck::{self, LinkStatus};
use redline_core::platform::Platform;
use redline_core::player::Source;
use redline_core::download_plan::PlannedDownload;
use redline_core::saved::{self, CollectionId, Import, SaveOutcome, SavedTrack};

use super::playlists::{AddTarget, add_menu, add_to, playlist_names};
use super::replace::{self, List, Target};
use super::tracks::{RowAction, RowState, convert_button, list_header, replace_button, search_box, track_row};
use crate::app::{App, Page};
use crate::format::fmt_duration;
use crate::task::Task;
use crate::theme;
use crate::tools::ytdlp::{self, MediaInfo};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Collections,
    Songs,
    All,
}

/// A link being looked up before it's saved.
struct Lookup {
    url: String,
    /// Saving alongside a download: no toasts unless saving fails.
    quiet: bool,
    /// One of an imported list of links; counted in [`SavedPage::import`] instead of toasted.
    import: bool,
    task: Task<Result<MediaInfo, String>>,
}

/// A collection being listed fresh from its platform.
struct Sync {
    id: CollectionId,
    task: Task<Result<Vec<Track>, String>>,
}

#[derive(Default)]
pub struct SavedPage {
    link: String,
    lookups: Vec<Lookup>,
    syncs: Vec<Sync>,
    /// The collection on screen, if one is open.
    pub open: Option<CollectionId>,
    tab: Tab,
    search: String,
    /// Collection being renamed, and the name so far.
    renaming: Option<(CollectionId, String)>,
    /// Remove was clicked once; the next click removes.
    confirm_remove: Option<CollectionId>,
    error: Option<String>,
    /// Stale collections still to refresh after launch, one at a time.
    launch_queue: std::collections::VecDeque<CollectionId>,
    launch_current: Option<CollectionId>,
    launch_checked: bool,
    /// An imported list of links being looked up and saved.
    import: Option<ImportTally>,
}

#[derive(Default)]
struct ImportTally {
    /// Links still to look up, one at a time so a long list doesn't start yt-dlp for each.
    waiting: std::collections::VecDeque<String>,
    pending: usize,
    saved: usize,
    failed: usize,
    /// Links in the file that were saved already.
    skipped: usize,
}

/// Collections not refreshed for this long are refreshed on launch.
const STALE_SECS: i64 = 12 * 3600;

impl SavedPage {
    pub fn is_syncing(&self, id: CollectionId) -> bool {
        self.syncs.iter().any(|s| s.id == id)
    }
}

/// Looks up a link in the background and saves it once that's done.
pub fn start_save(app: &mut App, url: &str) {
    start_save_with(app, url, false);
}

/// Looks a link up and saves it, saying nothing unless saving fails; for links downloaded on
/// the Download page without a lookup.
pub fn start_save_quietly(app: &mut App, url: &str) {
    start_save_with(app, url, true);
}

fn start_save_with(app: &mut App, url: &str, quiet: bool) {
    start_lookup(app, url, quiet, false);
}

fn start_lookup(app: &mut App, url: &str, quiet: bool, import: bool) {
    let url = url.trim().to_owned();
    if url.is_empty() {
        return;
    }
    if app.tools.ytdlp.is_none() {
        if quiet {
            return;
        }
        app.toast("Saving links needs yt-dlp. Set it up in Settings.");
        return;
    }
    let (tools, net, fetch) = (app.tools.clone(), app.settings.network.clone(), url.clone());
    app.saved_page.error = None;
    app.saved_page.lookups.push(Lookup {
        url,
        quiet,
        import,
        task: Task::spawn(&app.wake, move || {
            ytdlp::fetch_titled(&tools, &net, &fetch, &|_| false).map_err(|e| format!("{e:#}"))
        }),
    });
}

/// Saves a link that's already been looked up, and says what happened.
pub fn save_info(app: &mut App, info: &MediaInfo, url: &str) {
    save_info_with(app, info, url, false);
}

/// Saves a link being downloaded, so it lands in Saved too; only a failure is shown. A
/// collection saved before is refreshed, but its new songs aren't queued for offline (the
/// download is getting them).
pub fn save_info_quietly(app: &mut App, info: &MediaInfo, url: &str) {
    save_info_with(app, info, url, true);
}

fn save_info_with(app: &mut App, info: &MediaInfo, url: &str, quiet: bool) {
    match app.saved.save_link(info, url) {
        Ok(out) => {
            if quiet {
                return;
            }
            if let SaveOutcome::Refreshed { id, sync, .. } = &out {
                keep_new_offline(app, *id, &sync.added);
            }
            app.toast(out.message());
        }
        Err(e) => app.toast(format!("Couldn't save it to Saved: {e:#}")),
    }
}

/// Queues downloads that aren't already waiting in the queue. Returns how many.
fn queue_downloads(app: &mut App, planned: Vec<PlannedDownload>) -> usize {
    let planned = saved::not_queued(planned, &app.jobs);
    let n = planned.len();
    for job in planned {
        app.jobs.push(job.title, job.subtitle, job.spec);
    }
    n
}

/// After a refresh: downloads for the new tracks of a keep-offline collection.
fn keep_new_offline(app: &mut App, id: CollectionId, added: &[Track]) {
    if !app.saved.get(id).is_some_and(|c| c.keep_offline) || added.is_empty() {
        return;
    }
    let planned = added
        .iter()
        .filter(|t| matches!(&t.source, Source::Link(u) if app.history.find_url(u).is_none()))
        .filter_map(|t| library::offline_download(t, &app.settings))
        .collect();
    queue_downloads(app, planned);
}

/// Refreshes, one at a time, the collections that were stale at launch.
fn launch_refresh(app: &mut App) {
    let page = &mut app.saved_page;
    if !page.launch_checked {
        page.launch_checked = true;
        if app.settings.refresh_saved_on_start && app.tools.ytdlp.is_some() {
            page.launch_queue = app.saved.stale(unix_now(), STALE_SECS).into();
        }
    }
    if app.saved_page.launch_current.is_some_and(|id| app.saved_page.is_syncing(id)) {
        return;
    }
    app.saved_page.launch_current = None;
    while let Some(id) = app.saved_page.launch_queue.pop_front() {
        // Refreshed by hand since launch, or removed: skip it.
        let still_stale = app.saved.get(id).is_some_and(|c| c.synced_at.is_none_or(|t| unix_now() - t >= STALE_SECS));
        if still_stale && !app.saved_page.is_syncing(id) {
            start_sync(app, id);
            app.saved_page.launch_current = Some(id);
            return;
        }
    }
}

/// Lists the collection fresh from its platform in the background.
pub fn start_sync(app: &mut App, id: CollectionId) {
    if app.saved_page.is_syncing(id) || app.tools.ytdlp.is_none() {
        return;
    }
    let Some(c) = app.saved.get(id) else { return };
    let (tools, net, url) = (app.tools.clone(), app.settings.network.clone(), c.source_url.clone());
    let titled = c.titled_links();
    app.saved_page.syncs.push(Sync {
        id,
        task: Task::spawn(&app.wake, move || {
            saved::sync(&tools, &net, &url, &titled).map_err(|e| format!("{e:#}"))
        }),
    });
}

/// Takes in finished lookups and syncs. Called every frame, whichever page is showing.
pub fn poll(app: &mut App) {
    let mut i = 0;
    while i < app.saved_page.lookups.len() {
        let Some(result) = app.saved_page.lookups[i].task.take() else {
            i += 1;
            continue;
        };
        let lookup = app.saved_page.lookups.remove(i);
        if lookup.import {
            let saved = result.and_then(|info| app.saved.save_link(&info, &lookup.url).map_err(|e| format!("{e:#}")));
            if let SaveOutcome::Refreshed { id, sync, .. } = saved.as_ref().unwrap_or(&SaveOutcome::AlreadySaved) {
                let (id, added) = (*id, sync.added.clone());
                keep_new_offline(app, id, &added);
            }
            finish_import_link(app, saved.is_ok());
            continue;
        }
        match result {
            Ok(info) => save_info_with(app, &info, &lookup.url, lookup.quiet),
            // A download's own lookup failing shows on the download.
            Err(_) if lookup.quiet => {}
            Err(e) => {
                app.toast(e.lines().next().unwrap_or("Couldn't look that link up").to_owned());
                app.saved_page.error = Some(e);
            }
        }
    }
    let mut i = 0;
    while i < app.saved_page.syncs.len() {
        let Some(result) = app.saved_page.syncs[i].task.take() else {
            i += 1;
            continue;
        };
        let sync = app.saved_page.syncs.remove(i);
        finish_sync(app, sync.id, result);
    }
    launch_refresh(app);
}

/// Export…: writes Saved to a file the user picks.
fn export_saved(app: &mut App) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Redline Saved", &["json"])
        .set_file_name("redline-saved.json")
        .save_file()
    else {
        return;
    };
    match std::fs::write(&path, app.saved.export_json()) {
        Ok(()) => {
            let songs = app.saved.songs.len() + app.saved.collections.iter().map(|c| c.tracks.len()).sum::<usize>();
            app.toast(format!(
                "Exported {} and {} to {}",
                plural(app.saved.collections.len(), "collection"),
                plural(songs, "song"),
                path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())
            ));
        }
        Err(e) => app.toast(format!("Couldn't write {}: {e}", path.display())),
    }
}

/// Import…: a Redline export is merged in; a list of links is looked up and saved, one
/// toast at the end.
fn import_saved(app: &mut App) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Saved export or list of links", &["json", "txt"])
        .add_filter("All files", &["*"])
        .pick_file()
    else {
        return;
    };
    let parsed = std::fs::read_to_string(&path)
        .map_err(|e| format!("Couldn't read {}: {e}", path.display()))
        .and_then(|text| saved::parse_import(&text).map_err(|e| format!("{e:#}")));
    match parsed {
        Err(e) => app.toast(e),
        Ok(Import::Saved(export)) => {
            let report = app.saved.merge(export);
            let mut queued = 0;
            for id in &report.new_offline {
                if let Some(c) = app.saved.get(*id) {
                    let planned = saved::offline_jobs(c, &app.history, &app.settings);
                    queued += queue_downloads(app, planned);
                }
            }
            let more = if queued > 0 { format!("; downloading {} for offline", plural(queued, "song")) } else { String::new() };
            app.toast(format!("{}{more}", report.message()));
        }
        Ok(Import::Links(links)) => {
            if app.tools.ytdlp.is_none() {
                app.toast("Saving links needs yt-dlp. Set it up in Settings.");
                return;
            }
            let total = links.len();
            let new = app.saved.unsaved_links(links);
            if new.is_empty() {
                app.toast(format!("All {} in the file are saved already", plural(total, "link")));
                return;
            }
            app.toast(format!("Saving {}…", plural(new.len(), "link")));
            let tally = app.saved_page.import.get_or_insert_with(ImportTally::default);
            tally.pending += new.len();
            tally.skipped += total - new.len();
            let idle = tally.waiting.is_empty() && !app.saved_page.lookups.iter().any(|l| l.import);
            app.saved_page.import.as_mut().expect("just set").waiting.extend(new);
            if idle {
                next_import_link(app);
            }
        }
    }
}

fn next_import_link(app: &mut App) {
    if let Some(url) = app.saved_page.import.as_mut().and_then(|t| t.waiting.pop_front()) {
        start_lookup(app, &url, true, true);
    }
}

/// Counts one imported link as done, and says how the import went after the last.
fn finish_import_link(app: &mut App, ok: bool) {
    let Some(t) = app.saved_page.import.as_mut() else { return };
    t.pending = t.pending.saturating_sub(1);
    if ok {
        t.saved += 1;
    } else {
        t.failed += 1;
    }
    if t.pending > 0 {
        return next_import_link(app);
    }
    let t = app.saved_page.import.take().unwrap_or_default();
    let mut msg = match (t.saved, t.failed) {
        (0, n) => format!("Couldn't look up {}", plural(n, "link")),
        (s, 0) => format!("Imported {}", plural(s, "link")),
        (s, n) => format!("Imported {}; {n} couldn't be looked up", plural(s, "link")),
    };
    if t.skipped > 0 {
        msg += &format!(" ({} saved already)", t.skipped);
    }
    app.toast(msg);
}

fn finish_sync(app: &mut App, id: CollectionId, result: Result<Vec<Track>, String>) {
    let Some(c) = app.saved.get(id) else { return };
    match result.and_then(|fresh| saved::sync_plan(c, &fresh).map_err(|e| format!("{e:#}"))) {
        Ok(plan) => {
            let added = plan.added.clone();
            app.saved.apply_sync(id, plan);
            keep_new_offline(app, id, &added);
        }
        // A failed refresh is noted on the collection; the next one in line still runs.
        Err(e) => app.saved.set_sync_error(id, e),
    }
}

pub fn show(ui: &mut Ui, app: &mut App, platform: Option<Platform>) {
    let Some(p) = platform else {
        return overview(ui, app);
    };
    if let Some(id) = app.saved_page.open
        && app.saved.get(id).is_some_and(|c| c.platform() == p)
    {
        return collection(ui, app, id);
    }
    app.saved_page.open = None;
    if !app.saved.platforms().iter().any(|(q, _)| *q == p) {
        // The last item went; back to the overview.
        app.page = Page::Saved(None);
        return overview(ui, app);
    }
    platform_page(ui, app, p);
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// "Paste a link" box with a Save button, and what's being looked up.
fn save_box(ui: &mut Ui, app: &mut App) {
    let mut go = false;
    ui.horizontal(|ui| {
        Frame::new()
            .fill(theme::INPUT)
            .corner_radius(255)
            .inner_margin(Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.set_width(440.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::LINK).size(16.0).color(theme::MUTED));
                    let r = ui.add(
                        TextEdit::singleline(&mut app.saved_page.link)
                            .frame(Frame::NONE)
                            .hint_text("Paste a playlist, set, album or song link")
                            .desired_width(f32::INFINITY),
                    );
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        go = true;
                    }
                });
            });
        let can = app.tools.ytdlp.is_some() && !app.saved_page.link.trim().is_empty();
        if theme::small_pill_button(ui, &format!("{}  Save", icons::BOOKMARK_SIMPLE), can).clicked() {
            go = true;
        }
    });
    if go {
        let url = std::mem::take(&mut app.saved_page.link);
        start_save(app, &url);
    }
    let page = &mut app.saved_page;
    for l in page.lookups.iter().filter(|l| !l.import) {
        ui.horizontal(|ui| {
            ui.add(Spinner::new().size(14.0).color(theme::accent().base));
            ui.add(egui::Label::new(theme::muted(format!("Looking up {}…", l.url))).truncate());
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

fn overview(ui: &mut Ui, app: &mut App) {
    let platforms = app.saved.platforms();
    let songs: usize = platforms.iter().map(|(p, _)| app.saved.all_songs(*p).len()).sum();
    // Local sits on this page even with nothing saved online. A PC always has somewhere to
    // look (the Music folder until others are picked), so it counts as a source here.
    let local_ready = !app.settings.local_folders.is_empty() || !app.settings.local_files.is_empty();
    let subtitle = if platforms.is_empty() {
        "Save playlists and songs from YouTube, SoundCloud, Bandcamp and more.".to_owned()
    } else {
        format!(
            "{} · {} · {}{}",
            plural(app.saved.collections.len(), "collection"),
            plural(songs, "song"),
            plural(platforms.len(), "platform"),
            if local_ready { " + Device" } else { "" },
        )
    };
    theme::page_header(ui, "Saved", "Saved music", &subtitle);
    save_box(ui, app);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let importing = app.saved_page.import.as_ref().map(|t| t.pending);
        if theme::ghost_button(ui, &format!("{}  Export…", icons::EXPORT), true)
            .on_hover_text("Save your collections and songs to a file")
            .clicked()
        {
            export_saved(app);
        }
        if theme::ghost_button(ui, &format!("{}  Import…", icons::DOWNLOAD_SIMPLE), importing.is_none())
            .on_hover_text("Add from a Redline export, or a text file with one link per line")
            .clicked()
        {
            import_saved(app);
        }
        if let Some(n) = importing {
            ui.add(Spinner::new().size(14.0).color(theme::accent().base));
            ui.label(theme::muted(format!("Importing, {} to go…", plural(n, "link"))));
        }
    });
    ui.add_space(24.0);

    ui.label(theme::heading("Platforms"));
    ui.add_space(8.0);
    let now = unix_now();
    let mut pick = None;
    let mut open_local = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
        // First, beside the online platforms: the songs on this PC.
        if local_card(ui, app.local_page.tracks.len(), app.local_page.scanning()).clicked() {
            open_local = true;
        }
        for (p, _) in &platforms {
            let (collections, _) = app.saved.for_platform(*p);
            let synced = collections.iter().filter_map(|c| c.synced_at).max();
            let line = format!(
                "{} · {}",
                plural(collections.len(), "collection"),
                plural(app.saved.all_songs(*p).len(), "song")
            );
            let when = synced.map(|t| format!("Synced {}", ago(t, now)));
            if platform_card(ui, *p, &line, when.as_deref()).clicked() {
                pick = Some(*p);
            }
        }
    });
    if open_local {
        app.page = Page::Local;
        app.saved_page.open = None;
    } else if let Some(p) = pick {
        app.page = Page::Saved(Some(p));
        app.saved_page.open = None;
    }
    if platforms.is_empty() {
        ui.add_space(12.0);
        ui.label(theme::muted("Save a link above to add YouTube, SoundCloud and more."));
    }
}

/// The PC as a platform card: the same size and layout as the online ones, in grey.
fn local_card(ui: &mut Ui, songs: usize, scanning: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(240.0, 150.0), Sense::click());
    let accent = theme::MUTED;
    let mix = if resp.hovered() { 0.24 } else { 0.14 };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), theme::lerp_color(theme::CARD, accent, mix));
    let left = rect.left() + 18.0;
    painter.text(
        egui::pos2(left, rect.top() + 34.0),
        egui::Align2::LEFT_CENTER,
        icons::HARD_DRIVES,
        egui::FontId::proportional(34.0),
        accent,
    );
    painter.text(
        egui::pos2(left, rect.top() + 80.0),
        egui::Align2::LEFT_CENTER,
        "Local",
        egui::FontId::new(20.0, theme::bold()),
        theme::TEXT,
    );
    painter.text(
        egui::pos2(left, rect.top() + 106.0),
        egui::Align2::LEFT_CENTER,
        format!("{} · On this PC", plural(songs, "song")),
        egui::FontId::proportional(13.0),
        theme::MUTED,
    );
    let status = if scanning { "Looking for songs…" } else { "Music folder and added files" };
    painter.text(
        egui::pos2(left, rect.top() + 126.0),
        egui::Align2::LEFT_CENTER,
        status,
        egui::FontId::proportional(12.0),
        theme::SUBTLE,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A card tinted with the platform's color: glyph, name, counts and last sync. Painted
/// with the platform's accent directly: a `with_accent` scope would stop the row wrapping.
fn platform_card(ui: &mut Ui, p: Platform, line: &str, synced: Option<&str>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(240.0, 150.0), Sense::click());
    let accent = theme::accent_for(p);
    let mix = if resp.hovered() { 0.24 } else { 0.14 };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), theme::lerp_color(theme::CARD, accent.base, mix));
    let left = rect.left() + 18.0;
    painter.text(
        egui::pos2(left, rect.top() + 34.0),
        egui::Align2::LEFT_CENTER,
        theme::platform_glyph(p),
        egui::FontId::proportional(34.0),
        accent.base,
    );
    painter.text(
        egui::pos2(left, rect.top() + 80.0),
        egui::Align2::LEFT_CENTER,
        p.label(),
        egui::FontId::new(20.0, theme::bold()),
        theme::TEXT,
    );
    painter.text(
        egui::pos2(left, rect.top() + 106.0),
        egui::Align2::LEFT_CENTER,
        line,
        egui::FontId::proportional(13.0),
        theme::MUTED,
    );
    if let Some(s) = synced {
        painter.text(
            egui::pos2(left, rect.top() + 126.0),
            egui::Align2::LEFT_CENTER,
            s,
            egui::FontId::proportional(12.0),
            theme::SUBTLE,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn platform_page(ui: &mut Ui, app: &mut App, p: Platform) {
    let (collections, songs) = app.saved.for_platform(p);
    let ids: Vec<CollectionId> = collections.iter().map(|c| c.id).collect();
    let n_songs = songs.len();
    let n_all = app.saved.all_songs(p).len();

    // Header with the platform's glyph in its color.
    ui.add_space(12.0);
    ui.label(theme::eyebrow("Saved"));
    ui.horizontal(|ui| {
        ui.label(RichText::new(theme::platform_glyph(p)).size(52.0).color(theme::accent().base));
        ui.label(RichText::new(p.label()).family(theme::bold()).size(56.0).color(theme::TEXT));
    });
    ui.label(
        theme::muted(format!(
            "{} · {} saved on their own · {} in all",
            plural(ids.len(), "collection"),
            plural(n_songs, "song"),
            plural(n_all, "song"),
        ))
        .size(15.0),
    );
    ui.add_space(20.0);
    save_box(ui, app);
    ui.add_space(16.0);

    let syncing = ids.iter().filter(|id| app.saved_page.is_syncing(**id)).count();
    ui.horizontal(|ui| {
        let options = [
            (Tab::Collections, format!("Collections · {}", ids.len())),
            (Tab::Songs, format!("Songs · {n_songs}")),
            (Tab::All, "All songs".to_owned()),
        ];
        let opts: Vec<(Tab, &str)> = options.iter().map(|(t, l)| (*t, l.as_str())).collect();
        theme::chips(ui, &mut app.saved_page.tab, &opts);
        ui.add_space(8.0);
        let can = !ids.is_empty() && syncing < ids.len() && app.tools.ytdlp.is_some();
        if theme::ghost_button(ui, &format!("{}  Refresh all", icons::ARROWS_CLOCKWISE), can).clicked() {
            for id in &ids {
                start_sync(app, *id);
            }
        }
        if syncing > 0 {
            ui.add(Spinner::new().size(14.0).color(theme::accent().base));
            ui.label(theme::muted(format!("Refreshing {}…", plural(syncing, "collection"))));
        }
    });
    ui.add_space(12.0);
    let hint = match app.saved_page.tab {
        Tab::Collections => "Find a collection",
        _ => "Find a song",
    };
    search_box(ui, &mut app.saved_page.search, hint);
    ui.add_space(12.0);

    match app.saved_page.tab {
        Tab::Collections => collection_grid(ui, app, &ids),
        Tab::Songs => songs_table(ui, app, p),
        Tab::All => all_songs(ui, app, p),
    }
}

fn collection_grid(ui: &mut Ui, app: &mut App, ids: &[CollectionId]) {
    if ids.is_empty() {
        ui.label(theme::muted("No playlists, sets or albums saved from here yet. Paste one above."));
        return;
    }
    let query = app.saved_page.search.trim().to_lowercase();
    let now = unix_now();
    let mut open = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
        for id in ids {
            let Some(c) = app.saved.get(*id) else { continue };
            if !query.is_empty() && !c.name.to_lowercase().contains(&query) {
                continue;
            }
            let (rect, resp) = ui.allocate_exact_size(vec2(196.0, 262.0), Sense::click());
            let fill = if resp.hovered() { theme::CARD_HOVER } else { theme::CARD };
            ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
            let mut card = ui.new_child(
                UiBuilder::new()
                    .max_rect(rect.shrink(12.0))
                    .layout(Layout::top_down(Align::Min)),
            );
            card.spacing_mut().item_spacing.y = 4.0;
            let thumb = c.tracks.iter().find_map(|t| t.track.thumbnail.as_deref());
            theme::cover(&mut card, 172.0, thumb, icons::PLAYLIST);
            card.add_space(6.0);
            card.add(egui::Label::new(theme::strong(&c.name)).truncate());
            let status = if app.saved_page.is_syncing(c.id) {
                format!("{} Refreshing…", icons::ARROWS_CLOCKWISE)
            } else if c.sync_error.is_some() {
                format!("{} Couldn't refresh", icons::WARNING_CIRCLE)
            } else {
                c.synced_at.map_or_else(String::new, |t| format!("Synced {}", ago(t, now)))
            };
            card.label(theme::muted(plural(c.tracks.len() - c.removed_count(), "song")).small());
            card.label(RichText::new(status).small().color(theme::SUBTLE));
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                open = Some(c.id);
            }
        }
    });
    if open.is_some() {
        app.saved_page.open = open;
        app.saved_page.search.clear();
    }
}

/// The source to play for a row and whether it's a downloaded copy.
fn row_state<'a>(app: &App, t: &Track, tag: Option<&'a str>) -> RowState<'a> {
    let local = library::playable_source(t, &app.history);
    let current = app.player_state.source.as_ref();
    RowState {
        playing: current.is_some_and(|c| *c == t.source || *c == local),
        offline: t.is_link() && matches!(local, Source::Local(_)),
        tag,
        platform_link: false,
        link: LinkStatus::Unknown,
        replace_menu: false,
    }
}

/// Puts one saved song in a playlist or the queue, from the row's menu.
fn add_song(app: &mut App, target: AddTarget, t: &Track) {
    add_to(app, target, vec![t.clone()]);
    app.toast(format!("Added “{}”", t.title));
}

/// Opens Convert with a song's downloaded file.
fn convert_song(app: &mut App, t: &Track) {
    if let Some(file) = library::local_file(t, &app.history) {
        app.convert_file(file);
    }
}

fn queue_offline(app: &mut App, t: &Track) {
    if let Some(job) = library::offline_download(t, &app.settings) {
        app.jobs.push(job.title, job.subtitle, job.spec);
    }
}

/// Plays `list` from `index`, leaving out tracks gone from the platform. A removed track
/// that's clicked plays on its own (it may still have a downloaded copy).
fn play_saved(app: &mut App, list: &[SavedTrack], index: usize, shuffle: bool) {
    if list.get(index).is_some_and(|t| t.removed_upstream) {
        return app.play_tracks(vec![list[index].track.clone()], 0, None);
    }
    let start = list[..index.min(list.len())].iter().filter(|t| !t.removed_upstream).count();
    let tracks: Vec<Track> = list.iter().filter(|t| !t.removed_upstream).map(|t| t.track.clone()).collect();
    app.play_list(tracks, start, shuffle);
}

fn songs_table(ui: &mut Ui, app: &mut App, p: Platform) {
    let tracks: Vec<Track> = app.saved.for_platform(p).1.into_iter().cloned().collect();
    if tracks.is_empty() {
        ui.label(theme::muted("Songs you save on their own show up here."));
        return;
    }
    let query = app.saved_page.search.trim().to_lowercase();
    let list = List::Songs(p);
    let refs: Vec<&Track> = tracks.iter().collect();
    let links = replace::list_statuses(ui, app, list, &refs);
    replace::header(ui, app, list, &refs, &links);
    let only_unavailable = app.replace.filtering(list);
    list_header(ui);
    let names = playlist_names(&app.library);
    let mut action = None;
    let mut add = None;
    for (idx, t) in tracks.iter().enumerate() {
        let link = links[idx];
        if (!query.is_empty() && !t.matches(&query)) || (only_unavailable && link.label().is_none()) {
            continue;
        }
        let state = RowState { link, replace_menu: true, ..row_state(app, t, None) };
        let offline = state.offline;
        let row = track_row(ui, idx, t, state, |ui| {
            let mut a = None;
            if theme::icon_button(ui, icons::X, "Remove from Saved").clicked() {
                a = Some(RowAction::Remove(idx));
            }
            if link.label().is_some() {
                a = replace_button(ui, idx, link).or(a);
            } else if !offline && theme::icon_button(ui, icons::DOWNLOAD_SIMPLE, "Download for offline").clicked() {
                a = Some(RowAction::Offline(idx));
            }
            if let Some(target) = add_menu(ui, &names) {
                add = Some((target, idx));
            }
            convert_button(ui, idx, offline).or(a)
        });
        action = action.or(row);
    }
    if let Some((target, i)) = add {
        add_song(app, target, &tracks[i]);
    }
    match action {
        Some(RowAction::Convert(i)) => convert_song(app, &tracks[i]),
        Some(RowAction::Play(i)) => app.play_tracks(tracks, i, None),
        Some(RowAction::Remove(i)) => {
            app.saved.remove_song(&tracks[i].key());
        }
        Some(RowAction::Offline(i)) => queue_offline(app, &tracks[i]),
        Some(RowAction::FindReplacement(i)) => replace::find(app, list.target(i, &tracks[i]), tracks[i].clone()),
        _ => {}
    }
}

fn all_songs(ui: &mut Ui, app: &mut App, p: Platform) {
    let all = app.saved.all_songs(p);
    let playable = all.iter().filter(|t| !t.removed_upstream).count();
    ui.horizontal(|ui| {
        if theme::small_pill_button(ui, &format!("{}  Play all", icons::PLAY), playable > 0).clicked() {
            play_saved(app, &all, 0, false);
        }
        if theme::ghost_button(ui, &format!("{}  Shuffle", icons::SHUFFLE), playable > 0).clicked() {
            play_saved(app, &all, 0, true);
        }
        ui.label(theme::muted(plural(all.len(), "song")));
    });
    ui.add_space(8.0);
    saved_table(ui, app, &all, p, None);
}

/// Rows of saved tracks, with the ones gone from the platform tagged. With `checked`, the
/// links are checked and the list gets the unavailable-songs header; otherwise rows show
/// what's already known.
fn saved_table(ui: &mut Ui, app: &mut App, list: &[SavedTrack], p: Platform, checked: Option<List>) {
    if list.is_empty() {
        ui.label(theme::muted("Nothing here yet."));
        return;
    }
    let query = app.saved_page.search.trim().to_lowercase();
    let tag = format!("Removed from {}", p.label());
    let refs: Vec<&Track> = list.iter().map(|t| &t.track).collect();
    let links: Vec<LinkStatus> = match checked {
        Some(l) => {
            let links = replace::list_statuses(ui, app, l, &refs);
            replace::header(ui, app, l, &refs, &links);
            links
        }
        None => linkcheck::shared().statuses(refs.iter().copied()),
    };
    let only_unavailable = checked.is_some_and(|l| app.replace.filtering(l));
    list_header(ui);
    let names = playlist_names(&app.library);
    let mut action = None;
    let mut add = None;
    for (idx, st) in list.iter().enumerate() {
        let t = &st.track;
        let link = links[idx];
        if (!query.is_empty() && !t.matches(&query)) || (only_unavailable && link.label().is_none()) {
            continue;
        }
        let state = RowState { link, replace_menu: true, ..row_state(app, t, st.removed_upstream.then_some(tag.as_str())) };
        let offline = state.offline;
        let has_file = offline || !t.is_link();
        let row = track_row(ui, idx, t, state, |ui| {
            let a = if link.label().is_some() {
                replace_button(ui, idx, link)
            } else {
                (t.is_link() && !offline && theme::icon_button(ui, icons::DOWNLOAD_SIMPLE, "Download for offline").clicked())
                    .then_some(RowAction::Offline(idx))
            };
            if let Some(target) = add_menu(ui, &names) {
                add = Some((target, idx));
            }
            convert_button(ui, idx, has_file).or(a)
        });
        action = action.or(row);
    }
    if let Some((target, i)) = add {
        add_song(app, target, &list[i].track);
    }
    match action {
        Some(RowAction::Convert(i)) => convert_song(app, &list[i].track),
        Some(RowAction::Play(i)) => play_saved(app, list, i, false),
        Some(RowAction::Offline(i)) => queue_offline(app, &list[i].track),
        Some(RowAction::FindReplacement(i)) => {
            let t = &list[i].track;
            replace::find(app, Target::Saved { key: t.key() }, t.clone());
        }
        _ => {}
    }
}

fn collection(ui: &mut Ui, app: &mut App, id: CollectionId) {
    let Some(c) = app.saved.get(id) else { return };
    let p = c.platform();
    let (name, url, keep_offline, error) = (c.name.clone(), c.source_url.clone(), c.keep_offline, c.sync_error.clone());
    let tracks = c.tracks.clone();
    let removed = c.removed_count();
    let playable = tracks.len() - removed;
    let secs: f64 = tracks.iter().filter(|t| !t.removed_upstream).filter_map(|t| t.track.duration).sum();
    let synced = c.synced_at;
    let thumb = tracks.iter().find_map(|t| t.track.thumbnail.clone());
    let syncing = app.saved_page.is_syncing(id);

    ui.add_space(4.0);
    if ui
        .link(RichText::new(format!("{}  {}", icons::ARROW_LEFT, p.label())).color(theme::MUTED))
        .clicked()
    {
        app.saved_page.open = None;
        return;
    }
    ui.add_space(8.0);

    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            match &thumb {
                Some(t) => {
                    theme::cover(ui, 96.0, Some(t), icons::PLAYLIST);
                }
                None => {
                    theme::art_tile(ui, 96.0, icons::PLAYLIST, theme::accent().base);
                }
            }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(theme::eyebrow(&format!("{} {} collection", theme::platform_glyph(p), p.label())));
                let page = &mut app.saved_page;
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
                                app.saved.rename(id, &text);
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
                let mut meta = plural(playable, "song");
                if secs > 0.0 {
                    meta.push_str(&format!(" · {}", fmt_duration(secs)));
                }
                if let Some(t) = synced {
                    meta.push_str(&format!(" · synced {}", ago(t, unix_now())));
                }
                ui.label(theme::muted(meta));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::small_pill_button(ui, &format!("{}  Play", icons::PLAY), playable > 0).clicked() {
                        play_saved(app, &tracks, 0, false);
                    }
                    if theme::ghost_button(ui, &format!("{}  Shuffle", icons::SHUFFLE), playable > 0).clicked() {
                        play_saved(app, &tracks, 0, true);
                    }
                    if syncing {
                        ui.add(Spinner::new().size(16.0).color(theme::accent().base));
                        ui.label(theme::muted("Refreshing…"));
                    } else if theme::ghost_button(ui, &format!("{}  Refresh", icons::ARROWS_CLOCKWISE), app.tools.ytdlp.is_some())
                        .clicked()
                    {
                        start_sync(app, id);
                    }
                    ui.add_space(4.0);
                    let mut on = keep_offline;
                    if theme::toggle(ui, &mut on, "Keep offline")
                        .on_hover_text("Download every song, and new ones after each refresh")
                        .changed()
                    {
                        set_keep_offline(app, id, on);
                    }
                });
                ui.horizontal(|ui| {
                    copy_menu(ui, app, &name, &tracks);
                    if theme::icon_button(ui, icons::ARROW_SQUARE_OUT, &format!("Open on {}", p.label())).clicked() {
                        let _ = open::that_detached(&url);
                    }
                    if theme::icon_button(ui, icons::PENCIL_SIMPLE, "Rename").clicked() {
                        app.saved_page.renaming = Some((id, name.clone()));
                    }
                    let confirming = app.saved_page.confirm_remove == Some(id);
                    let tip = if confirming { "Click again to remove" } else { "Remove from Saved" };
                    let icon = if confirming { icons::WARNING } else { icons::TRASH };
                    if theme::icon_button(ui, icon, tip).clicked() {
                        if confirming {
                            app.saved.remove(id);
                            app.saved_page.confirm_remove = None;
                            app.saved_page.open = None;
                        } else {
                            app.saved_page.confirm_remove = Some(id);
                        }
                    }
                    if confirming {
                        ui.label(RichText::new("Click again to remove").small().color(theme::accent().hover));
                    }
                });
            });
        });
    });
    if app.saved.get(id).is_none() {
        return;
    }
    if let Some(e) = error {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::WARNING_CIRCLE).color(theme::accent().base));
            ui.add(egui::Label::new(RichText::new(format!("Couldn't refresh: {e}")).color(theme::TEXT)).wrap());
        });
    }
    if removed > 0 {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(theme::muted(format!("{} no longer on {}.", plural(removed, "song"), p.label())));
            if ui
                .link(RichText::new("Clear them").color(theme::accent().hover))
                .on_hover_text("Forget the songs the platform removed")
                .clicked()
            {
                app.saved.clear_removed(id);
            }
        });
    }
    ui.add_space(12.0);
    if tracks.len() > 8 {
        search_box(ui, &mut app.saved_page.search, "Find in collection");
        ui.add_space(8.0);
    }
    saved_table(ui, app, &tracks, p, Some(List::Collection(id)));
}

/// Turning it on downloads everything that isn't downloaded yet, straight away.
pub fn set_keep_offline(app: &mut App, id: CollectionId, on: bool) {
    app.saved.set_keep_offline(id, on);
    let Some(c) = app.saved.get(id).filter(|_| on) else { return };
    let planned = saved::offline_jobs(c, &app.history, &app.settings);
    match queue_downloads(app, planned) {
        0 => app.toast("Everything here is downloaded or on its way"),
        n => app.toast(format!("Downloading {} for offline", plural(n, "song"))),
    }
}

/// Saves a link track as a single song, or says where it already is.
pub fn save_track(app: &mut App, track: &Track) {
    if !track.is_link() {
        return;
    }
    let msg = if app.saved.add_song(track.clone()) {
        format!("Saved {}", track.title)
    } else {
        "Already saved".to_owned()
    };
    app.toast(msg);
}

/// The player bar's bookmark: saves the track, or takes it out of the single songs. A
/// track that's only saved as part of a collection stays (the collection follows the
/// platform).
pub fn toggle_saved(app: &mut App, track: &Track) {
    let key = track.key();
    if app.saved.remove_song(&key) {
        app.toast(format!("Removed {} from Saved", track.title));
    } else if let Some(c) = app.saved.collection_with(&key) {
        let msg = format!("Saved in {}", c.name);
        app.toast(msg);
    } else {
        save_track(app, track);
    }
}

/// "Copy to playlist…" button: puts the collection's songs (not the removed ones) into one
/// of your playlists, or a new one named after it.
fn copy_menu(ui: &mut Ui, app: &mut App, name: &str, tracks: &[SavedTrack]) {
    let resp = theme::ghost_button(ui, &format!("{}  Copy to playlist…", icons::COPY), true);
    let playlists = super::playlists::playlist_names(&app.library);
    let mut pick = None;
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(200.0);
        for (pid, pname) in &playlists {
            if ui.button(format!("{}  {pname}", icons::PLAYLIST)).clicked() {
                pick = Some(Some(*pid));
            }
        }
        if ui.button(format!("{}  New playlist", icons::PLUS)).clicked() {
            pick = Some(None);
        }
    });
    let Some(target) = pick else { return };
    let songs: Vec<Track> = tracks.iter().filter(|t| !t.removed_upstream).map(|t| t.track.clone()).collect();
    let n = songs.len();
    let id = target.unwrap_or_else(|| app.library.create(name));
    app.library.add(id, songs);
    let pname = app.library.get(id).map(|p| p.name.clone()).unwrap_or_default();
    app.toast(format!("Copied {} to {pname}", plural(n, "song")));
}
