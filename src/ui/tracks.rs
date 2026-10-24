//! The track table shared by Playlists and Saved: number, art, title and artist, source,
//! time and per-row buttons.

use eframe::egui::{self, Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, TextEdit, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icons;
use redline_core::library::Track;
use redline_core::linkcheck::LinkStatus;
use redline_core::platform::Platform;
use redline_core::player::Source;

use crate::format::fmt_duration;
use crate::theme;

pub enum RowAction {
    Play(usize),
    Up(usize),
    Down(usize),
    Remove(usize),
    Offline(usize),
    Save(usize),
    /// Open Convert with the row's file.
    Convert(usize),
    /// The platform glyph in the source column was clicked.
    OpenPlatform(Platform),
    /// Look for another upload of a link: a dead or restricted one's button, or any link's
    /// right-click menu.
    FindReplacement(usize),
}

/// Width of the per-row buttons, so the columns line up whatever buttons a row has.
const ACTIONS_W: f32 = 196.0;
const SOURCE_W: f32 = 110.0;
/// The warning glyph on links that are there but won't play for everyone.
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xD9, 0xA4, 0x41);

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
            ui.add_space(ACTIONS_W + 10.0);
            ui.add_sized([60.0, 16.0], egui::Label::new(theme::eyebrow("Time")));
            ui.add_sized([SOURCE_W, 16.0], egui::Label::new(theme::eyebrow("Source")));
        });
    });
    let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(sep, 0.0, egui::Color32::from_gray(0x2A));
    ui.add_space(4.0);
}

/// The Convert button for a row whose song is a file; `None` (no button) for streams.
pub fn convert_button(ui: &mut Ui, idx: usize, has_file: bool) -> Option<RowAction> {
    (has_file && theme::icon_button(ui, icons::ARROWS_LEFT_RIGHT, "Convert").clicked()).then_some(RowAction::Convert(idx))
}

/// The Find replacement button for a link that's dead or restricted; `None` (no button)
/// otherwise. It takes the place of Download for offline, which can't work for those.
pub fn replace_button(ui: &mut Ui, idx: usize, link: LinkStatus) -> Option<RowAction> {
    (link.label().is_some() && theme::icon_button(ui, icons::SWAP, "Find replacement").clicked())
        .then_some(RowAction::FindReplacement(idx))
}

/// How a row is drawn, apart from the track itself.
#[derive(Default)]
pub struct RowState<'a> {
    pub playing: bool,
    /// A link with a downloaded copy.
    pub offline: bool,
    /// Muted tag after the artist, like "Removed from YouTube".
    pub tag: Option<&'a str>,
    /// Show the link's platform glyph, which opens its Saved page.
    pub platform_link: bool,
    /// What the last link check found.
    pub link: LinkStatus,
    /// A right-click menu with Play and Find replacement… (links in lists that can take a
    /// replacement), whether or not the link is known to be broken.
    pub replace_menu: bool,
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
    let RowState {
        playing,
        offline,
        tag,
        platform_link,
        link,
        replace_menu,
    } = state;
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
    if replace_menu && t.is_link() {
        // Find replacement… comes first for a link known not to play, after Play otherwise.
        let flagged = link.label().is_some();
        resp.context_menu(|ui| {
            let mut pick = |ui: &mut Ui, label: String, a: RowAction| {
                if ui.button(label).clicked() {
                    action = Some(a);
                    ui.close();
                }
            };
            let find = format!("{}  Find replacement…", icons::SWAP);
            if flagged {
                pick(ui, find.clone(), RowAction::FindReplacement(idx));
            }
            pick(ui, format!("{}  Play", icons::PLAY), RowAction::Play(idx));
            if !flagged {
                pick(ui, find, RowAction::FindReplacement(idx));
            }
        });
    }
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
        ui.allocate_ui_with_layout(vec2(ACTIONS_W, 40.0), Layout::right_to_left(Align::Center), |ui| {
            ui.set_min_width(ACTIONS_W);
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
        ui.allocate_ui_with_layout(vec2(SOURCE_W, 20.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(SOURCE_W);
            ui.spacing_mut().item_spacing.x = 4.0;
            if platform_link && let Some(p) = t.platform() {
                let glyph = RichText::new(theme::platform_glyph(p)).size(14.0).color(theme::accent_for(p).base);
                let r = ui
                    .add(egui::Label::new(glyph).sense(Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(format!("Saved from {}", p.label()));
                if r.clicked() {
                    action = Some(RowAction::OpenPlatform(p));
                }
            }
            ui.add(egui::Label::new(RichText::new(format!("{src_icon} {src}")).small().color(theme::MUTED)))
                .on_hover_text(hover);
        });
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.vertical(|ui| {
                ui.add_space(2.0);
                let title = if playing {
                    RichText::new(&t.title).family(theme::bold()).color(theme::accent().hover)
                } else if tag.is_some() || link.is_dead() {
                    RichText::new(&t.title).family(theme::bold()).color(theme::MUTED)
                } else {
                    theme::strong(&t.title)
                };
                match link.label() {
                    None => {
                        ui.add(egui::Label::new(title).truncate());
                    }
                    Some(why) => {
                        let (glyph, color) = if link.is_dead() {
                            (icons::LINK_BREAK, theme::MUTED)
                        } else {
                            (icons::WARNING, AMBER)
                        };
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            ui.label(RichText::new(glyph).size(13.0).color(color)).on_hover_text(why);
                            ui.add(egui::Label::new(title).truncate()).on_hover_text(why);
                        });
                    }
                }
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
