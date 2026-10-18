use std::path::PathBuf;

use eframe::egui::{
    self, Align, ComboBox, CornerRadius, DragValue, Frame, Layout, Margin, RichText, Sense,
    Slider, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, vec2,
};
use egui_phosphor::regular as icons;

use super::presets::{PresetAction, PresetBar};
use crate::app::App;
use crate::presets::ConvertPreset;
use crate::theme;
use crate::tools::convert::{
    AUDIO_BITRATES, Category, ConvertOptions, GIF_DITHERS, GIF_FPS, GIF_SPEEDS, GIF_WIDTHS,
    GifDirection, MAX_HEIGHTS, PRESETS, render_stem,
};
use redline_core::convert_plan::{self, Sections};
use crate::tools::filters::{CropAspect, Rotation, SPEEDS, SubSource};
use crate::tools::hwaccel::{Encoders, Hardware, VideoCodec};

/// A file in the list: the core item plus whether its editor is open.
pub struct ConvertItem {
    pub file: convert_plan::ConvertItem,
    /// Per-file editor open.
    expanded: bool,
}

impl std::ops::Deref for ConvertItem {
    type Target = convert_plan::ConvertItem;
    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

impl std::ops::DerefMut for ConvertItem {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.file
    }
}

#[derive(Default)]
pub struct ConvertPage {
    pub items: Vec<ConvertItem>,
    skipped: Vec<String>,
    presets: PresetBar,
}

impl ConvertPage {
    /// Adds files, skipping duplicates and unsupported types.
    pub fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.skipped.clear();
        for path in paths {
            if self.items.iter().any(|i| i.path == path) {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match convert_plan::ConvertItem::from_path(path) {
                Some(file) => self.items.push(ConvertItem { file, expanded: false }),
                None => self.skipped.push(name),
            }
        }
    }

    /// The target all files would be joined into, if joining makes sense for this list.
    fn join_target(&self) -> Option<&str> {
        convert_plan::join_target(self.items.iter().map(|i| &i.file))
    }

    /// Takes the files out of the list, for queueing.
    fn take_files(&mut self) -> Vec<convert_plan::ConvertItem> {
        self.items.drain(..).map(|i| i.file).collect()
    }
}

const ALL_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff", "ico", "tga", "qoi", "mp4", "mkv",
    "webm", "mov", "avi", "m4v", "flv", "wmv", "mpg", "mpeg", "ts", "3gp", "ogv", "mp3", "m4a",
    "aac", "flac", "wav", "ogg", "opus", "wma", "aiff", "aif",
];

pub fn icon_for(category: Category) -> &'static str {
    match category {
        Category::Image => icons::IMAGE,
        Category::Video => icons::FILM_STRIP,
        Category::Audio => icons::MUSIC_NOTES,
    }
}

pub fn show(ui: &mut Ui, app: &mut App) {
    theme::page_header(
        ui,
        "Convert",
        "Change formats",
        "Turn videos, songs and images into the format you need. Drop files anywhere in this window.",
    );

    let has_av = app
        .convert
        .items
        .iter()
        .any(|i| i.category != Category::Image);
    if !app.tools.ready_for_convert() && (has_av || app.convert.items.is_empty()) {
        super::tools_banner(
            ui,
            app,
            "Video and audio conversion needs ffmpeg. Images work without it.",
        );
        ui.add_space(16.0);
    }

    drop_zone(ui, app);
    let page = &mut app.convert;

    if !page.skipped.is_empty() {
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!(
                "{} Skipped unsupported files: {}",
                icons::WARNING,
                page.skipped.join(", ")
            ))
            .color(theme::MUTED),
        );
    }

    if page.items.is_empty() {
        return;
    }
    ui.add_space(16.0);

    options_card(ui, app);
    ui.add_space(16.0);
    file_list(ui, app);
    ui.add_space(24.0);

    let n = app.convert.items.len();
    let blocked = app
        .convert
        .items
        .iter()
        .any(|i| i.category != Category::Image)
        && !app.tools.ready_for_convert();
    let join_target = app.convert.join_target().map(str::to_owned);
    ui.horizontal(|ui| {
        let label = format!(
            "{}  Convert {n} file{}",
            icons::ARROWS_LEFT_RIGHT,
            if n == 1 { "" } else { "s" }
        );
        if theme::pill_button(ui, &label, !blocked).clicked() {
            queue_conversions(app);
            app.page = crate::app::Page::Queue;
        }
        ui.add_space(4.0);
        if let Some(target) = &join_target {
            let r = theme::ghost_button(ui, &format!("{}  Join into one", icons::STACK), !blocked)
                .on_hover_text(format!(
                    "Play the files one after another as a single {}, in list order",
                    target.to_uppercase()
                ));
            if r.clicked() {
                queue_join(app, target);
                app.page = crate::app::Page::Queue;
            }
        }
        if theme::ghost_button(ui, "Clear all", true).clicked() {
            app.convert.items.clear();
        }
        if blocked {
            ui.label(theme::muted("Install ffmpeg to convert video and audio."));
        }
    });
}

fn queue_conversions(app: &mut App) {
    let files = app.convert.take_files();
    for job in convert_plan::requests(files, &app.settings, &app.encoders) {
        app.jobs.push(job.title, job.subtitle, job.spec);
    }
}

fn queue_join(app: &mut App, target: &str) {
    let files = app.convert.take_files();
    let job = convert_plan::join_request(files, target, &app.settings, &app.encoders);
    app.jobs.push(job.title, job.subtitle, job.spec);
}

fn drop_zone(ui: &mut Ui, app: &mut App) {
    let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let compact = !app.convert.items.is_empty();
    let height = if compact { 84.0 } else { 200.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(8),
        if hovering { theme::CARD_HOVER } else { theme::CARD },
    );
    painter.rect_stroke(
        rect.shrink(1.0),
        CornerRadius::same(8),
        Stroke::new(
            if hovering { 2.0 } else { 1.0 },
            if hovering { theme::accent().base } else { egui::Color32::from_gray(0x33) },
        ),
        StrokeKind::Inside,
    );

    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink(16.0))
            .layout(if compact {
                Layout::left_to_right(Align::Center)
            } else {
                Layout::top_down(Align::Center)
            }),
    );
    let ui = &mut child;
    if !compact {
        ui.add_space(22.0);
    }
    ui.label(
        RichText::new(icons::FILE_PLUS)
            .size(if compact { 28.0 } else { 44.0 })
            .color(if hovering { theme::accent().base } else { theme::MUTED }),
    );
    ui.label(theme::strong(if hovering {
        "Drop to add"
    } else if compact {
        "Drop more files, or"
    } else {
        "Drop images, videos or audio here"
    }));
    if !compact {
        ui.label(theme::muted("or"));
    }
    if theme::small_pill_button(ui, "Browse files", true).clicked()
        && let Some(files) = rfd::FileDialog::new()
            .add_filter("Media files", ALL_EXTS)
            .pick_files()
    {
        app.add_to_convert(files);
    }
}

fn options_card(ui: &mut Ui, app: &mut App) {
    let sec = Sections::of(app.convert.items.iter().filter(|i| i.options.is_none()).map(|i| &i.file));
    let customized = app.convert.items.iter().filter(|i| i.options.is_some()).count();
    let o = &mut app.settings.convert;
    let presets = &mut app.settings.convert_presets;
    theme::card(ui, |ui| {
        ui.label(theme::heading("Options"));
        ui.add_space(4.0);
        let names: Vec<String> = presets.iter().map(|p| p.name.clone()).collect();
        match app.convert.presets.show(ui, &names, presets.iter().position(|p| p.matches(o))) {
            Some(PresetAction::Apply(i)) => {
                presets[i].apply(o, app.convert.items.iter_mut().map(|i| &mut i.file));
            }
            Some(PresetAction::Delete(i)) => {
                presets.remove(i);
            }
            Some(PresetAction::Rename(i, name)) => presets[i].name = name,
            Some(PresetAction::SaveNew(name)) => {
                presets.push(ConvertPreset::from_items(name, o.clone(), app.convert.items.iter().map(|i| &i.file)));
            }
            None => {}
        }
        if customized > 0 {
            ui.label(theme::muted(if customized == 1 {
                "1 file uses its own settings".to_owned()
            } else {
                format!("{customized} files use their own settings")
            }));
        }
        ui.add_space(4.0);

        option_sections(ui, o, &sec, &app.encoders);

        ui.separator();
        ui.add_space(4.0);
        theme::option_row(ui, "Save to", |ui| {
            let mut custom = app.settings.convert_dir.is_some();
            if theme::chips(ui, &mut custom, &[(false, "Next to original"), (true, "Choose folder")]) {
                app.settings.convert_dir = if custom {
                    rfd::FileDialog::new().pick_folder()
                } else {
                    None
                };
            }
            if let Some(dir) = &app.settings.convert_dir {
                ui.add(egui::Label::new(theme::muted(dir.display().to_string())).truncate());
            }
        });
        theme::option_row(ui, "File name", |ui| {
            text_input(ui, &mut app.settings.rename_template, "{name}", 200.0);
            if let Some(first) = app.convert.items.first() {
                let example = render_stem(&app.settings.rename_template, &first.stem(), 1, &first.target);
                ui.label(
                    RichText::new(format!("→ {example}.{}", first.target))
                        .small()
                        .color(theme::MUTED),
                );
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(150.0);
            ui.label(
                RichText::new("{name} original name · {n} number in list · {date} today · {target} new format")
                    .small()
                    .color(theme::SUBTLE),
            );
        });
        ui.horizontal(|ui| {
            ui.add_space(150.0);
            theme::toggle(ui, &mut app.settings.recycle_originals, "Move originals to the Recycle Bin");
            ui.add_space(8.0);
            ui.label(RichText::new("Only after a successful conversion").small().color(theme::SUBTLE));
        });
    });
}

/// The option groups that apply to `sec`, editing `o`. Shared by the main card and the
/// per-file editors.
fn option_sections(ui: &mut Ui, o: &mut ConvertOptions, sec: &Sections, enc: &Encoders) {
    if sec.video {
        ui.label(theme::eyebrow("Video"));
        codec_rows(ui, o, enc, sec);
        theme::option_row(ui, "Size", |ui| {
            let mut by_size = o.target_size_mb > 0.0;
            if theme::chips(ui, &mut by_size, &[(false, "Set quality"), (true, "Fit a file size")]) {
                o.target_size_mb = if by_size { 10.0 } else { 0.0 };
            }
        });
        if o.target_size_mb > 0.0 {
            theme::option_row(ui, "Target size", |ui| {
                ui.add(DragValue::new(&mut o.target_size_mb).range(0.5..=10_000.0).speed(0.5).suffix(" MB"));
                for (mb, label) in [(8.0, "8"), (10.0, "10"), (25.0, "25"), (50.0, "50"), (100.0, "100")] {
                    if ui.selectable_label(o.target_size_mb == mb, format!("{label} MB")).clicked() {
                        o.target_size_mb = mb;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(150.0);
                ui.label(
                    RichText::new("Each video aims just under this size. Lower the resolution for long videos.")
                        .small()
                        .color(theme::SUBTLE),
                );
            });
        } else {
            theme::option_row(ui, "Quality", |ui| {
                // Slider runs "smaller file" → "better quality", the inverse of CRF.
                let mut q = 51 - o.crf as i32;
                ui.label(theme::muted("Smaller"));
                if ui.add(Slider::new(&mut q, 15..=37).show_value(false)).changed() {
                    o.crf = (51 - q) as u8;
                }
                ui.label(theme::muted("Better"));
                ui.label(RichText::new(format!("CRF {}", o.crf)).small().color(theme::SUBTLE));
            });
        }
        theme::option_row(ui, "Encode speed", |ui| {
            ComboBox::from_id_salt("cv-preset")
                .selected_text(&o.preset)
                .show_ui(ui, |ui| {
                    for p in PRESETS {
                        ui.selectable_value(&mut o.preset, p.to_string(), *p);
                    }
                });
        });
        theme::option_row(ui, "Max resolution", |ui| {
            let current = MAX_HEIGHTS
                .iter()
                .find(|(h, _)| *h == o.max_height)
                .map_or("Custom", |(_, l)| l);
            ComboBox::from_id_salt("cv-height")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (h, l) in MAX_HEIGHTS {
                        ui.selectable_value(&mut o.max_height, *h, *l);
                    }
                });
        });
        ui.add_space(6.0);
    }

    if sec.gif {
        gif_options(ui, &mut o.gif);
        ui.add_space(6.0);
    }

    if sec.audio {
        ui.label(theme::eyebrow("Audio"));
        theme::option_row(ui, "Bitrate", |ui| {
            ComboBox::from_id_salt("cv-bitrate")
                .selected_text(format!("{} kbps", o.audio_bitrate))
                .show_ui(ui, |ui| {
                    for b in AUDIO_BITRATES {
                        ui.selectable_value(&mut o.audio_bitrate, *b, format!("{b} kbps"));
                    }
                });
            ui.label(RichText::new("Ignored for FLAC and WAV").small().color(theme::SUBTLE));
        });
        ui.add_space(6.0);
    }

    if sec.image {
        ui.label(theme::eyebrow("Image"));
        theme::option_row(ui, "Quality", |ui| {
            ui.add(Slider::new(&mut o.image_quality, 1..=100));
            ui.label(RichText::new("JPG and AVIF · WebP is saved lossless").small().color(theme::SUBTLE));
        });
        theme::option_row(ui, "Fit within", |ui| {
            ui.add(DragValue::new(&mut o.resize_width).range(0..=16384).suffix(" px"));
            ui.label(theme::muted("×"));
            ui.add(DragValue::new(&mut o.resize_height).range(0..=16384).suffix(" px"));
            ui.label(RichText::new("0 keeps original size").small().color(theme::SUBTLE));
        });
        ui.add_space(6.0);
    }

    adjust_options(ui, o, sec);
}

/// Rotate, crop, speed and loudness.
fn adjust_options(ui: &mut Ui, o: &mut ConvertOptions, sec: &Sections) {
    let picture = sec.picture();
    let timed = sec.timed();
    if !picture && !timed {
        return;
    }
    let f = &mut o.filters;
    ui.horizontal(|ui| {
        ui.label(theme::eyebrow("Adjust"));
        if *f != Default::default()
            && ui
                .link(RichText::new("Reset").small().color(theme::SUBTLE))
                .clicked()
        {
            *f = Default::default();
        }
    });
    if picture {
        theme::option_row(ui, "Rotate", |ui| {
            theme::chips(
                ui,
                &mut f.rotate,
                &[
                    (Rotation::None, "None"),
                    (Rotation::Right, "90° right"),
                    (Rotation::Left, "90° left"),
                    (Rotation::Half, "180°"),
                ],
            );
            ui.add_space(8.0);
            theme::toggle(ui, &mut f.flip, "Mirror");
        });
        theme::option_row(ui, "Crop", |ui| {
            theme::chips(
                ui,
                &mut f.crop,
                &[
                    (CropAspect::None, "None"),
                    (CropAspect::Wide, "16:9"),
                    (CropAspect::Tall, "9:16"),
                    (CropAspect::Square, "1:1"),
                    (CropAspect::Portrait, "4:5"),
                ],
            );
            if f.crop != CropAspect::None {
                ui.label(RichText::new("Keeps the center").small().color(theme::SUBTLE));
            }
        });
    }
    if timed {
        theme::option_row(ui, "Speed", |ui| {
            let mut speed = f.speed;
            if theme::chips(ui, &mut speed, SPEEDS) {
                f.speed = speed;
            }
        });
        theme::option_row(ui, "Loudness", |ui| {
            theme::toggle(ui, &mut f.normalize, "Normalize");
            ui.add_space(12.0);
            ui.label(theme::muted("Volume"));
            ui.add(DragValue::new(&mut f.gain_db).range(-30.0..=30.0).speed(0.25).suffix(" dB"));
            ui.label(
                RichText::new("Normalize evens out quiet and loud files")
                    .small()
                    .color(theme::SUBTLE),
            );
        });
    }
    ui.add_space(6.0);
}

fn codec_rows(ui: &mut Ui, o: &mut ConvertOptions, enc: &Encoders, sec: &Sections) {
    theme::option_row(ui, "Codec", |ui| {
        let opts: Vec<(VideoCodec, &str)> = sec.codecs().into_iter().map(|c| (c, c.label())).collect();
        o.codec = sec.fit_codec(o.codec);
        theme::chips(ui, &mut o.codec, &opts);
        ui.label(RichText::new(convert_plan::codec_hint(o.codec)).small().color(theme::SUBTLE));
    });
    for note in sec.codec_notes(o.codec) {
        ui.horizontal(|ui| {
            ui.add_space(150.0);
            ui.label(RichText::new(note).small().color(theme::SUBTLE));
        });
    }

    theme::option_row(ui, "Encoder", |ui| {
        let opts: Vec<(Hardware, &str)> = convert_plan::encoder_choices(enc, o.hardware)
            .into_iter()
            .map(|h| (h, h.label()))
            .collect();
        theme::chips(ui, &mut o.hardware, &opts);
        ui.label(RichText::new(convert_plan::encoder_note(enc, o)).small().color(theme::SUBTLE));
    });
}

fn gif_options(ui: &mut Ui, g: &mut crate::tools::convert::GifOptions) {
    ui.horizontal(|ui| {
        ui.label(theme::eyebrow("GIF"));
        if ui
            .link(RichText::new("Reset").small().color(theme::SUBTLE))
            .clicked()
        {
            *g = Default::default();
        }
    });
    theme::option_row(ui, "Clip", |ui| {
        ui.label(theme::muted("Start"));
        ui.add(time_drag(&mut g.start, "0:00"));
        ui.add_space(8.0);
        ui.label(theme::muted("Length"));
        ui.add(time_drag(&mut g.length, "whole video"));
        ui.label(RichText::new("Type 1:30 or seconds").small().color(theme::SUBTLE));
    });
    theme::option_row(ui, "Width", |ui| {
        let current = GIF_WIDTHS
            .iter()
            .find(|(w, _)| *w == g.width)
            .map_or_else(|| format!("{} px", g.width), |(_, l)| l.to_string());
        ComboBox::from_id_salt("gif-width")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (w, l) in GIF_WIDTHS {
                    ui.selectable_value(&mut g.width, *w, *l);
                }
            });
        ui.add(DragValue::new(&mut g.width).range(0..=3840).suffix(" px"));
        ui.label(RichText::new("Height follows the aspect ratio").small().color(theme::SUBTLE));
    });
    theme::option_row(ui, "Frame rate", |ui| {
        ComboBox::from_id_salt("gif-fps")
            .selected_text(format!("{} fps", g.fps))
            .show_ui(ui, |ui| {
                for f in GIF_FPS {
                    ui.selectable_value(&mut g.fps, *f, format!("{f} fps"));
                }
            });
        ui.label(RichText::new("Lower is smaller").small().color(theme::SUBTLE));
    });
    theme::option_row(ui, "Colors", |ui| {
        ui.add(Slider::new(&mut g.colors, 2..=256).logarithmic(true));
        ui.label(RichText::new("Fewer colors, smaller file").small().color(theme::SUBTLE));
    });
    theme::option_row(ui, "Dithering", |ui| {
        let opts: Vec<(String, &str)> = GIF_DITHERS.iter().map(|(v, l)| (v.to_string(), *l)).collect();
        theme::chips(ui, &mut g.dither, &opts);
    });
    theme::option_row(ui, "Speed", |ui| {
        let mut speed = g.speed;
        if theme::chips(ui, &mut speed, GIF_SPEEDS) {
            g.speed = speed;
        }
    });
    theme::option_row(ui, "Playback", |ui| {
        theme::chips(
            ui,
            &mut g.direction,
            &[
                (GifDirection::Forward, "Forward"),
                (GifDirection::Reverse, "Reverse"),
                (GifDirection::Boomerang, "Boomerang"),
            ],
        );
    });
    theme::option_row(ui, "Loop", |ui| {
        theme::chips(ui, &mut g.plays, &[(0, "Forever"), (1, "Once"), (2, "Twice"), (3, "3 times")]);
    });
    ui.horizontal(|ui| {
        ui.add_space(150.0);
        theme::toggle(ui, &mut g.diff_only, "Only redraw what changes");
        ui.add_space(8.0);
        ui.label(
            RichText::new("Smaller files for clips with a still background")
                .small()
                .color(theme::SUBTLE),
        );
    });
    if g.direction != GifDirection::Forward && g.length == 0.0 {
        ui.label(
            RichText::new(format!(
                "{} Reverse and Boomerang hold the whole clip in memory. Set a length for long videos.",
                icons::INFO
            ))
            .small()
            .color(theme::MUTED),
        );
    }
}

/// Drag-or-type field for a time in seconds, shown as m:ss. `zero` labels the 0 value.
fn time_drag<'a>(secs: &'a mut f64, zero: &'a str) -> DragValue<'a> {
    DragValue::new(secs)
        .range(0.0..=86_400.0)
        .speed(0.25)
        .custom_formatter(move |v, _| {
            if v <= 0.0 {
                zero.to_owned()
            } else if v.fract().abs() < 0.05 {
                super::fmt_duration(v)
            } else {
                format!("{}.{}", super::fmt_duration(v.trunc()), (v.fract() * 10.0).round() as u32 % 10)
            }
        })
        .custom_parser(|text| match text.trim() {
            "" => Some(0.0),
            t if t.chars().any(char::is_alphabetic) => Some(0.0),
            t => super::parse_duration(t),
        })
}

enum RowAction {
    Remove(usize),
    Up(usize),
    Down(usize),
}

fn file_list(ui: &mut Ui, app: &mut App) {
    let page = &mut app.convert;

    // Bulk "set all" controls, one per category present.
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::heading(format!("Files · {}", page.items.len())));
        ui.add_space(16.0);
        for cat in [Category::Video, Category::Audio, Category::Image] {
            if !page.items.iter().any(|i| i.category == cat) {
                continue;
            }
            ui.label(theme::muted(format!("All {}s {}", cat.label().to_lowercase(), icons::ARROW_RIGHT)));
            ComboBox::from_id_salt(("bulk", cat.label()))
                .selected_text("set…")
                .width(80.0)
                .show_ui(ui, |ui| {
                    for t in cat.targets() {
                        if ui.selectable_label(false, t.to_uppercase()).clicked() {
                            for item in page.items.iter_mut().filter(|i| i.category == cat) {
                                item.target = t.to_string();
                            }
                        }
                    }
                });
            ui.add_space(8.0);
        }
        if page.items.iter().any(|i| i.category == Category::Video)
            && theme::ghost_button(ui, &format!("{}  Extract audio", icons::MUSIC_NOTES), true)
                .on_hover_text("Convert every video to MP3")
                .clicked()
        {
            for item in page.items.iter_mut().filter(|i| i.category == Category::Video) {
                item.target = "mp3".into();
            }
        }
    });
    ui.add_space(6.0);

    // Column header, like a playlist track list.
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.add_sized([28.0, 16.0], egui::Label::new(theme::eyebrow("#")));
        ui.add_space(52.0);
        ui.label(theme::eyebrow("File"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(140.0);
            ui.add_sized([110.0, 16.0], egui::Label::new(theme::eyebrow("Convert to")));
            ui.add_sized([70.0, 16.0], egui::Label::new(theme::eyebrow("Type")));
        });
    });
    let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter()
        .rect_filled(sep, 0.0, egui::Color32::from_gray(0x2A));
    ui.add_space(4.0);

    let count = page.items.len();
    let mut action = None;
    for idx in 0..count {
        let item = &mut page.items[idx];
        let (rect, _) = ui.allocate_exact_size(vec2(w, 56.0), Sense::hover());
        let hovered = ui.rect_contains_pointer(rect);
        if hovered || item.expanded {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(4), theme::CARD_HOVER);
        }
        let mut row = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(12.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        row.add_sized(
            [28.0, 20.0],
            egui::Label::new(theme::muted((idx + 1).to_string())),
        );
        row.add_space(4.0);
        theme::art_tile(&mut row, 40.0, icon_for(item.category), theme::MUTED);
        row.add_space(8.0);
        row.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(ui, icons::X, "Remove").clicked() {
                action = Some(RowAction::Remove(idx));
            }
            let tune = if item.is_customized() {
                RichText::new(icons::SLIDERS_HORIZONTAL).color(theme::accent().base)
            } else {
                RichText::new(icons::SLIDERS_HORIZONTAL)
            };
            if ui
                .add(egui::Button::new(tune.size(18.0)).frame(false))
                .on_hover_text("Settings for this file")
                .clicked()
            {
                item.expanded = !item.expanded;
            }
            if idx + 1 < count && theme::icon_button(ui, icons::CARET_DOWN, "Move down").clicked() {
                action = Some(RowAction::Down(idx));
            }
            if idx > 0 && theme::icon_button(ui, icons::CARET_UP, "Move up").clicked() {
                action = Some(RowAction::Up(idx));
            }
            ui.add_space(8.0);
            ComboBox::from_id_salt(("target", idx, &item.path))
                .selected_text(item.target.to_uppercase())
                .width(96.0)
                .show_ui(ui, |ui| {
                    for t in item.category.targets() {
                        ui.selectable_value(&mut item.target, t.to_string(), t.to_uppercase());
                    }
                });
            ui.add_space(10.0);
            ui.add_sized(
                [70.0, 20.0],
                egui::Label::new(theme::muted(item.category.label())),
            );
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(theme::strong(item.name())).truncate())
                        .on_hover_text(item.path.display().to_string());
                    let mut meta = super::fmt_bytes(item.size);
                    if item.options.is_some() {
                        meta.push_str(" · custom settings");
                    }
                    if item.burn_subs.is_some() && item.makes_video() {
                        meta.push_str(" · subtitles burned in");
                    }
                    ui.label(RichText::new(meta).small().color(theme::MUTED));
                });
            });
        });

        if item.expanded {
            item_editor(ui, idx, item, &app.settings.convert, &app.encoders);
            ui.add_space(6.0);
        }
    }
    match action {
        Some(RowAction::Remove(i)) => {
            page.items.remove(i);
        }
        Some(RowAction::Up(i)) => page.items.swap(i, i - 1),
        Some(RowAction::Down(i)) => page.items.swap(i, i + 1),
        None => {}
    }
}

/// Inline editor under a file row: its own settings and subtitle burn-in.
fn item_editor(ui: &mut Ui, idx: usize, item: &mut ConvertItem, shared: &ConvertOptions, enc: &Encoders) {
    Frame::new()
        .fill(theme::ELEVATED)
        .corner_radius(6)
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.push_id(("item-editor", idx), |ui| {
                let mut custom = item.options.is_some();
                ui.horizontal(|ui| {
                    if theme::toggle(ui, &mut custom, "Use its own settings").changed() {
                        item.options = custom.then(|| shared.clone());
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(if custom {
                            "Changes here only affect this file"
                        } else {
                            "Follows the options above"
                        })
                        .small()
                        .color(theme::SUBTLE),
                    );
                });
                let sec = Sections::of(std::iter::once(&item.file));
                if let Some(o) = &mut item.options {
                    ui.add_space(6.0);
                    option_sections(ui, o, &sec, enc);
                }
                if item.makes_video() {
                    ui.add_space(6.0);
                    ui.label(theme::eyebrow("Subtitles"));
                    theme::option_row(ui, "Burn in", |ui| {
                        let mut mode = match &item.burn_subs {
                            None => 0,
                            Some(SubSource::Embedded) => 1,
                            Some(SubSource::File(_)) => 2,
                        };
                        if theme::chips(ui, &mut mode, &[(0, "Off"), (1, "Built-in track"), (2, "From .srt file")]) {
                            item.burn_subs = match mode {
                                1 => Some(SubSource::Embedded),
                                2 => rfd::FileDialog::new()
                                    .add_filter("Subtitles", &["srt", "ass", "ssa", "vtt"])
                                    .pick_file()
                                    .map(SubSource::File)
                                    .or(item.burn_subs.take()),
                                _ => None,
                            };
                        }
                        if let Some(SubSource::File(f)) = &item.burn_subs {
                            ui.add(egui::Label::new(theme::muted(f.display().to_string())).truncate());
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(150.0);
                        ui.label(
                            RichText::new("Draws the subtitles into the picture, so they show in every player")
                                .small()
                                .color(theme::SUBTLE),
                        );
                    });
                }
            });
        });
}

fn text_input(ui: &mut Ui, text: &mut String, hint: &str, width: f32) {
    Frame::new()
        .fill(theme::INPUT)
        .corner_radius(6)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.add(
                TextEdit::singleline(text)
                    .frame(Frame::NONE)
                    .hint_text(hint)
                    .desired_width(width),
            );
        });
}
