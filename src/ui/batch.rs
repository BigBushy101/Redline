//! Batch page: paste many links, look them up one at a time, then queue or save them.

use eframe::egui::{self, RichText, Spinner, TextEdit, Ui};
use egui_phosphor::regular as icons;
use redline_core::download_plan::LinkChoices;
use redline_core::tools::ytdlp::{self, MediaInfo};

use super::saved;
use crate::app::{App, Page};
use crate::task::Task;
use crate::theme;

/// Most links one batch takes. Each look-up is a yt-dlp run, so this is also a time limit.
const MAX_LINKS: usize = 100;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Target {
    #[default]
    Download,
    Save,
}

struct Item {
    url: String,
    state: State,
}

enum State {
    Waiting,
    Looking,
    Queued(String),
    Saved(String),
    Failed(String),
}

/// The batch page's state, kept in App so switching pages doesn't stop a run.
#[derive(Default)]
pub struct BatchPage {
    pub(crate) input: String,
    target: Target,
    items: Vec<Item>,
    next: usize,
    fetch: Option<Task<anyhow::Result<MediaInfo>>>,
    running: bool,
    error: Option<String>,
}

/// Every http(s) link in the pasted text, in order and without repeats.
fn links_from(text: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        if !(word.starts_with("http://") || word.starts_with("https://")) {
            continue;
        }
        let link = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'']);
        if link.len() > 8 && !links.iter().any(|l| l == link) {
            links.push(link.to_owned());
            if links.len() == MAX_LINKS {
                break;
            }
        }
    }
    links
}

/// Takes in a finished look-up and moves the batch on. Called every frame, whichever page is
/// showing, so a run continues while the user is elsewhere.
pub fn poll(app: &mut App) {
    let Some(result) = app.batch.fetch.as_ref().and_then(Task::take) else {
        return;
    };
    app.batch.fetch = None;
    let index = app.batch.next;
    if index >= app.batch.items.len() {
        app.batch.running = false;
        return;
    }
    let url = app.batch.items[index].url.clone();
    let state = match result {
        Ok(info) => match app.batch.target {
            Target::Download => queue_one(app, &info, &url),
            Target::Save => save_one(app, &info, &url),
        },
        Err(e) => State::Failed(format!("{e:#}")),
    };
    app.batch.items[index].state = state;
    app.batch.next = index + 1;
    advance(app);
}

fn queue_one(app: &mut App, info: &MediaInfo, url: &str) -> State {
    let mut link = LinkChoices::default();
    link.set_info(info.clone());
    let mut settings = app.settings.clone();
    // As on the Download page: a playlist link downloads the whole list.
    settings.download.playlist = info.is_playlist;
    let job = link.plan(url, None, &settings);
    app.jobs.push(job.title, job.subtitle, job.spec);
    // Downloads land in Saved too, under their platform.
    saved::save_info_quietly(app, info, url);
    State::Queued(info.title.clone())
}

fn save_one(app: &mut App, info: &MediaInfo, url: &str) -> State {
    match saved::save_info_result(app, info, url) {
        Ok(out) => State::Saved(out.message()),
        Err(e) => State::Failed(format!("{e:#}")),
    }
}

fn advance(app: &mut App) {
    let index = app.batch.next;
    if index >= app.batch.items.len() {
        app.batch.running = false;
        return;
    }
    app.batch.items[index].state = State::Looking;
    let url = app.batch.items[index].url.clone();
    let target = app.batch.target;
    let tools = app.tools.clone();
    let net = app.settings.network.clone();
    app.batch.fetch = Some(Task::spawn(&app.wake, move || match target {
        Target::Download => ytdlp::fetch_info(&tools, &net, &url),
        // Saving wants the titles of a SoundCloud set's untitled tracks, like the Save box.
        Target::Save => ytdlp::fetch_titled(&tools, &net, &url, &|_| false),
    }));
}

pub fn show(ui: &mut Ui, app: &mut App) {
    let links = links_from(&app.batch.input);
    theme::page_header(
        ui,
        "Batch",
        "Many links at once",
        "Paste one link per line, or drop a text file. Redline looks each one up in turn.",
    );
    let running = app.batch.running;

    theme::card(ui, |ui| {
        ui.label(theme::heading("Links"));
        ui.add_space(6.0);
        ui.add_enabled(
            !running,
            TextEdit::multiline(&mut app.batch.input)
                .desired_rows(6)
                .desired_width(f32::INFINITY)
                .hint_text("https://www.youtube.com/watch?v=…\nhttps://soundcloud.com/…"),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(theme::muted(format!(
                "{} link{} found{}",
                links.len(),
                if links.len() == 1 { "" } else { "s" },
                if links.len() == MAX_LINKS { format!(" (first {MAX_LINKS})") } else { String::new() },
            )));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::ghost_button(ui, "Clear", !running && (!app.batch.input.is_empty() || !app.batch.items.is_empty())).clicked() {
                    app.batch = BatchPage::default();
                }
            });
        });
    });
    ui.add_space(12.0);

    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::muted("Do this with each link:"));
            ui.add_space(6.0);
            ui.add_enabled_ui(!running, |ui| {
                theme::chips(
                    ui,
                    &mut app.batch.target,
                    &[(Target::Download, "Download"), (Target::Save, "Save to Saved")],
                );
            });
        });
    });
    if let Some(e) = &app.batch.error {
        ui.add_space(8.0);
        ui.label(RichText::new(format!("{} {e}", icons::WARNING_CIRCLE)).color(theme::accent().base));
    }

    ui.add_space(16.0);
    let ready = !links.is_empty() && !running && app.tools.ytdlp.is_some();
    ui.horizontal(|ui| {
        if theme::pill_button(ui, &format!("{}  Start", icons::PLAY), ready).clicked() {
            start(app);
        }
        if running && theme::ghost_button(ui, "Stop", true).clicked() {
            app.batch.fetch = None;
            app.batch.running = false;
            if let Some(item) = app.batch.items.get_mut(app.batch.next) {
                if matches!(item.state, State::Looking) {
                    item.state = State::Failed("Stopped".into());
                }
            }
        }
        if !app.batch.items.is_empty() {
            let done = app.batch.items.iter().filter(|i| matches!(i.state, State::Queued(_) | State::Saved(_) | State::Failed(_))).count();
            ui.label(theme::muted(format!("{done} of {} done", app.batch.items.len())));
        }
    });
    if app.tools.ytdlp.is_none() {
        ui.label(theme::muted("Batch needs yt-dlp. Set it up in Settings."));
    }

    if !app.batch.items.is_empty() {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            let queued = app.batch.items.iter().filter(|i| matches!(i.state, State::Queued(_))).count();
            let saved_count = app.batch.items.iter().filter(|i| matches!(i.state, State::Saved(_))).count();
            let failed = app.batch.items.iter().filter(|i| matches!(i.state, State::Failed(_))).count();
            ui.label(theme::heading("Results"));
            ui.add_space(8.0);
            if queued > 0 {
                ui.label(theme::muted(format!("{queued} queued")));
            }
            if saved_count > 0 {
                ui.label(theme::muted(format!("{saved_count} saved")));
            }
            if failed > 0 {
                ui.label(RichText::new(format!("{failed} failed")).color(theme::accent().base));
            }
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for item in &app.batch.items {
                ui.horizontal(|ui| {
                    match &item.state {
                        State::Waiting => ui.label(RichText::new(icons::CIRCLE).color(theme::SUBTLE)),
                        State::Looking => ui.add(Spinner::new().size(14.0).color(theme::accent().base)),
                        State::Queued(_) | State::Saved(_) => {
                            ui.label(RichText::new(icons::CHECK_CIRCLE).color(theme::accent().base))
                        }
                        State::Failed(_) => ui.label(RichText::new(icons::WARNING_CIRCLE).color(theme::accent().base)),
                    };
                    ui.vertical(|ui| {
                        let label = match &item.state {
                            State::Queued(title) => format!("{title} — queued"),
                            State::Saved(message) => message.clone(),
                            State::Failed(error) => format!("Failed — {error}"),
                            State::Looking => "Looking up…".to_owned(),
                            State::Waiting => "Waiting".to_owned(),
                        };
                        ui.add(egui::Label::new(RichText::new(label).color(theme::TEXT)).truncate());
                        ui.add(egui::Label::new(theme::muted(item.url.clone())).truncate());
                    });
                });
                ui.add_space(4.0);
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if app.batch.items.iter().any(|i| matches!(i.state, State::Queued(_)))
                && ui.link(RichText::new("View queue").color(theme::MUTED)).clicked()
            {
                app.page = Page::Queue;
            }
            if app.batch.items.iter().any(|i| matches!(i.state, State::Saved(_)))
                && ui.link(RichText::new("View Saved").color(theme::MUTED)).clicked()
            {
                app.page = Page::Saved(None);
            }
        });
    }
}

fn start(app: &mut App) {
    let links = links_from(&app.batch.input);
    if links.is_empty() {
        app.batch.error = Some("No http(s) links in there.".into());
        return;
    }
    if app.tools.ytdlp.is_none() {
        app.batch.error = Some("Set up yt-dlp in Settings first.".into());
        return;
    }
    app.batch.error = None;
    app.batch.items = links
        .into_iter()
        .map(|url| Item { url, state: State::Waiting })
        .collect();
    app.batch.next = 0;
    app.batch.running = true;
    advance(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_links_in_order_and_without_repeats() {
        let text = "Watch this: https://youtu.be/a, then\nhttps://soundcloud.com/x/y.\nhttps://youtu.be/a";
        assert_eq!(links_from(text), vec!["https://youtu.be/a", "https://soundcloud.com/x/y"]);
    }

    #[test]
    fn ignores_text_that_is_not_a_link() {
        assert!(links_from("hello world").is_empty());
        assert!(links_from("ftp://example.com/a").is_empty());
    }
}
