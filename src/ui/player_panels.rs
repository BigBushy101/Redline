//! Speed and equalizer popups that open above the player bar.

use eframe::egui::{self, Align, Align2, Layout, RichText, Slider, Ui, vec2};
use redline_core::player::dsp::{
    BAND_DB, EQ_FREQS, EQ_PRESETS, EqSettings, MAX_SPEED, MIN_SPEED, PREAMP_MAX_DB, PREAMP_MIN_DB,
};

use crate::app::App;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Speed,
    Eq,
}

const SPEEDS: &[(f32, &str)] = &[(0.75, "0.75×"), (1.0, "1×"), (1.25, "1.25×"), (1.5, "1.5×"), (2.0, "2×")];

/// "1.25×", trimmed.
pub fn speed_label(speed: f32) -> String {
    let s = format!("{speed:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}×")
}

pub fn show(ctx: &egui::Context, app: &mut App) {
    let Some(panel) = app.player_panel else { return };
    if !app.player_state.is_active() {
        app.player_panel = None;
        return;
    }
    let title = match panel {
        Panel::Speed => "Speed",
        Panel::Eq => "Equalizer",
    };
    let mut open = true;
    egui::Window::new(title)
        .id(egui::Id::new("player-panel"))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -96.0))
        .frame(
            egui::Frame::new()
                .fill(theme::ELEVATED)
                .corner_radius(8)
                .inner_margin(egui::Margin::same(18))
                .shadow(ctx.global_style().visuals.window_shadow),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::heading(title));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(ui, egui_phosphor::regular::X, "Close").clicked() {
                        open = false;
                    }
                });
            });
            ui.add_space(6.0);
            match panel {
                Panel::Speed => speed(ui, app),
                Panel::Eq => equalizer(ui, &mut app.settings.player.eq),
            }
        });
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.player_panel = None;
    }
}

fn speed(ui: &mut Ui, app: &mut App) {
    ui.set_width(340.0);
    let p = &mut app.settings.player;
    theme::chips(ui, &mut p.speed, SPEEDS);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = 240.0;
        let r = ui.add(
            Slider::new(&mut p.speed, MIN_SPEED..=MAX_SPEED)
                .step_by(0.05)
                .show_value(false),
        );
        if r.double_clicked() {
            p.speed = 1.0;
        }
        r.on_hover_text("Double-click for 1×");
        ui.label(theme::strong(speed_label(p.speed)));
    });
    ui.add_space(4.0);
    theme::toggle(ui, &mut p.reset_speed, "Back to 1× for each new track");
    ui.add_space(4.0);
    ui.label(
        RichText::new("Pitch rises and falls with speed, like a record player.")
            .small()
            .color(theme::SUBTLE),
    );
}

fn band_label(freq: f32) -> String {
    if freq >= 1000.0 {
        format!("{}k", freq / 1000.0)
    } else {
        format!("{freq}")
    }
}

fn db_label(db: f32) -> String {
    if db.abs() < 0.05 {
        "0".to_owned()
    } else {
        format!("{db:+.1}")
    }
}

fn equalizer(ui: &mut Ui, eq: &mut EqSettings) {
    ui.horizontal(|ui| {
        theme::toggle(ui, &mut eq.enabled, "On");
        ui.add_space(12.0);
        let mut current = eq.preset();
        let options: Vec<(Option<&str>, &str)> = EQ_PRESETS.iter().map(|(n, ..)| (Some(*n), *n)).collect();
        if theme::chips(ui, &mut current, &options)
            && let Some((_, pre, bands)) = EQ_PRESETS.iter().find(|(n, ..)| Some(*n) == current)
        {
            eq.preamp_db = *pre;
            eq.bands_db = *bands;
            eq.enabled = true;
        }
    });
    ui.add_space(10.0);

    let dim = !eq.enabled;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.spacing_mut().slider_width = 150.0;
        let column = |ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str| {
            ui.allocate_ui_with_layout(vec2(38.0, 200.0), Layout::top_down(Align::Center), |ui| {
                let color = if dim { theme::SUBTLE } else { theme::TEXT };
                ui.label(RichText::new(db_label(*value)).small().color(color));
                let r = ui.add(Slider::new(value, range).vertical().step_by(0.5).show_value(false));
                if r.double_clicked() {
                    *value = 0.0;
                }
                r.on_hover_text("Double-click for 0 dB");
                ui.label(RichText::new(label).small().color(theme::MUTED));
            });
        };
        column(ui, &mut eq.preamp_db, PREAMP_MIN_DB..=PREAMP_MAX_DB, "Pre");
        ui.separator();
        for (band, freq) in eq.bands_db.iter_mut().zip(EQ_FREQS) {
            column(ui, band, -BAND_DB..=BAND_DB, &band_label(freq));
        }
    });
    if dim {
        ui.label(RichText::new("Turned off: you hear the track as it is.").small().color(theme::SUBTLE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(speed_label(1.0), "1×");
        assert_eq!(speed_label(1.25), "1.25×");
        assert_eq!(speed_label(0.5), "0.5×");
        assert_eq!(band_label(16000.0), "16k");
        assert_eq!(band_label(62.0), "62");
        assert_eq!(db_label(-3.0), "-3.0");
        assert_eq!(db_label(0.01), "0");
    }
}
