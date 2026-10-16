pub mod convert;
pub mod download;
pub mod history;
pub mod library;
pub mod now_bar;
pub mod player_panels;
pub mod presets;
pub mod queue;
pub mod settings;
pub mod sidebar;

use eframe::egui::{self, Frame, Margin, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use crate::app::App;
use crate::theme;

pub use crate::format::{fmt_bytes, fmt_duration, parse_duration};

/// Whether the player can play this file (audio, or video for its sound).
pub fn playable(path: &std::path::Path) -> bool {
    use crate::tools::convert::{Category, classify};
    path.is_file() && matches!(classify(path), Some(Category::Audio | Category::Video))
}

/// "Finish setting up" card offering to download the missing tools.
pub fn tools_banner(ui: &mut Ui, app: &mut App, why: &str) {
    let status = app.installer.status();
    let resp = Frame::new()
        .fill(theme::CARD)
        .corner_radius(8)
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::WRENCH).size(28.0).color(theme::accent().base));
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(theme::heading("Finish setting up"));
                    ui.label(theme::muted(why));
                    ui.add_space(10.0);
                    if status.running {
                        ui.label(RichText::new(&status.step).color(theme::TEXT));
                        theme::progress_line(ui, ui.available_width().min(420.0), status.progress);
                    } else {
                        if let Some(err) = &status.error {
                            ui.label(RichText::new(format!("{} {err}", icons::WARNING)).color(theme::accent().hover));
                            ui.add_space(4.0);
                        }
                        let label = if status.error.is_some() { "Try again" } else { "Download tools" };
                        if theme::small_pill_button(ui, label, true).clicked() {
                            app.installer.start(&app.tools, app.wake.clone());
                        }
                        ui.label(
                            RichText::new(format!(
                                "yt-dlp, ffmpeg and Deno, about 200 MB, into {}",
                                crate::tools::bin_dir().display()
                            ))
                            .small()
                            .color(theme::SUBTLE),
                        );
                    }
                });
            });
        });
    // Red accent strip down the card's left edge.
    let r = resp.response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_size(r.min, vec2(4.0, r.height())),
        egui::CornerRadius {
            nw: 8,
            sw: 8,
            ne: 0,
            se: 0,
        },
        theme::accent().base,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_arrow_is_the_icon_font_arrow() {
        assert_eq!(crate::format::ARROW_RIGHT, icons::ARROW_RIGHT);
    }
}
