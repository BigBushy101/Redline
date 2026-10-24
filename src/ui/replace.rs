//! Find replacement and Fix unavailable: another upload of a song whose link died, picked by
//! hand or swapped in when it's a clear match. Shared by Playlists and Saved.

use std::collections::HashSet;
use std::time::Duration;

use eframe::egui::{self, Align2, RichText, ScrollArea, Spinner, Ui, Vec2};
use egui_phosphor::regular as icons;
use redline_core::library::{PlaylistId, Track};
use redline_core::linkcheck::{self, Candidate, FixItem, LinkStatus, Ranked};
use redline_core::platform::Platform;
use redline_core::saved::CollectionId;

use crate::app::App;
use crate::format::fmt_duration;
use crate::task::Task;
use crate::theme;

/// Where a replacement goes.
#[derive(Clone, Debug)]
pub enum Target {
    /// One row of a playlist.
    Playlist { id: PlaylistId, index: usize },
    /// Every copy in Saved, by [`Track::key`].
    Saved { key: String },
}

/// A list whose links are checked and fixed together.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum List {
    Playlist(PlaylistId),
    Collection(CollectionId),
    /// A platform's songs saved on their own.
    Songs(Platform),
}

impl List {
    pub fn target(self, index: usize, track: &Track) -> Target {
        match self {
            List::Playlist(id) => Target::Playlist { id, index },
            List::Collection(_) | List::Songs(_) => Target::Saved { key: track.key() },
        }
    }
}

/// Find replacement for one song.
struct Search {
    target: Target,
    track: Track,
    task: Task<Result<Vec<Ranked>, String>>,
    found: Option<Result<Vec<Ranked>, String>>,
}

/// Fix unavailable, searching in the background.
struct Fixing {
    list: List,
    /// Started by the auto-replace setting: clear matches only, no review.
    auto: bool,
    task: Task<Vec<FixItem>>,
}

/// The songs Fix unavailable couldn't settle by itself.
struct Review {
    list: List,
    items: Vec<FixItem>,
}

#[derive(Default)]
pub struct ReplaceDialog {
    search: Option<Search>,
    fixing: Vec<Fixing>,
    review: Option<Review>,
    /// The list links were last asked about, and how many tracks it had then.
    checked: Option<(List, usize)>,
    /// Lists auto-replace has run on this session.
    auto_done: HashSet<List>,
    /// The list showing only its unavailable songs.
    filter: Option<List>,
}

/// How many picks a window offers per song.
const PICKS: usize = 5;

impl ReplaceDialog {
    pub fn is_fixing(&self, list: List) -> bool {
        self.fixing.iter().any(|f| f.list == list)
    }

    /// Whether `list` shows only its unavailable songs.
    pub fn filtering(&self, list: List) -> bool {
        self.filter == Some(list)
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// The link statuses of a list on screen. Asks for checks when the list comes on screen or
/// its length changes and, with auto-replace on, fixes its dead songs once the checks settle.
pub fn list_statuses(ui: &Ui, app: &mut App, list: List, tracks: &[&Track]) -> Vec<LinkStatus> {
    let lc = linkcheck::shared();
    let d = &mut app.replace;
    if d.checked != Some((list, tracks.len())) {
        d.checked = Some((list, tracks.len()));
        lc.request(tracks.iter().copied());
    }
    let links = lc.statuses(tracks.iter().copied());
    if app.settings.auto_replace_dead && !d.auto_done.contains(&list) && links.iter().any(|s| s.is_dead()) {
        if lc.busy() {
            // The last check finishing doesn't repaint, so look again shortly.
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        } else if !d.is_fixing(list) {
            d.auto_done.insert(list);
            start_fix(app, list, dead(tracks, &links), true);
        }
    }
    links
}

/// The dead tracks of a list, with where they are in it.
fn dead(tracks: &[&Track], links: &[LinkStatus]) -> Vec<(usize, Track)> {
    tracks
        .iter()
        .zip(links)
        .enumerate()
        .filter(|(_, (_, s))| s.is_dead())
        .map(|(i, (t, _))| (i, (*t).clone()))
        .collect()
}

/// "N unavailable" (shows only those songs) and "Fix unavailable", above a list's rows.
/// Draws nothing while every song plays.
pub fn header(ui: &mut Ui, app: &mut App, list: List, tracks: &[&Track], links: &[LinkStatus]) {
    let unavailable = links.iter().filter(|s| s.label().is_some()).count();
    if unavailable == 0 {
        if app.replace.filter == Some(list) {
            app.replace.filter = None;
        }
        return;
    }
    let n_dead = links.iter().filter(|s| s.is_dead()).count();
    ui.horizontal(|ui| {
        let mut only = app.replace.filtering(list);
        let label = format!("{}  {unavailable} unavailable", icons::LINK_BREAK);
        if theme::chips(ui, &mut only, &[(false, "All songs"), (true, label.as_str())]) {
            app.replace.filter = only.then_some(list);
        }
        ui.add_space(8.0);
        if app.replace.is_fixing(list) {
            ui.add(Spinner::new().size(14.0).color(theme::accent().base));
            ui.label(theme::muted("Finding replacements…"));
        } else if n_dead > 0 {
            let can = app.tools.ytdlp.is_some();
            if theme::ghost_button(ui, &format!("{}  Fix unavailable", icons::MAGIC_WAND), can)
                .on_hover_text("Swap in another upload of each song that's gone, asking about unclear ones")
                .clicked()
            {
                start_fix(app, list, dead(tracks, links), false);
            }
        }
    });
    ui.add_space(8.0);
}

fn start_fix(app: &mut App, list: List, tracks: Vec<(usize, Track)>, auto: bool) {
    let (tools, net) = (app.tools.clone(), app.settings.network.clone());
    let task = Task::spawn(&app.wake, move || linkcheck::fix_unavailable(&tools, &net, tracks));
    app.replace.fixing.push(Fixing { list, auto, task });
}

/// Opens Find replacement for `track`, searching straight away.
pub fn find(app: &mut App, target: Target, track: Track) {
    let (tools, net) = (app.tools.clone(), app.settings.network.clone());
    let t = track.clone();
    let task = Task::spawn(&app.wake, move || linkcheck::search(&tools, &net, &t).map_err(|e| format!("{e:#}")));
    app.replace.search = Some(Search { target, track, task, found: None });
}

/// Puts `c` where `track` was. Returns false if the song isn't there any more.
fn apply(app: &mut App, target: &Target, track: &Track, c: &Candidate) -> bool {
    let done = match target {
        Target::Playlist { id, index } => {
            // The playlist may have changed while searching; find the song again by its link.
            let at = app.library.get(*id).and_then(|p| match p.tracks.get(*index) {
                Some(t) if t.source == track.source => Some(*index),
                _ => p.tracks.iter().position(|t| t.source == track.source),
            });
            at.is_some_and(|i| app.library.replace_link(*id, i, c))
        }
        Target::Saved { key } => app.saved.replace_link(key, c),
    };
    if done {
        linkcheck::shared().record(&c.url, LinkStatus::Alive);
    }
    done
}

/// Applies a pick from a window and says so.
fn use_pick(app: &mut App, target: &Target, track: &Track, c: &Candidate) {
    if apply(app, target, track, c) {
        app.toast(format!("Replaced with “{}”", c.title));
    } else {
        app.toast(format!("“{}” isn't there any more", track.title));
    }
}

/// Takes in finished searches and fixes. Called every frame, whichever page is showing.
pub fn poll(app: &mut App) {
    if let Some(s) = app.replace.search.as_mut()
        && s.found.is_none()
    {
        s.found = s.task.take();
    }
    let mut i = 0;
    while i < app.replace.fixing.len() {
        let Some(items) = app.replace.fixing[i].task.take() else {
            i += 1;
            continue;
        };
        let Fixing { list, auto, .. } = app.replace.fixing.remove(i);
        let mut fixed = 0;
        let mut left = Vec::new();
        for item in items {
            match item.clear_match().cloned() {
                Some(c) if apply(app, &list.target(item.index, &item.track), &item.track, &c) => fixed += 1,
                _ => left.push(item),
            }
        }
        if fixed > 0 {
            app.toast(format!("Fixed {}", plural(fixed, "song")));
        }
        if !auto {
            if left.is_empty() {
                if fixed == 0 {
                    app.toast("Nothing to fix");
                }
            } else {
                app.replace.review = Some(Review { list, items: left });
            }
        }
    }
}

/// The open windows, if any.
pub fn show(ctx: &egui::Context, app: &mut App) {
    search_window(ctx, app);
    review_window(ctx, app);
}

fn search_window(ctx: &egui::Context, app: &mut App) {
    let Some(s) = &app.replace.search else { return };
    let mut open = true;
    let mut pick = None;
    egui::Window::new("Find replacement")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(theme::muted(format!("Another upload of “{}”", s.track.title)));
            ui.add_space(8.0);
            match &s.found {
                None => {
                    ui.horizontal(|ui| {
                        ui.add(Spinner::new().size(16.0).color(theme::accent().base));
                        ui.label(theme::muted("Searching YouTube…"));
                    });
                }
                Some(Err(e)) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::WARNING_CIRCLE).color(theme::accent().base));
                        ui.add(egui::Label::new(RichText::new(e).color(theme::TEXT)).wrap());
                    });
                }
                Some(Ok(ranked)) if ranked.is_empty() => {
                    ui.label(theme::muted("No matches found"));
                }
                Some(Ok(ranked)) => {
                    for (i, r) in ranked.iter().take(PICKS).enumerate() {
                        if pick_row(ui, r) {
                            pick = Some(r.candidate.clone());
                        }
                        if i + 1 < ranked.len().min(PICKS) {
                            ui.add_space(4.0);
                        }
                    }
                }
            }
        });
    if let Some(c) = pick {
        let s = app.replace.search.take().expect("search window is open");
        use_pick(app, &s.target, &s.track, &c);
    } else if !open {
        app.replace.search = None;
    }
}

fn review_window(ctx: &egui::Context, app: &mut App) {
    let Some(r) = &app.replace.review else { return };
    enum Act {
        Use(usize, Candidate),
        Skip(usize),
    }
    let mut open = true;
    let mut act = None;
    egui::Window::new("Review replacements")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(theme::muted(format!(
                "No clear match for {}. Pick an upload, or skip.",
                plural(r.items.len(), "song")
            )));
            ui.add_space(8.0);
            ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                for (i, item) in r.items.iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::LINK_BREAK).color(theme::MUTED));
                        ui.label(theme::strong(&item.track.title));
                        if let Some(a) = &item.track.artist {
                            ui.label(theme::muted(a));
                        }
                    });
                    if item.ranked.is_empty() {
                        ui.label(theme::muted("No matches found"));
                    }
                    for p in item.ranked.iter().take(PICKS) {
                        if pick_row(ui, p) {
                            act = Some(Act::Use(i, p.candidate.clone()));
                        }
                    }
                    if theme::ghost_button(ui, "Skip", true).clicked() {
                        act = Some(Act::Skip(i));
                    }
                    ui.add_space(4.0);
                }
            });
        });
    if !open {
        app.replace.review = None;
        return;
    }
    let Some(act) = act else { return };
    let mut review = app.replace.review.take().expect("review window is open");
    let i = match act {
        Act::Use(i, c) => {
            let item = &review.items[i];
            use_pick(app, &review.list.target(item.index, &item.track), &item.track, &c);
            i
        }
        Act::Skip(i) => i,
    };
    review.items.remove(i);
    if !review.items.is_empty() {
        app.replace.review = Some(review);
    }
}

/// One pick: art, title, channel, length and how far that is from the song's, and "Use this".
/// Returns whether it was picked.
fn pick_row(ui: &mut Ui, r: &Ranked) -> bool {
    let c = &r.candidate;
    let mut meta: Vec<String> = Vec::new();
    if let Some(u) = &c.uploader {
        meta.push(u.clone());
    }
    if let Some(d) = c.duration {
        meta.push(fmt_duration(d));
    }
    match r.delta_secs {
        Some(d) if d.abs() < 0.5 => meta.push("same length".to_owned()),
        Some(d) => meta.push(format!("{d:+.0} s")),
        None => {}
    }
    ui.horizontal(|ui| {
        theme::cover(ui, 40.0, c.thumbnail.as_deref(), icons::LINK);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.set_width(320.0);
            ui.add(egui::Label::new(theme::strong(&c.title)).truncate()).on_hover_text(&c.url);
            ui.add(egui::Label::new(RichText::new(meta.join(" · ")).small().color(theme::MUTED)).truncate());
        });
        theme::small_pill_button(ui, "Use this", true).clicked()
    })
    .inner
}
