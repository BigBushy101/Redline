//! A row of named presets: click to apply, right-click to rename or delete, "+" to save.

use eframe::egui::{self, Frame, Key, Margin, RichText, TextEdit, Ui};
use egui_phosphor::regular as icons;

use crate::theme;

pub enum PresetAction {
    Apply(usize),
    Delete(usize),
    Rename(usize, String),
    SaveNew(String),
}

/// Inline name editing for the preset row.
#[derive(Default)]
pub struct PresetBar {
    /// Name typed for a new preset.
    naming: Option<String>,
    /// Preset being renamed and its new name.
    renaming: Option<(usize, String)>,
}

impl PresetBar {
    /// Draws the row. `active` is the preset matching the current settings, if any.
    pub fn show(&mut self, ui: &mut Ui, names: &[String], active: Option<usize>) -> Option<PresetAction> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(theme::eyebrow("Presets"));
            ui.add_space(4.0);
            for (i, name) in names.iter().enumerate() {
                if let Some((idx, text)) = &mut self.renaming
                    && *idx == i
                {
                    let done = name_field(ui, text, ("preset-rename", i));
                    match done {
                        Some(true) if !text.trim().is_empty() => {
                            action = Some(PresetAction::Rename(i, text.trim().to_owned()));
                            self.renaming = None;
                        }
                        Some(_) => self.renaming = None,
                        None => {}
                    }
                    continue;
                }
                let r = ui
                    .selectable_label(active == Some(i), name)
                    .on_hover_text("Click to apply · right-click to rename or delete");
                if r.clicked() {
                    action = Some(PresetAction::Apply(i));
                }
                r.context_menu(|ui| {
                    if ui.button(format!("{}  Rename", icons::PENCIL_SIMPLE)).clicked() {
                        self.renaming = Some((i, name.clone()));
                        ui.close();
                    }
                    if ui.button(format!("{}  Delete", icons::TRASH)).clicked() {
                        action = Some(PresetAction::Delete(i));
                        ui.close();
                    }
                });
            }
            match &mut self.naming {
                Some(text) => match name_field(ui, text, "preset-new") {
                    Some(true) if !text.trim().is_empty() => {
                        action = Some(PresetAction::SaveNew(text.trim().to_owned()));
                        self.naming = None;
                    }
                    Some(_) => self.naming = None,
                    None => {}
                },
                None => {
                    if ui
                        .link(RichText::new(format!("{} Save current", icons::PLUS)).color(theme::MUTED))
                        .on_hover_text("Save these settings as a preset")
                        .clicked()
                    {
                        self.naming = Some(String::new());
                    }
                }
            }
        });
        action
    }
}

/// Small text field; `Some(true)` on Enter/Save, `Some(false)` on Escape/✕.
fn name_field(ui: &mut Ui, text: &mut String, id: impl std::hash::Hash + std::fmt::Debug) -> Option<bool> {
    let mut result = None;
    Frame::new()
        .fill(theme::INPUT)
        .corner_radius(6)
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            let r = ui.add(
                TextEdit::singleline(text)
                    .id_salt(id)
                    .frame(Frame::NONE)
                    .hint_text("Preset name")
                    .desired_width(140.0),
            );
            if !r.has_focus() && !r.lost_focus() && text.is_empty() {
                r.request_focus();
            }
            // Losing focus alone isn't an answer: it also happens on the way to clicking Save.
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                result = Some(true);
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                result = Some(false);
            }
        });
    if ui.small_button("Save").clicked() {
        result = Some(true);
    }
    if ui
        .add(egui::Button::new(RichText::new(icons::X).color(theme::MUTED)).frame(false))
        .clicked()
    {
        result = Some(false);
    }
    result
}
