use std::path::PathBuf;

use eframe::egui::{self, RichText, Slider, Spinner, Ui};
use egui_phosphor::regular as icons;

use crate::app::App;
use crate::task::Task;
use crate::theme;
use crate::tools::{self, ToolPaths, install};

#[derive(Default)]
pub struct SettingsPage {
    update: Option<Task<anyhow::Result<String>>>,
    update_msg: Option<Result<String, String>>,
    version: Option<Task<Option<String>>>,
    pub ytdlp_version: Option<String>,
    version_checked: bool,
}

impl SettingsPage {
    pub fn invalidate_version(&mut self) {
        self.version_checked = false;
        self.ytdlp_version = None;
    }
}

pub fn show(ui: &mut Ui, app: &mut App) {
    let page = &mut app.settings_page;
    if let Some(r) = page.update.as_ref().and_then(Task::take) {
        page.update = None;
        page.update_msg = Some(r.map_err(|e| format!("{e:#}")));
        page.invalidate_version();
    }
    if let Some(v) = page.version.as_ref().and_then(Task::take) {
        page.version = None;
        page.ytdlp_version = v;
    }
    if !page.version_checked && app.tools.ytdlp.is_some() {
        page.version_checked = true;
        let tools = app.tools.clone();
        page.version = Some(Task::spawn(ui.ctx(), move || install::ytdlp_version(&tools)));
    }

    theme::page_header(ui, "Settings", "Settings", "Tools, folders and performance.");

    tools_card(ui, app);
    ui.add_space(16.0);

    theme::card(ui, |ui| {
        ui.label(theme::heading("Folders"));
        ui.add_space(6.0);
        theme::option_row(ui, "Downloads", |ui| {
            ui.add(egui::Label::new(theme::strong(app.settings.download_dir.display().to_string())).truncate());
            if theme::ghost_button(ui, "Change", true).clicked()
                && let Some(d) = rfd::FileDialog::new()
                    .set_directory(&app.settings.download_dir)
                    .pick_folder()
            {
                app.settings.download_dir = d;
            }
        });
        theme::option_row(ui, "Conversions", |ui| {
            match app.settings.convert_dir.clone() {
                Some(d) => {
                    ui.add(egui::Label::new(theme::strong(d.display().to_string())).truncate());
                    if theme::ghost_button(ui, "Use source folder", true).clicked() {
                        app.settings.convert_dir = None;
                    }
                }
                None => {
                    ui.label(theme::strong("Next to the original file"));
                }
            }
            if theme::ghost_button(ui, "Choose…", true).clicked()
                && let Some(d) = rfd::FileDialog::new().pick_folder()
            {
                app.settings.convert_dir = Some(d);
            }
        });
    });
    ui.add_space(16.0);

    theme::card(ui, |ui| {
        ui.label(theme::heading("Performance"));
        ui.add_space(6.0);
        theme::option_row(ui, "Simultaneous jobs", |ui| {
            ui.add(Slider::new(&mut app.settings.concurrency, 1..=6));
        });
    });
    ui.add_space(16.0);

    ui.label(
        RichText::new(format!(
            "Redline {} · yt-dlp {}",
            env!("CARGO_PKG_VERSION"),
            app.settings_page
                .ytdlp_version
                .as_deref()
                .unwrap_or("not installed")
        ))
        .small()
        .color(theme::SUBTLE),
    );
}

fn tools_card(ui: &mut Ui, app: &mut App) {
    let mut changed = false;
    theme::card(ui, |ui| {
        ui.label(theme::heading("Tools"));
        ui.label(theme::muted(
            "Redline runs these programs to do the actual work. Override a path if you keep your own copies.",
        ));
        ui.add_space(8.0);

        let s = &mut app.settings;
        let t = &app.tools;
        let rows: [(&str, &Option<PathBuf>, &mut Option<PathBuf>, &str); 4] = [
            ("yt-dlp", &t.ytdlp, &mut s.ytdlp_path, "Downloads media"),
            ("ffmpeg", &t.ffmpeg, &mut s.ffmpeg_path, "Converts and merges video/audio"),
            ("ffprobe", &t.ffprobe, &mut s.ffprobe_path, "Reads durations for progress"),
            ("Deno", &t.deno, &mut s.deno_path, "Lets yt-dlp solve YouTube's JavaScript checks"),
        ];
        for (name, found, override_path, what) in rows {
            ui.horizontal(|ui| {
                ui.set_min_height(44.0);
                let (icon, color) = if found.is_some() {
                    (icons::CHECK_CIRCLE, theme::TEXT)
                } else {
                    (icons::WARNING_CIRCLE, theme::ACCENT)
                };
                ui.label(RichText::new(icon).size(20.0).color(color));
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(theme::strong(name));
                        ui.label(RichText::new(what).small().color(theme::SUBTLE));
                    });
                    let path_text = found
                        .as_ref()
                        .map_or("Not found".to_owned(), |p| p.display().to_string());
                    ui.add(egui::Label::new(RichText::new(path_text).small().color(theme::MUTED)).truncate());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if override_path.is_some() && theme::ghost_button(ui, "Reset", true).clicked() {
                        *override_path = None;
                        changed = true;
                    }
                    if theme::ghost_button(ui, "Choose…", true).clicked()
                        && let Some(p) = rfd::FileDialog::new()
                            .set_title(format!("Locate {name}"))
                            .pick_file()
                    {
                        *override_path = Some(p);
                        changed = true;
                    }
                });
            });
        }

        ui.add_space(8.0);
        let status = app.installer.status();
        if status.running {
            ui.label(RichText::new(&status.step).color(theme::TEXT));
            theme::progress_line(ui, ui.available_width().min(480.0), status.progress);
        } else if let Some(err) = &status.error {
            ui.label(RichText::new(format!("{} {err}", icons::WARNING)).color(theme::ACCENT_HOVER));
        }
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            if app.tools.missing_any()
                && theme::small_pill_button(ui, "Download missing tools", !status.running).clicked()
            {
                app.installer.start(&app.tools, ui.ctx());
            }
            let page = &mut app.settings_page;
            let updating = page.update.is_some();
            if theme::ghost_button(ui, "Update yt-dlp", app.tools.ytdlp.is_some() && !updating).clicked() {
                let tools: ToolPaths = app.tools.clone();
                page.update_msg = None;
                page.update = Some(Task::spawn(ui.ctx(), move || install::update_ytdlp(&tools)));
            }
            if theme::ghost_button(ui, "Open tools folder", true).clicked() {
                let dir = tools::bin_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = open::that_detached(dir);
            }
            if updating {
                ui.add(Spinner::new().color(theme::ACCENT));
            }
            match &page.update_msg {
                Some(Ok(m)) => {
                    ui.label(theme::muted(m));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::ACCENT_HOVER));
                }
                None => {}
            }
        });
    });
    if changed {
        app.tools = ToolPaths::locate(&app.settings);
        app.settings_page.invalidate_version();
    }
}
