use eframe::egui::{
    self, ComboBox, FontId, Frame, Key, Margin, RichText, Spinner, TextEdit, Ui, vec2,
};
use egui_phosphor::regular as icons;

use crate::app::{App, Page};
use crate::jobs::JobSpec;
use crate::task::Task;
use crate::theme;
use crate::tools::ytdlp::{
    self, AUDIO_BITRATES, AUDIO_FORMATS, CONTAINERS, DownloadRequest, MediaInfo, Mode, Trim,
    VIDEO_QUALITIES,
};

#[derive(Default)]
pub struct DownloadPage {
    url: String,
    info: Option<MediaInfo>,
    fetching: Option<Task<anyhow::Result<MediaInfo>>>,
    error: Option<String>,
    /// "Added to queue" toast and when it was shown.
    toast: Option<(String, f64)>,
    trim: TrimState,
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

/// Like `fmt_duration` but keeps tenths of a second when the time isn't whole.
fn fmt_time(secs: f64) -> String {
    let tenths = (secs.fract() * 10.0).round() as u32;
    if tenths == 0 || tenths >= 10 {
        super::fmt_duration(secs)
    } else {
        format!("{}.{tenths}", super::fmt_duration(secs.trunc()))
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
                page.info = Some(info);
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
                        page.info = None;
                        page.error = None;
                        page.trim = TrimState::default();
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
        page.info = None;
        page.error = None;
        page.trim = TrimState::default();
        let tools = app.tools.clone();
        let net = app.settings.network.clone();
        let fetch_url = url.clone();
        page.fetching = Some(Task::spawn(ui.ctx(), move || {
            ytdlp::fetch_info(&tools, &net, &fetch_url)
        }));
    }

    ui.add_space(16.0);

    // Lookup result
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
    } else if let Some(info) = &page.info {
        let seen = app.history.find_url(&url).map(|e| e.finished_at);
        media_card(ui, info, seen);
        ui.add_space(16.0);
    }

    // Format options
    let is_playlist = page.info.as_ref().is_some_and(|i| i.is_playlist);
    let o = &mut app.settings.download;
    theme::card(ui, |ui| {
        ui.label(theme::heading("Format"));
        ui.add_space(6.0);
        theme::chips(ui, &mut o.mode, &[(Mode::Video, "Video"), (Mode::Audio, "Audio only")]);
        ui.add_space(10.0);

        match o.mode {
            Mode::Video => {
                theme::option_row(ui, "Quality", |ui| {
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
                    });
                }
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 28.0;
            theme::toggle(ui, &mut o.embed_thumbnail, "Embed cover art");
            theme::toggle(ui, &mut o.embed_metadata, "Embed metadata");
            if o.mode == Mode::Video {
                theme::toggle(ui, &mut o.embed_subs, "Embed subtitles");
            }
            if is_playlist {
                theme::toggle(ui, &mut o.playlist, "Download whole playlist");
            }
        });
    });
    ui.add_space(16.0);

    let duration = app.download.info.as_ref().and_then(|i| i.duration);
    edit_card(ui, app, is_playlist, duration);
    ui.add_space(16.0);

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
    ui.horizontal(|ui| {
        if theme::pill_button(ui, &format!("{}  Download", icons::DOWNLOAD_SIMPLE), ready).clicked() {
            let trim = trim.ok().flatten();
            let title = page
                .info
                .as_ref()
                .map_or_else(|| url.clone(), |i| i.title.clone());
            let options = app.settings.download.clone();
            let mut options_for_job = options.clone();
            if !is_playlist {
                options_for_job.playlist = false;
            }
            let mut summary = options.summary();
            if let Some(t) = &trim {
                let end = t.end.map_or("end".to_owned(), fmt_time);
                summary.push_str(&format!(" · Clip {}–{end}", fmt_time(t.start)));
            }
            app.jobs.push(
                title.clone(),
                summary,
                JobSpec::Download(DownloadRequest {
                    url: url.clone(),
                    options: options_for_job,
                    out_dir: app.settings.download_dir.clone(),
                    // Clips of something already downloaded are still wanted.
                    archive: (app.settings.skip_downloaded && trim.is_none())
                        .then(crate::settings::archive_path),
                    network: app.settings.network.clone(),
                    trim,
                }),
            );
            page.toast = Some((title, now));
            page.url.clear();
            page.info = None;
            page.trim = TrimState::default();
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
fn media_card(ui: &mut Ui, info: &MediaInfo, downloaded_at: Option<i64>) {
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
                            crate::history::fmt_timestamp(at)
                        ))
                        .small()
                        .color(theme::ACCENT_HOVER),
                    );
                }
            });
        });
    });
}
