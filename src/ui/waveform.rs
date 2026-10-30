//! The optional waveform strip above the player bar: cached peaks, the A-B loop span and a
//! draggable playhead.

use eframe::egui::{self, Color32, Sense, Stroke, Ui, vec2};
use egui_phosphor::regular as icons;
use redline_core::player::Command;
use redline_core::waveform as peaks;

use crate::app::App;
use crate::task::Task;
use crate::theme;

#[derive(Default)]
pub struct Waveform {
    key: Option<String>,
    peaks: Option<Vec<f32>>,
    duration: Option<f64>,
    task: Option<Task<anyhow::Result<peaks::Waveform>>>,
    error: Option<String>,
    drag: Option<f64>,
}

/// Starts/finishes analyses for the current track. Called every frame; the strip only shows
/// when the setting is on.
pub fn poll(app: &mut App) {
    if let Some(result) = app.waveform.task.as_ref().and_then(Task::take) {
        app.waveform.task = None;
        match result {
            Ok(wave) => {
                app.waveform.peaks = Some(wave.peaks);
                app.waveform.duration = wave.duration;
                app.waveform.error = None;
            }
            Err(e) => app.waveform.error = Some(format!("{e:#}")),
        }
    }
    if !app.settings.player.waveform || !app.player_state.is_active() {
        app.waveform.key = None;
        app.waveform.peaks = None;
        app.waveform.duration = None;
        app.waveform.task = None;
        app.waveform.error = None;
        app.waveform.drag = None;
        return;
    }
    let Some(track) = app.now.current().cloned() else { return };
    let (source, key) = peaks::source_and_key(&track, &app.history);
    if app.waveform.key.as_deref() == Some(key.as_str()) {
        return;
    }
    app.waveform.key = Some(key.clone());
    app.waveform.peaks = None;
    app.waveform.duration = track.duration;
    app.waveform.error = None;
    let tools = app.tools.clone();
    let net = app.settings.network.clone();
    let duration = track.duration;
    app.waveform.task = Some(Task::spawn(&app.wake, move || {
        peaks::for_source(&tools, &net, &source, &key, duration)
    }));
}

pub fn show(ui: &mut Ui, app: &mut App) {
    let st = app.player_state.clone();
    let Some(total) = app.waveform.duration.or(st.duration).filter(|d| *d > 0.0) else {
        ui.label(theme::muted("Waiting for the track's length…"));
        return;
    };
    let Some(bars) = app.waveform.peaks.clone() else {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0).color(theme::accent().base));
            ui.label(theme::muted(if app.waveform.error.is_some() {
                "Couldn't read this waveform"
            } else {
                "Reading waveform…"
            }));
            if let Some(e) = &app.waveform.error {
                ui.add(egui::Label::new(egui::RichText::new(e).small().color(theme::SUBTLE)).truncate());
            }
        });
        return;
    };

    let response = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click_and_drag()).1;
    let rect = response.rect;
    let width = rect.width().max(1.0);
    let x_of = |t: f64| rect.left() + (t / total).clamp(0.0, 1.0) as f32 * width;
    let secs_at = |x: f32| (((x - rect.left()) / width) as f64 * total).clamp(0.0, total);
    if response.dragged() {
        if let Some(p) = response.interact_pointer_pos() {
            app.waveform.drag = Some(secs_at(p.x));
        }
    }
    if response.clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            let to = secs_at(p.x);
            app.player.send(Command::Seek(to));
            app.player_state.position = to;
        }
    }
    if let Some(to) = app.waveform.drag
        && !ui.input(|i| i.pointer.any_down())
    {
        app.player.send(Command::Seek(to));
        app.player_state.position = to;
        app.waveform.drag = None;
    }
    let position = app.waveform.drag.unwrap_or(st.position).clamp(0.0, total);
    let play_x = x_of(position);

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, theme::BG);
    let (a, b) = (st.ab_loop.a, st.ab_loop.b);
    if let (Some(a), Some(b)) = (a, b) {
        let (x1, x2) = (x_of(a), x_of(b));
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x1, rect.top()), egui::pos2(x2, rect.bottom())),
            0.0,
            theme::accent().base.gamma_multiply(0.16),
        );
    }
    let center = rect.center().y;
    let slots = ((width / 3.0) as usize).clamp(1, bars.len()).max(1);
    let step = width / slots as f32;
    let bar = (step * 0.55).max(1.0);
    for i in 0..slots {
        let from = i * bars.len() / slots;
        let to = ((i + 1) * bars.len() / slots).max(from + 1);
        let p = bars[from..to].iter().copied().fold(0.0f32, f32::max);
        let x = rect.left() + (i as f32 + 0.5) * step;
        let amp = (p * rect.height() * 0.42).max(0.5);
        let color = if x <= play_x { theme::accent().base } else { theme::SUBTLE };
        painter.line_segment(
            [egui::pos2(x, center - amp), egui::pos2(x, center + amp)],
            Stroke::new(bar, color),
        );
    }
    if let (Some(a), Some(b)) = (a, b) {
        for (t, label) in [(a, "A"), (b, "B")] {
            let x = x_of(t);
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                Stroke::new(1.5, theme::TEXT),
            );
            painter.text(
                egui::pos2(x + 3.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(10.0),
                theme::TEXT,
            );
        }
    }
    painter.line_segment(
        [
            egui::pos2(play_x, rect.top()),
            egui::pos2(play_x, rect.bottom()),
        ],
        Stroke::new(2.0, Color32::WHITE),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if app.waveform.error.is_some() {
        ui.label(theme::muted(format!("{} Cached waveform unavailable", icons::WARNING)));
    }
}
