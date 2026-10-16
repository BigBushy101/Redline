use eframe::egui::{
    self, ComboBox, FontId, Frame, Key, Margin, RichText, Spinner, TextEdit, Ui, vec2,
};
use egui_phosphor::regular as icons;

use crate::app::{App, Page};
use crate::presets::DownloadPreset;
use crate::task::Task;
use crate::theme;
use super::presets::{PresetAction, PresetBar};
use crate::format::{fmt_time, fmt_timestamp, starts_at};
use crate::tools::ytdlp::{
    self, AUDIO_BITRATES, AUDIO_FORMATS, CONTAINERS, DownloadOptions, FormatInfo, MediaInfo,
    Mode, SubsMode, Tags, Trim, VIDEO_QUALITIES,
};
use redline_core::download_plan::{LinkChoices, Schedule};
use redline_core::library::{self, PlaylistId};

use super::library::{AddTarget, add_to, playlist_names};

#[derive(Default)]
pub struct DownloadPage {
    url: String,
    fetching: Option<Task<anyhow::Result<MediaInfo>>>,
    error: Option<String>,
    /// "Added to queue" toast and when it was shown.
    toast: Option<(String, f64)>,
    trim: TrimState,
    /// Lookup result, picked formats and tags for the current link.
    link: LinkChoices,
    presets: PresetBar,
    schedule: Schedule,
}

impl DownloadPage {
    /// Forgets everything tied to the current link.
    fn reset_link(&mut self) {
        self.error = None;
        self.trim = TrimState::default();
        self.link = LinkChoices::default();
    }
}

/// Per-link trim selection, edited with the range bar or the time fields.
#[derive(Default)]
struct TrimState {
    on: bool,
    precise: bool,
    start: f64,
    /// `None` = to the end (only possible when the duration is unknown).
    end: Option<f64>,
    start_text: String,
    end_text: String,
}

impl TrimState {
    fn for_duration(duration: Option<f64>) -> Self {
        let mut t = Self {
            end: duration.map(f64::ceil),
            ..Default::default()
        };
        t.sync_text();
        t
    }

    fn sync_text(&mut self) {
        self.start_text = fmt_time(self.start);
        self.end_text = self.end.map(fmt_time).unwrap_or_default();
    }

    /// Applies whatever currently parses in the time fields.
    fn apply_text(&mut self, duration: Option<f64>) {
        let clamp = |v: f64| duration.map_or(v, |d| v.min(d.ceil()));
        if let Some(v) = super::parse_duration(&self.start_text) {
            self.start = clamp(v);
        }
        if self.end_text.trim().is_empty() && duration.is_none() {
            self.end = None;
        } else if let Some(v) = super::parse_duration(&self.end_text) {
            self.end = Some(clamp(v));
        }
    }

    /// The trim to send with the job, or why the current selection is invalid.
    fn request(&self, duration: Option<f64>) -> Result<Option<Trim>, String> {
        if !self.on {
            return Ok(None);
        }
        if self.end.is_some_and(|e| e <= self.start) {
            return Err("The end has to come after the start.".into());
        }
        if duration.is_some_and(|d| self.start >= d) {
            return Err("The start is past the end of the video.".into());
        }
        let whole = self.start <= 0.0
            && match (duration, self.end) {
                (Some(d), Some(e)) => e >= d.ceil(),
                (_, None) => true,
                (None, Some(_)) => false,
            };
        if whole {
            return Err("Drag the handles or type times to choose the part to keep.".into());
        }
        Ok(Some(Trim {
            start: self.start,
            end: self.end,
            precise: self.precise,
        }))
    }
}

pub fn show(ui: &mut Ui, app: &mut App) {
    let now = ui.input(|i| i.time);
    let page = &mut app.download;

    if let Some(result) = page.fetching.as_ref().and_then(Task::take) {
        page.fetching = None;
        match result {
            Ok(info) => {
                app.settings.download.playlist = info.is_playlist;
                page.trim = TrimState::for_duration(info.duration);
                page.link.set_info(info);
            }
            Err(e) => page.error = Some(format!("{e:#}")),
        }
    }

    theme::page_header(
        ui,
        "Download",
        "Grab anything",
        "Paste a link from YouTube, SoundCloud, Bandcamp, Vimeo and over a thousand other sites.",
    );

    if !app.tools.ready_for_download() {
        super::tools_banner(
            ui,
            app,
            "Downloading needs yt-dlp and ffmpeg. Redline can fetch them for you.",
        );
        ui.add_space(16.0);
    }
    let page = &mut app.download;

    // URL bar
    let can_fetch = app.tools.ytdlp.is_some() && page.fetching.is_none();
    let mut lookup = false;
    ui.horizontal(|ui| {
        let button_w = 120.0;
        Frame::new()
            .fill(theme::INPUT)
            .corner_radius(255)
            .inner_margin(Margin::symmetric(20, 12))
            .show(ui, |ui| {
                ui.set_width((ui.available_width() - button_w - 40.0).max(240.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::LINK).size(20.0).color(theme::MUTED));
                    let resp = ui.add(
                        TextEdit::singleline(&mut page.url)
                            .frame(Frame::NONE)
                            .hint_text("Paste a video, song or playlist link")
                            .font(FontId::proportional(16.0))
                            .desired_width(f32::INFINITY),
                    );
                    if resp.changed() {
                        page.reset_link();
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        lookup = true;
                    }
                });
            });
        ui.add_space(4.0);
        if theme::ghost_button(ui, "Look up", can_fetch && !page.url.trim().is_empty()).clicked() {
            lookup = true;
        }
    });

    let url = page.url.trim().to_owned();
    if lookup && can_fetch && !url.is_empty() {
        page.reset_link();
        let tools = app.tools.clone();
        let net = app.settings.network.clone();
        let fetch_url = url.clone();
        page.fetching = Some(Task::spawn(&app.wake, move || {
            ytdlp::fetch_info(&tools, &net, &fetch_url)
        }));
    }

    ui.add_space(16.0);

    // Lookup result
    let mut card = None;
    if page.fetching.is_some() {
        theme::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(Spinner::new().size(20.0).color(theme::ACCENT));
                ui.label(theme::muted("Looking up link…"));
            });
        });
        ui.add_space(16.0);
    } else if let Some(err) = &page.error {
        theme::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::WARNING_CIRCLE).size(22.0).color(theme::ACCENT));
                ui.add(egui::Label::new(RichText::new(err).color(theme::TEXT)).wrap());
            });
        });
        ui.add_space(16.0);
    } else if let Some(info) = &page.link.info {
        let seen = app.history.find_url(&url).map(|e| e.finished_at);
        let playlists = playlist_names(&app.library);
        card = media_card(ui, info, seen, &playlists).map(|a| (a, library::tracks_from_info(&url, info)));
        ui.add_space(16.0);
    }
    match card {
        Some((CardAction::Play, tracks)) => app.play_tracks(tracks, 0, None),
        Some((CardAction::Add(target), tracks)) => add_to(app, target, tracks),
        None => {}
    }

    // Format options
    let is_playlist = app.download.link.is_playlist();
    format_card(ui, app, is_playlist);
    ui.add_space(16.0);
    let duration = app.download.link.info.as_ref().and_then(|i| i.duration);
    edit_card(ui, app, is_playlist, duration);
    ui.add_space(16.0);
    if app.settings.download.mode == Mode::Audio && !is_playlist && app.download.link.info.is_some() {
        tags_card(ui, &mut app.download.link.tags);
        ui.add_space(16.0);
    }

    // Destination + action
    let dir = app.settings.download_dir.clone();
    theme::card(ui, |ui| {
        theme::option_row(ui, "Save to", |ui| {
            ui.label(RichText::new(icons::FOLDER).size(18.0).color(theme::MUTED));
            ui.add(
                egui::Label::new(RichText::new(dir.display().to_string()).color(theme::TEXT))
                    .truncate(),
            );
            if theme::ghost_button(ui, "Change", true).clicked()
                && let Some(d) = rfd::FileDialog::new().set_directory(&dir).pick_folder()
            {
                app.settings.download_dir = d;
            }
        });
    });
    ui.add_space(24.0);

    let page = &mut app.download;
    let trim = if is_playlist { Ok(None) } else { page.trim.request(duration) };
    let ready = app.tools.ready_for_download() && !url.is_empty() && trim.is_ok();
    schedule_row(ui, &mut page.schedule);
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if theme::pill_button(ui, &format!("{}  Download", icons::DOWNLOAD_SIMPLE), ready).clicked() {
            let trim = trim.ok().flatten();
            let job = page.link.plan(&url, trim, &app.settings);
            let title = job.title.clone();
            app.jobs.push_at(job.title, job.subtitle, job.spec, page.schedule.not_before());
            page.toast = Some((title, now));
            page.url.clear();
            page.reset_link();
        }

        if let Some((title, at)) = &page.toast {
            if now - at < 4.0 {
                ui.add_space(12.0);
                ui.label(RichText::new(icons::CHECK_CIRCLE).size(18.0).color(theme::ACCENT));
                ui.add(
                    egui::Label::new(RichText::new(format!("Added “{title}” to your queue")).color(theme::TEXT))
                        .truncate(),
                );
                if ui
                    .link(RichText::new("View queue").color(theme::MUTED))
                    .clicked()
                {
                    app.page = Page::Queue;
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            } else {
                page.toast = None;
            }
        }
    });
}

/// Trim and clean-up applied while downloading.
fn edit_card(ui: &mut Ui, app: &mut App, is_playlist: bool, duration: Option<f64>) {
    let chapters = app.download.link.info.as_ref().map(|i| i.chapters);
    let t = &mut app.download.trim;
    let o = &mut app.settings.download;
    theme::card(ui, |ui| {
        ui.label(theme::heading("Edit"));
        ui.add_space(6.0);

        if is_playlist {
            ui.label(theme::muted("Trimming works on single videos, not playlists."));
        } else {
            theme::toggle(ui, &mut t.on, "Trim");
            if t.on {
                ui.add_space(8.0);
                if let Some(total) = duration.filter(|d| *d >= 2.0).map(f64::ceil) {
                    let mut end = t.end.unwrap_or(total);
                    if theme::range_bar(ui, &mut t.start, &mut end, total) {
                        t.end = Some(end);
                        t.sync_text();
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("0:00").small().color(theme::SUBTLE));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(super::fmt_duration(total))
                                    .small()
                                    .color(theme::SUBTLE),
                            );
                        });
                    });
                    ui.add_space(4.0);
                }
                theme::option_row(ui, "Keep", |ui| {
                    let mut edited = time_field(ui, &mut t.start_text, "0:00");
                    ui.label(theme::muted("to"));
                    edited |= time_field(ui, &mut t.end_text, if duration.is_some() { "" } else { "end" });
                    if edited {
                        t.apply_text(duration);
                    }
                    if let Some(e) = t.end.filter(|e| *e > t.start) {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!("{} long", fmt_time(e - t.start)))
                                .small()
                                .color(theme::SUBTLE),
                        );
                    }
                });
                theme::option_row(ui, "Cut", |ui| {
                    theme::chips(ui, &mut t.precise, &[(false, "Fast"), (true, "Precise")]);
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(if t.precise {
                            "Re-encodes around the cuts so they land exactly"
                        } else {
                            "Cuts may snap to the nearest keyframe"
                        })
                        .small()
                        .color(theme::SUBTLE),
                    );
                });
                if let Err(e) = t.request(duration) {
                    ui.label(RichText::new(format!("{} {e}", icons::WARNING)).color(theme::MUTED));
                }
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            theme::toggle(ui, &mut o.remove_sponsors, "Remove sponsor segments");
            ui.add_space(8.0);
            ui.label(
                RichText::new("YouTube only · via SponsorBlock")
                    .small()
                    .color(theme::SUBTLE),
            );
        });
        if !is_playlist && !t.on {
            ui.horizontal(|ui| {
                theme::toggle(ui, &mut o.split_chapters, "Split into chapter files");
                ui.add_space(8.0);
                let hint = match chapters {
                    Some(0) => "This video has no chapters".to_owned(),
                    Some(n) => format!("{n} chapters · saved in a folder next to the full file"),
                    None => "Only for videos with chapters".to_owned(),
                };
                ui.label(RichText::new(hint).small().color(theme::SUBTLE));
            });
        }
    });
}

/// Small "m:ss" input. Returns true when the text changed.
fn time_field(ui: &mut Ui, text: &mut String, hint: &str) -> bool {
    Frame::new()
        .fill(theme::INPUT)
        .corner_radius(6)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.add(
                TextEdit::singleline(text)
                    .frame(Frame::NONE)
                    .hint_text(hint)
                    .desired_width(64.0),
            )
            .changed()
        })
        .inner
}

/// Album-header style card: thumbnail, kind, big title, meta line.
enum CardAction {
    Play,
    Add(AddTarget),
}

/// Album-style card for the looked-up link, with Play and Add to playlist.
fn media_card(
    ui: &mut Ui,
    info: &MediaInfo,
    downloaded_at: Option<i64>,
    playlists: &[(PlaylistId, String)],
) -> Option<CardAction> {
    let mut action = None;
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            let size = vec2(232.0, 130.0);
            match &info.thumbnail {
                Some(url) => {
                    ui.add(
                        egui::Image::new(url.as_str())
                            .fit_to_exact_size(size)
                            .maintain_aspect_ratio(false)
                            .corner_radius(6),
                    );
                }
                None => {
                    let icon = if info.is_playlist { icons::PLAYLIST } else { icons::VIDEO };
                    ui.allocate_ui(size, |ui| theme::art_tile(ui, size.y, icon, theme::MUTED));
                }
            }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(8.0);
                let kind = if info.is_playlist { "Playlist" } else { "Video" };
                let label = match &info.extractor {
                    Some(e) => format!("{kind} · {e}"),
                    None => kind.to_owned(),
                };
                ui.label(theme::eyebrow(&label));
                ui.add(
                    egui::Label::new(
                        RichText::new(&info.title)
                            .family(theme::bold())
                            .size(26.0)
                            .color(theme::TEXT),
                    )
                    .wrap(),
                );
                let mut meta = Vec::new();
                if let Some(u) = &info.uploader {
                    meta.push(u.clone());
                }
                if info.is_playlist {
                    meta.push(format!("{} items", info.entries));
                }
                if let Some(d) = info.duration {
                    meta.push(super::fmt_duration(d));
                }
                ui.label(theme::muted(meta.join(" • ")));
                if let Some(at) = downloaded_at {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "{} You downloaded this on {}",
                            icons::CLOCK_COUNTER_CLOCKWISE,
                            fmt_timestamp(at)
                        ))
                        .small()
                        .color(theme::ACCENT_HOVER),
                    );
                }
                let playable = !info.is_playlist || !info.items.is_empty();
                if playable {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let label = if info.is_playlist { "Play all" } else { "Play" };
                        if theme::small_pill_button(ui, &format!("{}  {label}", icons::PLAY), true)
                            .on_hover_text("Stream it now without downloading")
                            .clicked()
                        {
                            action = Some(CardAction::Play);
                        }
                        if let Some(target) = super::library::add_menu(ui, playlists) {
                            action = Some(CardAction::Add(target));
                        }
                    });
                }
            });
        });
    });
    action
}

/// Video/audio choice, quality, exact formats, embeds and subtitles.
fn format_card(ui: &mut Ui, app: &mut App, is_playlist: bool) {
    let page = &mut app.download;
    let o = &mut app.settings.download;
    let presets = &mut app.settings.download_presets;
    let duration = page.link.info.as_ref().and_then(|i| i.duration);
    let has_formats = page
        .link
        .info
        .as_ref()
        .is_some_and(|i| !i.is_playlist && !i.formats.is_empty());
    theme::card(ui, |ui| {
        ui.label(theme::heading("Format"));
        ui.add_space(6.0);
        // "Whole playlist" follows the link, so it doesn't make a preset differ.
        let same = |p: &DownloadPreset| DownloadOptions { playlist: o.playlist, ..p.options.clone() } == *o;
        let names: Vec<String> = presets.iter().map(|p| p.name.clone()).collect();
        match page.presets.show(ui, &names, presets.iter().position(same)) {
            Some(PresetAction::Apply(i)) => {
                *o = DownloadOptions { playlist: o.playlist, ..presets[i].options.clone() };
            }
            Some(PresetAction::Delete(i)) => {
                presets.remove(i);
            }
            Some(PresetAction::Rename(i, name)) => presets[i].name = name,
            Some(PresetAction::SaveNew(name)) => presets.push(DownloadPreset { name, options: o.clone() }),
            None => {}
        }
        ui.add_space(8.0);
        theme::chips(ui, &mut o.mode, &[(Mode::Video, "Video"), (Mode::Audio, "Audio only")]);
        ui.add_space(10.0);

        let picked = match o.mode {
            Mode::Video => page.link.pick_video.is_some(),
            Mode::Audio => page.link.pick_audio.is_some(),
        };
        let formats: &[FormatInfo] = match &page.link.info {
            Some(i) if has_formats => &i.formats,
            _ => &[],
        };
        match o.mode {
            Mode::Video => {
                theme::option_row(ui, "Quality", |ui| {
                    if picked {
                        ui.label(theme::muted("Exact format picked below"));
                    } else if has_formats {
                        quality_combo(ui, o, &ytdlp::height_choices(formats, duration));
                    } else {
                        let current = VIDEO_QUALITIES
                            .iter()
                            .find(|(h, _)| *h == o.max_height)
                            .map_or("Custom", |(_, l)| l);
                        ComboBox::from_id_salt("dl-quality")
                            .selected_text(current)
                            .width(180.0)
                            .show_ui(ui, |ui| {
                                for (h, label) in VIDEO_QUALITIES {
                                    ui.selectable_value(&mut o.max_height, *h, *label);
                                }
                            });
                    }
                });
                theme::option_row(ui, "Container", |ui| {
                    let opts: Vec<(String, &str)> =
                        CONTAINERS.iter().map(|c| (c.to_string(), *c)).collect();
                    theme::chips(ui, &mut o.container, &opts);
                });
            }
            Mode::Audio => {
                theme::option_row(ui, "Format", |ui| {
                    let opts: Vec<(String, &str)> =
                        AUDIO_FORMATS.iter().map(|c| (c.to_string(), *c)).collect();
                    theme::chips(ui, &mut o.audio_format, &opts);
                });
                if !ytdlp::is_lossless(&o.audio_format) {
                    theme::option_row(ui, "Bitrate", |ui| {
                        let current = AUDIO_BITRATES
                            .iter()
                            .find(|(v, _)| *v == o.audio_bitrate)
                            .map_or("Custom", |(_, l)| l);
                        ComboBox::from_id_salt("dl-bitrate")
                            .selected_text(current)
                            .width(180.0)
                            .show_ui(ui, |ui| {
                                for (v, label) in AUDIO_BITRATES {
                                    ui.selectable_value(&mut o.audio_bitrate, v.to_string(), *label);
                                }
                            });
                        if let Some(size) = ytdlp::best_audio_size(formats, duration) {
                            ui.label(
                                RichText::new(format!("source ≈ {}", super::fmt_bytes(size)))
                                    .small()
                                    .color(theme::SUBTLE),
                            );
                        }
                    });
                }
            }
        }

        if has_formats {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(RichText::new("Pick exact formats").color(theme::MUTED))
                .id_salt("dl-exact")
                .show(ui, |ui| {
                    exact_formats(ui, &mut page.link.pick_video, &mut page.link.pick_audio, formats, o.mode, duration)
                });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 28.0;
            theme::toggle(ui, &mut o.embed_thumbnail, "Embed cover art");
            if o.mode == Mode::Audio && o.embed_thumbnail {
                theme::toggle(ui, &mut o.square_cover, "Square cover");
            }
            theme::toggle(ui, &mut o.embed_metadata, "Embed metadata");
            theme::toggle(ui, &mut o.embed_subs, "Subtitles");
            if is_playlist {
                theme::toggle(ui, &mut o.playlist, "Download whole playlist");
            }
        });
        if o.embed_subs {
            ui.add_space(6.0);
            subtitle_options(ui, o, page.link.info.as_ref());
        }
    });
}

fn quality_combo(ui: &mut Ui, o: &mut DownloadOptions, choices: &[ytdlp::HeightChoice]) {
    let label = |c: &ytdlp::HeightChoice| {
        let mut s = format!("{}p", c.height);
        if let Some(f) = c.fps.filter(|f| *f > 30.5) {
            s.push_str(&format!("{}", f.round()));
        }
        if let Some(size) = c.size {
            s.push_str(&format!("  ·  ≈ {}", super::fmt_bytes(size)));
        }
        s
    };
    let current = if o.max_height == 0 {
        choices
            .first()
            .map_or("Best available".to_owned(), |c| format!("Best ({})", label(c)))
    } else {
        // yt-dlp picks the tallest at or below the limit.
        choices
            .iter()
            .find(|c| c.height <= o.max_height)
            .map_or_else(|| format!("Up to {}p", o.max_height), label)
    };
    ComboBox::from_id_salt("dl-quality-real")
        .selected_text(current)
        .width(240.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut o.max_height, 0, "Best available");
            for c in choices {
                ui.selectable_value(&mut o.max_height, c.height, label(c));
            }
        });
}

fn exact_formats(
    ui: &mut Ui,
    pick_video: &mut Option<String>,
    pick_audio: &mut Option<String>,
    formats: &[FormatInfo],
    mode: Mode,
    duration: Option<f64>,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Overrides the quality setting for this link only.")
                .small()
                .color(theme::SUBTLE),
        );
        if (pick_video.is_some() || pick_audio.is_some()) && ui.link("Back to automatic").clicked() {
            *pick_video = None;
            *pick_audio = None;
        }
    });
    let row = |f: &FormatInfo| {
        let codec = match (&f.vcodec, &f.acodec) {
            (Some(v), Some(a)) => format!("{} + {}", short_codec(v), short_codec(a)),
            (Some(v), None) => short_codec(v).to_owned(),
            (None, Some(a)) => short_codec(a).to_owned(),
            (None, None) => String::new(),
        };
        let size = f
            .estimated_size(duration)
            .map_or("?".to_owned(), super::fmt_bytes);
        format!("{:<8}{:<12}{:<6}{:<20}{size}", f.id, f.quality_label(), f.ext, codec)
    };
    let list = |ui: &mut Ui, title: &str, pick: &mut Option<String>, keep: &dyn Fn(&FormatInfo) -> bool| {
        ui.add_space(4.0);
        ui.label(theme::eyebrow(title));
        let mut rows: Vec<&FormatInfo> = formats.iter().filter(|f| keep(f)).collect();
        rows.sort_by(|a, b| {
            (b.height.unwrap_or(0), b.tbr.unwrap_or(0.0) as u64)
                .cmp(&(a.height.unwrap_or(0), a.tbr.unwrap_or(0.0) as u64))
        });
        for f in rows {
            let selected = pick.as_deref() == Some(f.id.as_str());
            if ui
                .selectable_label(selected, RichText::new(row(f)).monospace().size(12.0))
                .clicked()
            {
                *pick = if selected { None } else { Some(f.id.clone()) };
            }
        }
    };
    if mode == Mode::Video {
        list(ui, "Video", pick_video, &|f| f.has_video());
    }
    list(ui, "Audio", pick_audio, &|f| f.has_audio() && !f.has_video());
}

fn short_codec(c: &str) -> &str {
    c.split('.').next().unwrap_or(c)
}

fn subtitle_options(ui: &mut Ui, o: &mut DownloadOptions, info: Option<&MediaInfo>) {
    theme::option_row(ui, "Languages", |ui| {
        text_input(ui, &mut o.sub_langs, ytdlp::DEFAULT_SUB_LANGS, 200.0);
        ui.label(
            RichText::new("Comma separated · \"all\" for every language")
                .small()
                .color(theme::SUBTLE),
        );
    });
    let single = info.filter(|i| !i.is_playlist);
    if let Some(info) = single.filter(|i| !i.subtitle_langs.is_empty()) {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(150.0);
            ui.label(RichText::new("Available:").small().color(theme::SUBTLE));
            for lang in info.subtitle_langs.iter().take(24) {
                let mut langs: Vec<String> = o
                    .sub_langs
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect();
                let on = langs.contains(lang);
                if ui.selectable_label(on, lang).clicked() {
                    if on {
                        langs.retain(|l| l != lang);
                    } else {
                        langs.push(lang.clone());
                    }
                    o.sub_langs = langs.join(",");
                }
            }
        });
    } else if let Some(info) = single {
        ui.horizontal(|ui| {
            ui.add_space(150.0);
            let msg = if info.has_auto_captions {
                "No uploaded subtitles; turn on auto-generated captions below."
            } else {
                "This video has no subtitles."
            };
            ui.label(RichText::new(msg).small().color(theme::SUBTLE));
        });
    }
    ui.horizontal(|ui| {
        ui.add_space(150.0);
        theme::toggle(ui, &mut o.auto_subs, "Include auto-generated captions");
    });
    if o.mode == Mode::Video {
        theme::option_row(ui, "Save as", |ui| {
            theme::chips(
                ui,
                &mut o.subs_mode,
                &[(SubsMode::Embed, "Inside the video"), (SubsMode::Srt, ".srt files")],
            );
        });
    } else {
        ui.horizontal(|ui| {
            ui.add_space(150.0);
            ui.label(
                RichText::new("Saved as .srt files next to the audio")
                    .small()
                    .color(theme::SUBTLE),
            );
        });
    }
}

fn tags_card(ui: &mut Ui, tags: &mut Tags) {
    theme::card(ui, |ui| {
        ui.label(theme::heading("Tags"));
        ui.label(theme::muted("Written into the audio file. Edit to fix names from the site."));
        ui.add_space(6.0);
        theme::option_row(ui, "Title", |ui| text_input(ui, &mut tags.title, "", 360.0));
        theme::option_row(ui, "Artist", |ui| text_input(ui, &mut tags.artist, "", 360.0));
        theme::option_row(ui, "Album", |ui| text_input(ui, &mut tags.album, "", 360.0));
    });
}

fn text_input(ui: &mut Ui, text: &mut String, hint: &str, width: f32) {
    Frame::new()
        .fill(theme::INPUT)
        .corner_radius(6)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.add(
                TextEdit::singleline(text)
                    .frame(Frame::NONE)
                    .hint_text(hint)
                    .desired_width(width),
            );
        });
}

fn schedule_row(ui: &mut Ui, s: &mut Schedule) {
    ui.horizontal(|ui| {
        ui.label(theme::muted("Start"));
        theme::chips(ui, &mut s.later, &[(false, "Now"), (true, "Later")]);
        if s.later {
            ui.add_space(8.0);
            ui.label(theme::muted("at"));
            ui.add(egui::DragValue::new(&mut s.hour).range(0..=23).custom_formatter(|v, _| format!("{v:02}")));
            ui.label(theme::muted(":"));
            ui.add(egui::DragValue::new(&mut s.minute).range(0..=59).custom_formatter(|v, _| format!("{v:02}")));
            if let Some(t) = s.next() {
                ui.label(
                    RichText::new(format!(
                        "{} · Redline has to stay open",
                        starts_at(t)
                    ))
                    .small()
                    .color(theme::SUBTLE),
                );
            }
        }
    });
}
