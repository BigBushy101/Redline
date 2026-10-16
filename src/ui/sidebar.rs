use eframe::egui::{self, CornerRadius, FontId, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icons;

use crate::app::{App, Page};
use crate::theme;

/// One sub-item under Saved per platform that has something saved, in its color.
fn saved_platforms(ui: &mut Ui, app: &mut App) {
    for (p, count) in app.saved.platforms() {
        let selected = app.page == Page::Saved(Some(p));
        let clicked = theme::with_accent(ui, theme::accent_for(p), |ui| {
            theme::nav_sub_item(ui, theme::platform_glyph(p), p.label(), selected, count).clicked()
        });
        if clicked {
            app.page = Page::Saved(Some(p));
            app.saved_page.open = None;
        }
        ui.add_space(2.0);
    }
}

pub fn show(ui: &mut Ui, app: &mut App) {
    // Logo
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 18.0, theme::accent().base);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icons::VINYL_RECORD,
            FontId::proportional(24.0),
            theme::TEXT,
        );
        ui.add_space(4.0);
        ui.label(RichText::new("Redline").family(theme::bold()).size(24.0).color(theme::TEXT));
    });
    ui.add_space(20.0);

    let height_id = ui.id().with("tools-card-height");
    let last_h = ui.data(|d| d.get_temp::<f32>(height_id)).unwrap_or(150.0);

    // Scrolls once every platform under Saved makes the list taller than the window.
    let active = app.jobs.active_count();
    egui::ScrollArea::vertical()
        .max_height((ui.available_height() - last_h - 8.0).max(120.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (page, icon, label, badge) in [
                (Page::Download, icons::DOWNLOAD_SIMPLE, "Download", None),
                (Page::Convert, icons::ARROWS_LEFT_RIGHT, "Convert", None),
                (Page::Saved(None), icons::BOOKMARKS_SIMPLE, "Saved", None),
                (Page::Playlists, icons::PLAYLIST, "Playlists", None),
                (Page::Queue, icons::QUEUE, "Queue", Some(active)),
                (Page::History, icons::CLOCK_COUNTER_CLOCKWISE, "History", None),
                (Page::Settings, icons::GEAR_SIX, "Settings", None),
            ] {
                if theme::nav_item(ui, icon, label, app.page == page, badge).clicked() {
                    app.page = page;
                    app.saved_page.open = None;
                }
                ui.add_space(2.0);
                if page == Page::Saved(None) {
                    saved_platforms(ui, app);
                }
            }
        });

    // Pin the tools card to the bottom using last frame's measured height.
    let tools = &app.tools;
    let rows = [
        ("yt-dlp", tools.ytdlp.is_some()),
        ("ffmpeg", tools.ffmpeg.is_some() && tools.ffprobe.is_some()),
        ("Deno", tools.deno.is_some()),
    ];
    let all_ok = rows.iter().all(|(_, ok)| *ok);
    ui.add_space((ui.available_height() - last_h).max(8.0));

    let frame_resp = egui::Frame::new()
        .fill(egui::Color32::from_gray(0x1A))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::eyebrow("Tools"));
            ui.add_space(2.0);
            for (name, ok) in rows {
                ui.horizontal(|ui| {
                    let (icon, color) = if ok {
                        (icons::CHECK_CIRCLE, theme::MUTED)
                    } else {
                        (icons::WARNING_CIRCLE, theme::accent().base)
                    };
                    ui.label(RichText::new(icon).size(16.0).color(color));
                    ui.label(RichText::new(name).color(if ok { theme::MUTED } else { theme::TEXT }));
                });
            }
            if !all_ok {
                ui.add_space(2.0);
                ui.label(
                    RichText::new(format!("Set up in Settings {}", icons::ARROW_RIGHT))
                        .small()
                        .color(theme::accent().hover),
                );
            }
        });
    let h = frame_resp.response.rect.height();
    if (h - last_h).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(height_id, h));
        ui.ctx().request_repaint();
    }
    if !all_ok
        && ui
            .interact(frame_resp.response.rect, ui.id().with("tools-card"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    {
        app.page = Page::Settings;
    }
}
