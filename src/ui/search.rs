//! Global search / command palette: pages, Saved collections and songs, playlists and local
//! files, opened with Ctrl+K (or the sidebar's Search button).

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Frame, Key, Margin, Modifiers, RichText, ScrollArea,
    Sense, TextEdit, Ui, vec2,
};
use egui_phosphor::regular as icons;
use redline_core::library::{PlaylistId, Track};
use redline_core::platform::Platform;

use super::playlists::View;
use crate::app::{App, Page};
use crate::theme;

#[derive(Default)]
pub struct Search {
    pub is_open: bool,
    query: String,
    selected: usize,
    focus: bool,
}

impl Search {
    pub fn open(&mut self) {
        self.is_open = true;
        self.query.clear();
        self.selected = 0;
        self.focus = true;
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }

    pub fn toggle(&mut self) {
        if self.is_open {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, app: &mut App) {
        if !self.is_open {
            return;
        }
        // Keyboard navigation before the text field gets the events: arrows choose a result
        // rather than moving the caret, and Enter opens it.
        let found = hits(app, &self.query);
        self.selected = self.selected.min(found.len().saturating_sub(1));
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.close();
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            self.selected = (self.selected + 1).min(found.len().saturating_sub(1));
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            self.selected = self.selected.saturating_sub(1);
        }
        let enter = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        let mut run = enter.then(|| found.get(self.selected).map(|h| h.action.clone())).flatten();

        let screen = ctx.content_rect();
        let mut close = false;
        egui::Area::new(egui::Id::new("search-dim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(170));
                if ui.allocate_rect(screen, Sense::click()).clicked() {
                    close = true;
                }
            });

        let width = (screen.width() - 80.0).clamp(340.0, 680.0);
        let pos = egui::pos2(screen.center().x - width / 2.0, screen.top() + 64.0);
        let mut clicked = None;
        egui::Area::new(egui::Id::new("search-palette"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::ELEVATED)
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(icons::MAGNIFYING_GLASS)
                                    .size(20.0)
                                    .color(theme::accent().base),
                            );
                            ui.label(theme::heading("Search"));
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                ui.label(theme::muted("Ctrl+K to close"));
                            });
                        });
                        ui.add_space(8.0);
                        let response = Frame::new()
                            .fill(theme::INPUT)
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.add(
                                    TextEdit::singleline(&mut self.query)
                                        .frame(Frame::NONE)
                                        .hint_text("Search saved music, playlists, local files, pages…")
                                        .font(FontId::proportional(15.0))
                                        .desired_width(f32::INFINITY),
                                )
                            })
                            .inner;
                        if self.focus {
                            response.request_focus();
                            self.focus = false;
                        }
                        if response.changed() {
                            self.selected = 0;
                        }
                        ui.add_space(8.0);
                        ui.separator();
                        // The query may have changed this frame; show the fresh hits.
                        let shown = hits(app, &self.query);
                        self.selected = self.selected.min(shown.len().saturating_sub(1));
                        if shown.is_empty() {
                            ui.add_space(12.0);
                            ui.label(theme::muted(if self.query.trim().is_empty() {
                                "Start typing to search."
                            } else {
                                "No matches."
                            }));
                        } else {
                            ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                                for (i, hit) in shown.iter().enumerate() {
                                    if result_row(ui, hit, i == self.selected).clicked() {
                                        clicked = Some(hit.action.clone());
                                    }
                                }
                            });
                        }
                        ui.add_space(8.0);
                        ui.label(theme::muted("↑ ↓ to choose · Enter to open · Esc to close"));
                    });
            });

        if let Some(action) = run.take().or(clicked) {
            apply(app, action);
            self.close();
            return;
        }
        if close {
            self.close();
        }
    }
}

#[derive(Clone)]
enum Action {
    Page(Page),
    Collection(Platform, u64),
    Playlist(PlaylistId),
    Play(Vec<Track>, usize, Option<PlaylistId>),
}

struct Hit {
    title: String,
    meta: String,
    icon: &'static str,
    rank: u8,
    action: Action,
}

fn rank(title: &str, meta: &str, q: &str) -> Option<u8> {
    if q.is_empty() {
        return Some(1);
    }
    let title = title.to_lowercase();
    if title.starts_with(q) {
        Some(0)
    } else if title.contains(q) {
        Some(1)
    } else if meta.to_lowercase().contains(q) {
        Some(2)
    } else {
        None
    }
}

fn push(out: &mut Vec<Hit>, title: String, meta: String, icon: &'static str, rank: u8, action: Action) {
    out.push(Hit {
        title,
        meta,
        icon,
        rank,
        action,
    });
}

fn hits(app: &App, query: &str) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let mut out = Vec::new();

    let pages = [
        (Page::Download, "Download", icons::DOWNLOAD_SIMPLE),
        (Page::Batch, "Batch", icons::LIST_CHECKS),
        (Page::Convert, "Convert", icons::ARROWS_LEFT_RIGHT),
        (Page::Saved(None), "Saved", icons::BOOKMARKS_SIMPLE),
        (Page::Playlists, "Playlists", icons::PLAYLIST),
        (Page::Local, "Local", icons::HARD_DRIVES),
        (Page::Queue, "Queue", icons::QUEUE),
        (Page::History, "History", icons::CLOCK_COUNTER_CLOCKWISE),
        (Page::Settings, "Settings", icons::GEAR_SIX),
    ];
    for (page, label, icon) in pages {
        if let Some(rank) = rank(label, "Go to page", &q) {
            push(&mut out, label.to_owned(), "Go to page".into(), icon, rank, Action::Page(page));
        }
    }

    if !q.is_empty() {
        for c in &app.saved.collections {
            let p = c.platform();
            let meta = format!("{} · Collection · {} songs", p.label(), c.tracks.len());
            if let Some(rank) = rank(&c.name, &meta, &q) {
                push(
                    &mut out,
                    c.name.clone(),
                    meta,
                    icons::BOOKMARK_SIMPLE,
                    rank,
                    Action::Collection(p, c.id),
                );
            }
        }
        for (p, _) in app.saved.platforms() {
            for st in app.saved.all_songs(p) {
                let t = &st.track;
                let artist = t.artist.as_deref().unwrap_or("");
                let meta = format!(
                    "{} · Saved song{}{}",
                    p.label(),
                    if artist.is_empty() { "" } else { " · " },
                    artist
                );
                if let Some(rank) = rank(&t.title, &meta, &q) {
                    push(
                        &mut out,
                        t.title.clone(),
                        meta,
                        icons::MUSIC_NOTES,
                        rank,
                        Action::Play(vec![t.clone()], 0, None),
                    );
                }
            }
        }
        for pl in &app.library.playlists {
            let meta = format!("Playlist · {} songs", pl.tracks.len());
            if let Some(rank) = rank(&pl.name, &meta, &q) {
                push(
                    &mut out,
                    pl.name.clone(),
                    meta,
                    icons::PLAYLIST,
                    rank,
                    Action::Playlist(pl.id),
                );
            }
            for (i, t) in pl.tracks.iter().enumerate() {
                let artist = t.artist.as_deref().unwrap_or("");
                let meta = format!(
                    "Playlist · {}{}",
                    pl.name,
                    if artist.is_empty() { String::new() } else { format!(" · {artist}") }
                );
                if let Some(rank) = rank(&t.title, &meta, &q) {
                    push(
                        &mut out,
                        t.title.clone(),
                        meta,
                        icons::MUSIC_NOTES,
                        rank,
                        Action::Play(pl.tracks.clone(), i, Some(pl.id)),
                    );
                }
            }
        }
        for t in &app.local_page.tracks {
            let artist = t.artist.as_deref().unwrap_or("");
            let meta = format!(
                "Local file{}",
                if artist.is_empty() { String::new() } else { format!(" · {artist}") }
            );
            if let Some(rank) = rank(&t.title, &meta, &q) {
                push(
                    &mut out,
                    t.title.clone(),
                    meta,
                    icons::HARD_DRIVES,
                    rank,
                    Action::Play(vec![t.clone()], 0, None),
                );
            }
        }
    }

    out.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    out.truncate(30);
    out
}

fn result_row(ui: &mut Ui, hit: &Hit, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    let fill = if selected || response.hovered() {
        theme::lerp_color(
            theme::CARD,
            theme::accent().base,
            if selected { 0.14 } else { 0.06 },
        )
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    let left = rect.left() + 10.0;
    painter.text(
        egui::pos2(left, rect.top() + 10.0),
        egui::Align2::LEFT_TOP,
        hit.icon,
        FontId::proportional(18.0),
        theme::MUTED,
    );
    painter.text(
        egui::pos2(left + 28.0, rect.top() + 6.0),
        egui::Align2::LEFT_TOP,
        &hit.title,
        FontId::new(14.0, theme::bold()),
        theme::TEXT,
    );
    painter.text(
        egui::pos2(left + 28.0, rect.top() + 25.0),
        egui::Align2::LEFT_TOP,
        &hit.meta,
        FontId::proportional(12.0),
        theme::MUTED,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn apply(app: &mut App, action: Action) {
    match action {
        Action::Page(page) => app.page = page,
        Action::Collection(platform, id) => {
            app.page = Page::Saved(Some(platform));
            app.saved_page.open = Some(id);
        }
        Action::Playlist(id) => {
            app.page = Page::Playlists;
            app.playlists_page.view = Some(View::Playlist(id));
        }
        Action::Play(tracks, start, playlist) => {
            if !tracks.is_empty() {
                let start = start.min(tracks.len().saturating_sub(1));
                app.play_tracks(tracks, start, playlist);
            }
        }
    }
}
