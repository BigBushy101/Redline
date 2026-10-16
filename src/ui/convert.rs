use std::path::PathBuf;

use eframe::egui::{
    self, Align, ComboBox, CornerRadius, DragValue, Layout, RichText, Sense, Slider, Stroke,
    StrokeKind, Ui, UiBuilder, vec2,
};
use egui_phosphor::regular as icons;

use crate::app::App;
use crate::jobs::JobSpec;
use crate::theme;
use crate::tools::convert::{
    AUDIO_BITRATES, Category, ConvertRequest, GIF_DITHERS, GIF_FPS, GIF_SPEEDS, GIF_WIDTHS,
    GifDirection, MAX_HEIGHTS, PRESETS, classify,
};
use crate::tools::ffmpeg::is_audio_target;
use crate::tools::hwaccel::{self, Encoders, Hardware, VideoCodec};

pub struct ConvertItem {
    pub path: PathBuf,
    pub category: Category,
    pub size: u64,
    pub target: String,
}

#[derive(Default)]
pub struct ConvertPage {
    pub items: Vec<ConvertItem>,
    skipped: Vec<String>,
}

impl ConvertPage {
    /// Adds files, skipping duplicates and unsupported types.
    pub fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.skipped.clear();
        for path in paths {
            if self.items.iter().any(|i| i.path == path) {
                continue;
            }
            match classify(&path) {
                Some(category) if path.is_file() => {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    self.items.push(ConvertItem {
                        path,
                        category,
                        size,
                        target: category.default_target().to_owned(),
                    });
                }
                _ => self.skipped.push(
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            }
        }
    }
}

/// Which option groups apply to the files in the list.
struct Sections {
    video: bool,
    audio: bool,
    gif: bool,
    image: bool,
}

impl Sections {
    fn of(items: &[ConvertItem]) -> Self {
        let has = |f: &dyn Fn(&ConvertItem) -> bool| items.iter().any(f);
        let is_video = |i: &ConvertItem| i.category == Category::Video;
        Self {
            video: has(&|i| is_video(i) && i.target != "gif" && !is_audio_target(&i.target)),
            audio: has(&|i| i.category == Category::Audio || is_video(i) && i.target != "gif"),
            gif: has(&|i| is_video(i) && i.target == "gif"),
            image: has(&|i| i.category == Category::Image),
        }
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

    let page = &mut app.convert;
    let n = page.items.len();
    let blocked = page
        .items
        .iter()
        .any(|i| i.category != Category::Image)
        && !app.tools.ready_for_convert();
    ui.horizontal(|ui| {
        let label = format!(
            "{}  Convert {n} file{}",
            icons::ARROWS_LEFT_RIGHT,
            if n == 1 { "" } else { "s" }
        );
        if theme::pill_button(ui, &label, !blocked).clicked() {
            for item in page.items.drain(..) {
                let name = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let from = item
                    .path
                    .extension()
                    .map(|e| e.to_string_lossy().to_uppercase())
                    .unwrap_or_default();
                app.jobs.push(
                    name,
                    format!("{from} {} {}", icons::ARROW_RIGHT, item.target.to_uppercase()),
                    JobSpec::Convert(ConvertRequest {
                        options: job_options(&app.settings.convert, &item.target, &app.encoders),
                        input: item.path,
                        category: item.category,
                        target: item.target,
                        out_dir: app.settings.convert_dir.clone(),
                    }),
                );
            }
            app.page = crate::app::Page::Queue;
        }
        ui.add_space(4.0);
        if theme::ghost_button(ui, "Clear all", true).clicked() {
            page.items.clear();
        }
        if blocked {
            ui.label(theme::muted("Install ffmpeg to convert video and audio."));
        }
    });
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
            if hovering { theme::ACCENT } else { egui::Color32::from_gray(0x33) },
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
            .color(if hovering { theme::ACCENT } else { theme::MUTED }),
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
        app.convert.add_paths(files);
    }
}

fn options_card(ui: &mut Ui, app: &mut App) {
    let sec = Sections::of(&app.convert.items);
    let o = &mut app.settings.convert;
    theme::card(ui, |ui| {
        ui.label(theme::heading("Options"));
        ui.add_space(4.0);

        if sec.video {
            ui.label(theme::eyebrow("Video"));
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
            codec_rows(ui, o, &app.encoders, &app.convert.items);
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
            ui.add_space(52.0);
            ui.add_sized([110.0, 16.0], egui::Label::new(theme::eyebrow("Convert to")));
            ui.add_sized([70.0, 16.0], egui::Label::new(theme::eyebrow("Type")));
        });
    });
    let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter()
        .rect_filled(sep, 0.0, egui::Color32::from_gray(0x2A));
    ui.add_space(4.0);

    let mut remove = None;
    for (idx, item) in page.items.iter_mut().enumerate() {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 56.0), Sense::hover());
        let hovered = ui.rect_contains_pointer(rect);
        if hovered {
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
                remove = Some(idx);
            }
            ui.add_space(12.0);
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
                    let name = item
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.add(egui::Label::new(theme::strong(name)).truncate())
                        .on_hover_text(item.path.display().to_string());
                    ui.label(
                        RichText::new(super::fmt_bytes(item.size))
                            .small()
                            .color(theme::MUTED),
                    );
                });
            });
        });
    }
    if let Some(i) = remove {
        page.items.remove(i);
    }
}

/// Options as sent with a job: the GPU falls back to CPU if it can't encode this codec.
fn job_options(o: &crate::tools::convert::ConvertOptions, target: &str, enc: &Encoders) -> crate::tools::convert::ConvertOptions {
    let mut o = o.clone();
    o.hardware = enc.resolve(hwaccel::effective_codec(target, o.codec), o.hardware);
    o
}

fn codec_rows(
    ui: &mut Ui,
    o: &mut crate::tools::convert::ConvertOptions,
    enc: &Encoders,
    items: &[ConvertItem],
) {
    let video_targets: Vec<&str> = items
        .iter()
        .filter(|i| i.category == Category::Video)
        .map(|i| i.target.as_str())
        .filter(|t| !hwaccel::codecs_for(t).is_empty())
        .collect();
    let has_webm = video_targets.contains(&"webm");
    let has_mp4ish = video_targets.iter().any(|t| *t != "webm");
    theme::option_row(ui, "Codec", |ui| {
        let mut opts = vec![(VideoCodec::H264, "H.264"), (VideoCodec::H265, "H.265"), (VideoCodec::Av1, "AV1")];
        if has_webm {
            opts.push((VideoCodec::Vp9, "VP9"));
        }
        if !has_mp4ish && matches!(o.codec, VideoCodec::H264 | VideoCodec::H265) {
            o.codec = VideoCodec::Vp9;
        }
        theme::chips(ui, &mut o.codec, &opts);
        let hint = match o.codec {
            VideoCodec::H264 => "Plays everywhere",
            VideoCodec::H265 => "Smaller files, newer devices",
            VideoCodec::Av1 => "Smallest files, slowest on CPU",
            VideoCodec::Vp9 => "WebM only",
        };
        ui.label(RichText::new(hint).small().color(theme::SUBTLE));
    });
    if has_webm && matches!(o.codec, VideoCodec::H264 | VideoCodec::H265) {
        ui.horizontal(|ui| {
            ui.add_space(150.0);
            ui.label(RichText::new("WebM files use VP9").small().color(theme::SUBTLE));
        });
    }
    if has_mp4ish && o.codec == VideoCodec::Vp9 {
        ui.horizontal(|ui| {
            ui.add_space(150.0);
            ui.label(RichText::new("MP4, MKV and MOV files use H.264").small().color(theme::SUBTLE));
        });
    }

    let gpus = enc.gpus();
    theme::option_row(ui, "Encoder", |ui| {
        let mut opts: Vec<(Hardware, &str)> = vec![(Hardware::Cpu, "CPU")];
        for hw in &gpus {
            opts.push((*hw, hw.label()));
        }
        if !opts.iter().any(|(h, _)| *h == o.hardware) {
            opts.push((o.hardware, o.hardware.label()));
        }
        theme::chips(ui, &mut o.hardware, &opts);
        let msg = if gpus.is_empty() {
            "No GPU encoder found".to_owned()
        } else if o.hardware == Hardware::Cpu {
            "GPU is much faster; CPU makes smaller files".to_owned()
        } else if !enc.supports(o.codec, o.hardware) {
            format!("{} can't encode {} here, so CPU is used", o.hardware.label(), o.codec.label())
        } else {
            "Much faster, slightly larger files".to_owned()
        };
        ui.label(RichText::new(msg).small().color(theme::SUBTLE));
    });
}
