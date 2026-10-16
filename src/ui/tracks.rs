//! The track table shared by Playlists and Saved: number, art, title and artist, source,
//! time and per-row buttons.

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, TextEdit, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icons;
use redline_core::library::Track;
use redline_core::player::Source;

use crate::format::fmt_duration;
use crate::theme;

pub enum RowAction {
    Play(usize),
    Up(usize),
    Down(usize),
    Remove(usize),
    Offline(usize),
}

pub fn search_box(ui: &mut Ui, text: &mut String, hint: &str) {
    Frame::new()
        .fill(theme::INPUT)
        .corner_radius(255)
        .inner_margin(Margin::symmetric(16, 8))
        .show(ui, |ui| {
            ui.set_width(300.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::MAGNIFYING_GLASS).size(16.0).color(theme::MUTED));
                ui.add(
                    TextEdit::singleline(text)
                        .frame(Frame::NONE)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                );
            });
        });
}

pub fn list_header(ui: &mut Ui) {
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.add_sized([28.0, 16.0], egui::Label::new(theme::eyebrow("#")));
        ui.add_space(52.0);
        ui.label(theme::eyebrow("Title"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(170.0);
            ui.add_sized([60.0, 16.0], egui::Label::new(theme::eyebrow("Time")));
            ui.add_sized([96.0, 16.0], egui::Label::new(theme::eyebrow("Source")));
        });
    });
    let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(sep, 0.0, egui::Color32::from_gray(0x2A));
    ui.add_space(4.0);
}

/// How a row is drawn, apart from the track itself.
#[derive(Default)]
pub struct RowState<'a> {
    pub playing: bool,
    /// A link with a downloaded copy.
    pub offline: bool,
    /// Muted tag after the artist, like "Removed from YouTube".
    pub tag: Option<&'a str>,
}

/// A playlist-style row. Double-click asks to play it; `actions` draws the buttons on the
/// right, right to left.
pub fn track_row(
    ui: &mut Ui,
    idx: usize,
    t: &Track,
    state: RowState,
    actions: impl FnOnce(&mut Ui) -> Option<RowAction>,
) -> Option<RowAction> {
    let RowState { playing, offline, tag } = state;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 56.0), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    if hovered || playing {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(4),
            if playing { theme::ELEVATED } else { theme::CARD_HOVER },
        );
    }
    let mut action = resp.double_clicked().then_some(RowAction::Play(idx));
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(12.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let number = if playing {
        RichText::new(icons::SPEAKER_HIGH).color(theme::accent().base)
    } else {
        theme::muted((idx + 1).to_string())
    };
    row.add_sized([28.0, 20.0], egui::Label::new(number));
    row.add_space(4.0);
    let icon = if t.is_link() { icons::LINK } else { icons::MUSIC_NOTES };
    theme::cover(&mut row, 40.0, t.thumbnail.as_deref(), icon);
    row.add_space(8.0);
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.allocate_ui_with_layout(vec2(160.0, 40.0), Layout::right_to_left(Align::Center), |ui| {
            // Fixed width, so the columns line up whatever buttons a row has.
            ui.set_min_width(160.0);
            if let Some(a) = actions(ui) {
                action = Some(a);
            }
        });
        ui.add_space(10.0);
        let time = t.duration.map_or("".to_owned(), fmt_duration);
        ui.add_sized([60.0, 20.0], egui::Label::new(theme::muted(time)));
        let (src_icon, src) = match (&t.source, offline) {
            (Source::Link(_), true) => (icons::CHECK_CIRCLE, "Downloaded"),
            (Source::Link(_), false) => (icons::GLOBE, "Stream"),
            (Source::Local(_), _) => (icons::HARD_DRIVE, "On this PC"),
        };
        let hover = match &t.source {
            Source::Link(u) => u.clone(),
            Source::Local(p) => p.display().to_string(),
        };
        ui.add_sized(
            [96.0, 20.0],
            egui::Label::new(RichText::new(format!("{src_icon} {src}")).small().color(theme::MUTED)),
        )
        .on_hover_text(hover);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.vertical(|ui| {
                ui.add_space(2.0);
                let title = if playing {
                    RichText::new(&t.title).family(theme::bold()).color(theme::accent().hover)
                } else if tag.is_some() {
                    RichText::new(&t.title).family(theme::bold()).color(theme::MUTED)
                } else {
                    theme::strong(&t.title)
                };
                ui.add(egui::Label::new(title).truncate());
                let artist = RichText::new(t.artist.clone().unwrap_or_default()).small().color(theme::MUTED);
                match tag {
                    None => {
                        ui.add(egui::Label::new(artist).truncate());
                    }
                    Some(tag) => {
                        // Keep the line as short as a plain label so the row doesn't grow.
                        ui.spacing_mut().interact_size.y = 14.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            Frame::new()
                                .fill(egui::Color32::from_gray(0x2A))
                                .corner_radius(4)
                                .inner_margin(Margin::symmetric(5, 0))
                                .show(ui, |ui| ui.label(RichText::new(tag).size(11.0).color(theme::MUTED)));
                            ui.add(egui::Label::new(artist).truncate());
                        });
                    }
                }
            });
        });
    });
    action
}
