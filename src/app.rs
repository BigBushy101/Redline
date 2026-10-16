use eframe::egui::{
    self, CentralPanel, CornerRadius, FontId, Frame, Margin, Panel, ScrollArea, UiBuilder,
};
use egui_phosphor::regular as icons;

use redline_core::Wake;
use redline_core::library::{self, Library, NowPlaying, PlaylistId, Track};
use redline_core::player::{Command, Player, PlayerSettings, PlayerState, Source};
use redline_core::queue_watch::{self, AfterQueueStep, QueueWatch};

use crate::history::History;
use crate::jobs::{JobManager, JobStatus};
use crate::system;
use crate::settings::Settings;
use crate::theme;
use crate::task::{self, Task};
use crate::tools::ToolPaths;
use crate::tools::hwaccel::{self, Encoders};
use crate::tools::install::Installer;
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Download,
    Convert,
    Library,
    Queue,
    History,
    Settings,
}

pub struct App {
    pub page: Page,
    pub settings: Settings,
    saved_settings: Settings,
    pub tools: ToolPaths,
    /// Video encoders that work on this machine (CPU only until detection finishes).
    pub encoders: Encoders,
    encoders_task: Option<Task<Encoders>>,
    pub jobs: JobManager,
    pub history: History,
    pub installer: Installer,
    pub download: ui::download::DownloadPage,
    pub convert: ui::convert::ConvertPage,
    pub history_page: ui::history::HistoryPage,
    pub settings_page: ui::settings::SettingsPage,
    pub library: Library,
    pub library_page: ui::library::LibraryPage,
    /// What the player bar works through: a playlist or ad-hoc picks.
    pub now: NowPlaying,
    /// Auto-advances of the player the queue has caught up with.
    seen_advances: u64,
    /// The next track (by its own source) last handed to the player.
    sent_next: Option<Option<Source>>,
    /// Windows media keys and overlay.
    #[cfg(windows)]
    media_keys: Option<redline_core::media_keys::MediaKeys>,
    /// Notifications, keep-awake and the after-queue action.
    pub watch: QueueWatch,
    /// Repaints the window; handed to background work.
    pub wake: Wake,
    pub player: Player,
    /// This frame's copy of the player's state.
    pub player_state: PlayerState,
    /// Volume to go back to when unmuting.
    pub unmuted_volume: f32,
    /// Network settings the player was last given.
    player_network: Option<crate::tools::ytdlp::NetworkOptions>,
    /// Volume, speed and EQ the player was last given.
    player_sent: Option<PlayerSettings>,
    /// The load the speed was last reset for.
    seen_load: u64,
    /// Speed or EQ popup above the player bar.
    pub player_panel: Option<ui::player_panels::Panel>,
    /// egui time when a shutdown was scheduled, for the countdown.
    pub shutdown_at: Option<f64>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings = Settings::load();
        let tools = ToolPaths::locate(&settings);
        let wake = task::waker(&cc.egui_ctx);
        let encoders_task = Self::detect_encoders(&wake, &tools);
        let mut jobs = JobManager::load();
        jobs.set_wake(wake.clone());
        let player = Player::system(tools.clone(), wake.clone());
        #[cfg(windows)]
        let media_keys = Self::media_keys(cc, &player, &wake);
        Self {
            page: Page::Download,
            tools: tools.clone(),
            saved_settings: settings.clone(),
            settings,
            encoders: Encoders::default(),
            encoders_task,
            jobs,
            history: History::load(),
            installer: Installer::default(),
            download: Default::default(),
            convert: Default::default(),
            history_page: Default::default(),
            settings_page: Default::default(),
            library: Library::load(),
            library_page: Default::default(),
            now: NowPlaying::default(),
            seen_advances: 0,
            sent_next: None,
            #[cfg(windows)]
            media_keys,
            watch: QueueWatch::default(),
            player,
            player_state: PlayerState::default(),
            unmuted_volume: 1.0,
            player_network: None,
            player_sent: None,
            seen_load: 0,
            player_panel: None,
            wake,
            shutdown_at: None,
        }
    }

    /// Hooks up the media keys to the window. They're a nicety, so failing is fine.
    #[cfg(windows)]
    fn media_keys(cc: &eframe::CreationContext<'_>, player: &Player, wake: &Wake) -> Option<redline_core::media_keys::MediaKeys> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(w) = cc.window_handle().ok()?.as_raw() else {
            return None;
        };
        redline_core::media_keys::MediaKeys::new(w.hwnd.get() as *mut std::ffi::c_void, player.remote(), wake.clone()).ok()
    }

    /// Acts on media keys that need the queue, and keeps the Windows overlay current.
    #[cfg(windows)]
    fn media_keys_frame(&mut self, ctx: &egui::Context) {
        use redline_core::media_keys::MediaKey;
        let Some(mk) = self.media_keys.as_mut() else { return };
        mk.update(&self.player_state);
        for key in mk.take_keys() {
            match key {
                MediaKey::Next => self.next_track(),
                MediaKey::Previous => self.prev_track(),
                MediaKey::SeekBy(secs) => {
                    self.player.send(Command::Seek((self.player_state.position + secs).max(0.0)));
                }
                MediaKey::Raise => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
        }
    }

    fn detect_encoders(wake: &Wake, tools: &ToolPaths) -> Option<Task<Encoders>> {
        let ffmpeg = tools.ffmpeg.clone()?;
        Some(Task::spawn(wake, move || hwaccel::detect(&ffmpeg)))
    }

    /// Notifies about jobs that just ended, if Redline isn't the focused window.
    fn announce(&self, ctx: &egui::Context, ended: &[(String, JobStatus)]) {
        let Some(a) = queue_watch::announcement(ended, self.jobs.active_count()) else {
            return;
        };
        if ctx.input(|i| i.viewport().focused).unwrap_or(true) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
            egui::UserAttentionType::Informational,
        ));
        if self.settings.notify {
            system::notify(a.title, a.body);
        }
    }

    /// Opens a popup above the player bar, or closes it if it's open.
    pub fn toggle_panel(&mut self, panel: ui::player_panels::Panel) {
        self.player_panel = if self.player_panel == Some(panel) { None } else { Some(panel) };
    }

    /// Hands changed volume, speed and EQ settings to the player.
    fn sync_player(&mut self) {
        let st = &self.player_state;
        if st.load_id != self.seen_load {
            self.seen_load = st.load_id;
            if self.settings.player.reset_speed {
                self.settings.player.speed = 1.0;
            }
        }
        let want = &self.settings.player;
        let sent = self.player_sent.as_ref();
        if sent.is_none_or(|s| s.volume != want.volume) {
            self.player.send(Command::SetVolume(want.volume));
        }
        if sent.is_none_or(|s| s.speed != want.speed) {
            self.player.send(Command::SetSpeed(want.speed));
        }
        if sent.is_none_or(|s| s.eq != want.eq) {
            self.player.send(Command::SetEq(want.eq));
        }
        self.player_sent = Some(want.clone());
    }

    /// Plays a file in the player bar.
    pub fn play_file(&mut self, path: std::path::PathBuf) {
        let source = Source::Local(path);
        let title = source.label();
        self.play_tracks(vec![Track::new(source, title)], 0, None);
    }

    /// Replaces Now playing with `tracks` and starts at `start`.
    pub fn play_tracks(&mut self, tracks: Vec<Track>, start: usize, playlist: Option<PlaylistId>) {
        self.now.start(tracks, start, playlist);
        self.load_current();
    }

    pub fn play_playlist(&mut self, id: PlaylistId, start: usize, shuffle: bool) {
        let Some(p) = self.library.get(id) else { return };
        let tracks = p.tracks.clone();
        let start = if shuffle && !tracks.is_empty() {
            // Shuffle from a random first track, not always the top one.
            self.now.shuffle = true;
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            nanos as usize % tracks.len()
        } else {
            start
        };
        self.play_tracks(tracks, start, Some(id));
    }

    /// Adds tracks to the end of Now playing, starting it if nothing's queued.
    pub fn queue_tracks(&mut self, tracks: Vec<Track>) {
        if self.now.is_empty() {
            self.play_tracks(tracks, 0, None);
        } else {
            self.now.append(tracks);
        }
    }

    pub fn jump_to(&mut self, index: usize) {
        if self.now.jump(index).is_some() {
            self.load_current();
        }
    }

    pub fn next_track(&mut self) {
        if self.now.next().is_some() {
            self.load_current();
        }
    }

    /// Back to the start of the track, or the one before if it only just started.
    pub fn prev_track(&mut self) {
        if self.player_state.position > 3.0 || !self.now.has_prev() {
            self.player.send(Command::Seek(0.0));
        } else if self.now.prev().is_some() {
            self.load_current();
        }
    }

    pub fn stop_queue(&mut self) {
        self.now.clear();
        self.player.send(Command::Stop);
    }

    /// The source the player has loaded, if any.
    pub fn now_playing_source(&self) -> Option<Source> {
        self.player_state.source.clone()
    }

    /// Loads the queue's current track, from its downloaded copy when there is one.
    fn load_current(&mut self) {
        if let Some(t) = self.now.current() {
            let source = library::playable_source(t, &self.history);
            self.player.send(Command::Load(source, 0.0));
            // A load clears the player's next track; hand it over again.
            self.sent_next = None;
        }
    }

    /// Keeps the player told what follows the current track, and catches the queue up when
    /// the player has moved on by itself.
    fn follow_queue(&mut self) {
        let advances = self.player_state.advances;
        if advances != self.seen_advances {
            for _ in self.seen_advances..advances {
                self.now.after_end();
            }
            self.seen_advances = advances;
            self.sent_next = None;
        }
        // Compared by the track's own source, so History is only searched on a change.
        let next = self.now.peek_after_end().map(|t| t.source.clone());
        if self.sent_next.as_ref() != Some(&next) {
            let playable = self.now.peek_after_end().map(|t| library::playable_source(t, &self.history));
            self.player.send(Command::SetNext(playable));
            self.sent_next = Some(next);
        }
    }

    fn run_after_queue(&mut self, ctx: &egui::Context, step: AfterQueueStep) {
        match step {
            AfterQueueStep::OpenFolder(dir) => {
                let _ = open::that_detached(dir);
            }
            AfterQueueStep::Sleep => system::sleep_now(),
            AfterQueueStep::ShutDown => {
                if system::schedule_shutdown().is_ok() {
                    self.shutdown_at = Some(ctx.input(|i| i.time));
                }
            }
        }
    }

    /// Countdown with a Cancel button while Windows is about to shut down.
    fn shutdown_window(&mut self, ctx: &egui::Context) {
        let Some(at) = self.shutdown_at else { return };
        let left = system::SHUTDOWN_DELAY_SECS as f64 - (ctx.input(|i| i.time) - at);
        if left <= 0.0 {
            self.shutdown_at = None;
            return;
        }
        egui::Window::new("Shutting down")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(theme::strong("The queue is done."));
                ui.label(theme::muted(format!(
                    "Windows will shut down in {} seconds.",
                    left.ceil() as u32
                )));
                ui.add_space(8.0);
                if theme::small_pill_button(ui, "Cancel shutdown", true).clicked() {
                    system::cancel_shutdown();
                    self.shutdown_at = None;
                }
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        if self.page == Page::Library && ui::library::drop_files(self, dropped.clone()) {
            return;
        }
        self.convert.add_paths(dropped);
        self.page = Page::Convert;
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.installer.take_finished() {
            self.tools = ToolPaths::locate(&self.settings);
            self.player.send(Command::SetTools(self.tools.clone()));
            self.settings_page.invalidate_version();
            self.encoders_task = Self::detect_encoders(&self.wake, &self.tools);
        }
        self.player_state = self.player.state();
        self.sync_player();
        self.follow_queue();
        #[cfg(windows)]
        self.media_keys_frame(ctx);
        self.library_page.poll(&mut self.library);
        if self.player_network.as_ref() != Some(&self.settings.network) {
            self.player_network = Some(self.settings.network.clone());
            self.player.send(Command::SetNetwork(self.settings.network.clone()));
        }
        if let Some(e) = self.encoders_task.as_ref().and_then(Task::take) {
            self.encoders = e;
            self.encoders_task = None;
        }
        // Wake up when the next scheduled job is due.
        if let Some(after) = self.jobs.pump(&self.tools, self.settings.concurrency) {
            ctx.request_repaint_after(after);
        }
        let mut ended = Vec::new();
        for id in self.jobs.take_finished() {
            if let Some(job) = self.jobs.get(id) {
                self.history.record(job);
                self.watch.job_ended(job);
                ended.push((job.title.clone(), job.status.clone()));
            }
        }
        self.announce(ctx, &ended);

        let active = self.jobs.active_count();
        if let Some(step) = self.watch.update(active, &self.settings.download_dir) {
            self.run_after_queue(ctx, step);
        }
        self.jobs.save_if_dirty();
        self.history.save_if_dirty();
        self.library.save_if_dirty();
        if self.settings != self.saved_settings {
            self.settings.save();
            self.saved_settings = self.settings.clone();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);
        self.shutdown_window(&ctx);
        ui::player_panels::show(&ctx, self);

        Panel::bottom("now-bar")
            .exact_size(84.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(theme::BLACK).inner_margin(Margin {
                left: 16,
                right: 16,
                top: 8,
                bottom: 8,
            }))
            .show(ui, |ui| ui::now_bar::show(ui, self));

        Panel::left("sidebar")
            .exact_size(260.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(theme::BLACK).inner_margin(Margin {
                left: 8,
                right: 4,
                top: 8,
                bottom: 0,
            }))
            .show(ui, |ui| {
                Frame::new()
                    .fill(theme::BG)
                    .corner_radius(8)
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        ui::sidebar::show(ui, self);
                    });
            });

        CentralPanel::default()
            .frame(Frame::new().fill(theme::BLACK).inner_margin(Margin {
                left: 4,
                right: 8,
                top: 8,
                bottom: 0,
            }))
            .show(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(8), theme::BG);
                theme::paint_header_gradient(ui.painter(), rect);

                let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
                child.set_clip_rect(rect);
                ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(&mut child, |ui| {
                        Frame::new()
                            .inner_margin(Margin {
                                left: 32,
                                right: 32,
                                top: 20,
                                bottom: 40,
                            })
                            .show(ui, |ui| match self.page {
                                Page::Download => ui::download::show(ui, self),
                                Page::Convert => ui::convert::show(ui, self),
                                Page::Library => ui::library::show(ui, self),
                                Page::Queue => ui::queue::show(ui, self),
                                Page::History => ui::history::show(ui, self),
                                Page::Settings => ui::settings::show(ui, self),
                            });
                    });
            });

        // Full-window hint while files are dragged over.
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let screen = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop-overlay"),
            ));
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(190));
            painter.rect_stroke(
                screen.shrink(12.0),
                CornerRadius::same(12),
                egui::Stroke::new(2.0, theme::accent().base),
                egui::StrokeKind::Inside,
            );
            painter.text(
                screen.center() - egui::vec2(0.0, 24.0),
                egui::Align2::CENTER_CENTER,
                icons::FILE_PLUS,
                FontId::proportional(64.0),
                theme::accent().base,
            );
            painter.text(
                screen.center() + egui::vec2(0.0, 36.0),
                egui::Align2::CENTER_CENTER,
                if self.page == Page::Library && matches!(self.library_page.view, Some(ui::library::View::Playlist(_))) {
                    "Drop to add to the playlist"
                } else {
                    "Drop to convert"
                },
                FontId::new(22.0, theme::bold()),
                theme::TEXT,
            );
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Don't leave yt-dlp/ffmpeg running after the window closes.
        self.jobs.cancel_all();
    }
}
