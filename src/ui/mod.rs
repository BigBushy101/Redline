pub mod convert;
pub mod download;
pub mod history;
pub mod now_bar;
pub mod queue;
pub mod settings;
pub mod sidebar;

use eframe::egui::{self, Frame, Margin, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use crate::app::App;
use crate::theme;

pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Parses `90`, `1:30`, `1:02:03` or `12.5` into seconds.
pub fn parse_duration(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut secs = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let v: f64 = part.trim().parse().ok().filter(|v: &f64| *v >= 0.0)?;
        // Only the last field may be fractional, and minutes/seconds after a colon stay under 60.
        if i + 1 < parts.len() && v.fract() != 0.0 || i > 0 && v >= 60.0 {
            return None;
        }
        secs = secs * 60.0 + v;
    }
    Some(secs)
}

pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
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
                ui.label(RichText::new(icons::WRENCH).size(28.0).color(theme::ACCENT));
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
                            ui.label(RichText::new(format!("{} {err}", icons::WARNING)).color(theme::ACCENT_HOVER));
                            ui.add_space(4.0);
                        }
                        let label = if status.error.is_some() { "Try again" } else { "Download tools" };
                        if theme::small_pill_button(ui, label, true).clicked() {
                            app.installer.start(&app.tools, ui.ctx());
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
        theme::ACCENT,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_duration(5.0), "0:05");
        assert_eq!(fmt_duration(125.4), "2:05");
        assert_eq!(fmt_duration(3725.0), "1:02:05");
        assert_eq!(parse_duration("90"), Some(90.0));
        assert_eq!(parse_duration(" 1:30 "), Some(90.0));
        assert_eq!(parse_duration("1:02:03"), Some(3723.0));
        assert_eq!(parse_duration("0:12.5"), Some(12.5));
        assert_eq!(parse_duration("1:75"), None);
        assert_eq!(parse_duration("1.5:00"), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
