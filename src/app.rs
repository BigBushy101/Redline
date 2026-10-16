use eframe::egui::{
    self, CentralPanel, CornerRadius, FontId, Frame, Margin, Panel, ScrollArea, UiBuilder,
};
use egui_phosphor::regular as icons;

use crate::history::History;
use crate::jobs::JobManager;
use crate::settings::Settings;
use crate::theme;
use crate::tools::ToolPaths;
use crate::tools::install::Installer;
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Download,
    Convert,
    Queue,
    History,
    Settings,
}

pub struct App {
    pub page: Page,
    pub settings: Settings,
    saved_settings: Settings,
    pub tools: ToolPaths,
    pub jobs: JobManager,
    pub history: History,
    pub installer: Installer,
    pub download: ui::download::DownloadPage,
    pub convert: ui::convert::ConvertPage,
    pub history_page: ui::history::HistoryPage,
    pub settings_page: ui::settings::SettingsPage,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings = Settings::load();
        let tools = ToolPaths::locate(&settings);
        Self {
            page: Page::Download,
            saved_settings: settings.clone(),
            settings,
            tools,
            jobs: JobManager::load(),
            history: History::load(),
            installer: Installer::default(),
            download: Default::default(),
            convert: Default::default(),
            history_page: Default::default(),
            settings_page: Default::default(),
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.convert.add_paths(dropped);
            self.page = Page::Convert;
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.installer.take_finished() {
            self.tools = ToolPaths::locate(&self.settings);
            self.settings_page.invalidate_version();
        }
        self.jobs
            .pump(ctx, &self.tools, self.settings.concurrency);
        for id in self.jobs.take_finished() {
            if let Some(job) = self.jobs.get(id) {
                self.history.record(job);
            }
        }
        self.jobs.save_if_dirty();
        self.history.save_if_dirty();
        if self.settings != self.saved_settings {
            self.settings.save();
            self.saved_settings = self.settings.clone();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);

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
                egui::Stroke::new(2.0, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
            painter.text(
                screen.center() - egui::vec2(0.0, 24.0),
                egui::Align2::CENTER_CENTER,
                icons::FILE_PLUS,
                FontId::proportional(64.0),
                theme::ACCENT,
            );
            painter.text(
                screen.center() + egui::vec2(0.0, 36.0),
                egui::Align2::CENTER_CENTER,
                "Drop to convert",
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
