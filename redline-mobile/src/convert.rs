//! The Convert page across the bridge: records mirroring redline-core's convert options,
//! files and presets, and the desktop page's decisions (which sections show, what joins,
//! what a preset does) as plain functions. The file list itself lives in Kotlin, as it lives
//! in the desktop page; [`crate::Engine`] turns it into jobs with `convert_plan`, exactly as the
//! desktop's Convert button does.

use std::path::{Path, PathBuf};

use redline_core::convert_plan::{self, ConvertItem, Sections};
use redline_core::presets::ConvertPreset;
use redline_core::settings::Settings;
use redline_core::tools::convert::{
    AUDIO_BITRATES, Category, ConvertOptions, GIF_DITHERS, GIF_FPS, GIF_SPEEDS, GIF_WIDTHS, GifDirection,
    GifOptions, MAX_HEIGHTS, PRESETS, classify, render_stem,
};
use redline_core::tools::filters::{CropAspect, Filters, Rotation, SPEEDS, SubSource};
use redline_core::tools::hwaccel::{Encoders, Hardware, VideoCodec};

use crate::records::ChoiceRecord;

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ---- Enums --------------------------------------------------------------------------------

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCategory {
    Image,
    Video,
    Audio,
}

impl From<Category> for MediaCategory {
    fn from(c: Category) -> Self {
        match c {
            Category::Image => Self::Image,
            Category::Video => Self::Video,
            Category::Audio => Self::Audio,
        }
    }
}

impl From<MediaCategory> for Category {
    fn from(c: MediaCategory) -> Self {
        match c {
            MediaCategory::Image => Self::Image,
            MediaCategory::Video => Self::Video,
            MediaCategory::Audio => Self::Audio,
        }
    }
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecChoice {
    H264,
    H265,
    Av1,
    Vp9,
}

impl From<VideoCodec> for CodecChoice {
    fn from(c: VideoCodec) -> Self {
        match c {
            VideoCodec::H264 => Self::H264,
            VideoCodec::H265 => Self::H265,
            VideoCodec::Av1 => Self::Av1,
            VideoCodec::Vp9 => Self::Vp9,
        }
    }
}

impl From<CodecChoice> for VideoCodec {
    fn from(c: CodecChoice) -> Self {
        match c {
            CodecChoice::H264 => Self::H264,
            CodecChoice::H265 => Self::H265,
            CodecChoice::Av1 => Self::Av1,
            CodecChoice::Vp9 => Self::Vp9,
        }
    }
}

/// Which encoder makes the video. Only CPU works on Android: redline-core's GPU encoders are
/// the desktop vendors' (NVENC, Quick Sync, AMF), and a phone's ffmpeg has none of them.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderChoice {
    Cpu,
    Nvidia,
    Intel,
    Amd,
}

impl From<Hardware> for EncoderChoice {
    fn from(h: Hardware) -> Self {
        match h {
            Hardware::Cpu => Self::Cpu,
            Hardware::Nvidia => Self::Nvidia,
            Hardware::Intel => Self::Intel,
            Hardware::Amd => Self::Amd,
        }
    }
}

impl From<EncoderChoice> for Hardware {
    fn from(h: EncoderChoice) -> Self {
        match h {
            EncoderChoice::Cpu => Self::Cpu,
            EncoderChoice::Nvidia => Self::Nvidia,
            EncoderChoice::Intel => Self::Intel,
            EncoderChoice::Amd => Self::Amd,
        }
    }
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotateChoice {
    None,
    /// 90° clockwise.
    Right,
    /// 90° counter-clockwise.
    Left,
    Half,
}

impl From<Rotation> for RotateChoice {
    fn from(r: Rotation) -> Self {
        match r {
            Rotation::None => Self::None,
            Rotation::Right => Self::Right,
            Rotation::Left => Self::Left,
            Rotation::Half => Self::Half,
        }
    }
}

impl From<RotateChoice> for Rotation {
    fn from(r: RotateChoice) -> Self {
        match r {
            RotateChoice::None => Self::None,
            RotateChoice::Right => Self::Right,
            RotateChoice::Left => Self::Left,
            RotateChoice::Half => Self::Half,
        }
    }
}

/// Center crop to an aspect ratio.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropChoice {
    None,
    /// 16:9.
    Wide,
    /// 9:16.
    Tall,
    /// 1:1.
    Square,
    /// 4:5.
    Portrait,
}

impl From<CropAspect> for CropChoice {
    fn from(c: CropAspect) -> Self {
        match c {
            CropAspect::None => Self::None,
            CropAspect::Wide => Self::Wide,
            CropAspect::Tall => Self::Tall,
            CropAspect::Square => Self::Square,
            CropAspect::Portrait => Self::Portrait,
        }
    }
}

impl From<CropChoice> for CropAspect {
    fn from(c: CropChoice) -> Self {
        match c {
            CropChoice::None => Self::None,
            CropChoice::Wide => Self::Wide,
            CropChoice::Tall => Self::Tall,
            CropChoice::Square => Self::Square,
            CropChoice::Portrait => Self::Portrait,
        }
    }
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GifPlayback {
    Forward,
    Reverse,
    /// Forward then backward, so it loops seamlessly.
    Boomerang,
}

impl From<GifDirection> for GifPlayback {
    fn from(d: GifDirection) -> Self {
        match d {
            GifDirection::Forward => Self::Forward,
            GifDirection::Reverse => Self::Reverse,
            GifDirection::Boomerang => Self::Boomerang,
        }
    }
}

impl From<GifPlayback> for GifDirection {
    fn from(d: GifPlayback) -> Self {
        match d {
            GifPlayback::Forward => Self::Forward,
            GifPlayback::Reverse => Self::Reverse,
            GifPlayback::Boomerang => Self::Boomerang,
        }
    }
}

// ---- Options ------------------------------------------------------------------------------

/// Video → GIF settings.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct GifOptionsRecord {
    pub fps: u32,
    /// Output width in px; 0 keeps the source width.
    pub width: u32,
    /// Palette size, 2–256.
    pub colors: u32,
    /// An ffmpeg `paletteuse` dither mode (see [`ConvertChoiceListsRecord::gif_dithers`]).
    pub dither: String,
    /// Only redraw what changes between frames.
    pub diff_only: bool,
    /// 0 loops forever, otherwise the number of times it plays.
    pub plays: u32,
    pub speed: f32,
    pub direction: GifPlayback,
    /// Clip start in seconds.
    pub start_secs: f64,
    /// Clip length in seconds of source time; 0 runs to the end.
    pub length_secs: f64,
}

impl From<&GifOptions> for GifOptionsRecord {
    fn from(g: &GifOptions) -> Self {
        Self {
            fps: g.fps,
            width: g.width,
            colors: g.colors,
            dither: g.dither.clone(),
            diff_only: g.diff_only,
            plays: g.plays,
            speed: g.speed,
            direction: g.direction.into(),
            start_secs: g.start,
            length_secs: g.length,
        }
    }
}

impl From<&GifOptionsRecord> for GifOptions {
    fn from(g: &GifOptionsRecord) -> Self {
        Self {
            fps: g.fps,
            width: g.width,
            colors: g.colors,
            dither: g.dither.clone(),
            diff_only: g.diff_only,
            plays: g.plays,
            speed: g.speed,
            direction: g.direction.into(),
            start: g.start_secs,
            length: g.length_secs,
        }
    }
}

/// Rotate, flip, crop, speed and loudness.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct FiltersRecord {
    pub rotate: RotateChoice,
    /// Mirror.
    pub flip: bool,
    pub crop: CropChoice,
    /// Playback speed for video and audio (GIFs have their own).
    pub speed: f32,
    /// Even out loudness (EBU R128, -16 LUFS).
    pub normalize: bool,
    pub gain_db: f32,
}

impl From<&Filters> for FiltersRecord {
    fn from(f: &Filters) -> Self {
        Self {
            rotate: f.rotate.into(),
            flip: f.flip,
            crop: f.crop.into(),
            speed: f.speed,
            normalize: f.normalize,
            gain_db: f.gain_db,
        }
    }
}

impl From<&FiltersRecord> for Filters {
    fn from(f: &FiltersRecord) -> Self {
        Self {
            rotate: f.rotate.into(),
            flip: f.flip,
            crop: f.crop.into(),
            speed: f.speed,
            normalize: f.normalize,
            gain_db: f.gain_db,
        }
    }
}

/// How files are converted: the shared options on the Convert page, or one file's own.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ConvertOptionsRecord {
    /// x264-style quality, lower is better; the page's slider shows 51 − crf.
    pub crf: u8,
    /// x264 preset name (see [`ConvertChoiceListsRecord::encode_speeds`]).
    pub preset: String,
    pub codec: CodecChoice,
    pub hardware: EncoderChoice,
    /// 0 keeps the original.
    pub max_height: u32,
    /// kbit/s.
    pub audio_bitrate: u32,
    /// 1–100, for JPG and AVIF.
    pub image_quality: u8,
    /// Fit within this box; 0 keeps the original size.
    pub resize_width: u32,
    pub resize_height: u32,
    pub gif: GifOptionsRecord,
    /// Aim for this output size in MB (video only); 0 uses the quality setting.
    pub target_size_mb: f32,
    pub filters: FiltersRecord,
}

impl From<&ConvertOptions> for ConvertOptionsRecord {
    fn from(o: &ConvertOptions) -> Self {
        Self {
            crf: o.crf,
            preset: o.preset.clone(),
            codec: o.codec.into(),
            hardware: o.hardware.into(),
            max_height: o.max_height,
            audio_bitrate: o.audio_bitrate,
            image_quality: o.image_quality,
            resize_width: o.resize_width,
            resize_height: o.resize_height,
            gif: (&o.gif).into(),
            target_size_mb: o.target_size_mb,
            filters: (&o.filters).into(),
        }
    }
}

impl From<&ConvertOptionsRecord> for ConvertOptions {
    fn from(o: &ConvertOptionsRecord) -> Self {
        Self {
            crf: o.crf,
            preset: o.preset.clone(),
            codec: o.codec.into(),
            hardware: o.hardware.into(),
            max_height: o.max_height,
            audio_bitrate: o.audio_bitrate,
            image_quality: o.image_quality,
            resize_width: o.resize_width,
            resize_height: o.resize_height,
            gif: (&o.gif).into(),
            target_size_mb: o.target_size_mb,
            filters: (&o.filters).into(),
        }
    }
}

/// The options a fresh install starts with; the page's "Reset" links take their GIF and
/// adjust parts.
#[uniffi::export]
pub fn default_convert_options() -> ConvertOptionsRecord {
    (&ConvertOptions::default()).into()
}

// ---- Files --------------------------------------------------------------------------------

/// Where burned-in subtitles come from.
#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum SubtitleBurn {
    /// The video's first subtitle track.
    Embedded,
    /// A subtitle file (.srt, .ass, .ssa, .vtt).
    File { path: String },
}

impl From<&SubSource> for SubtitleBurn {
    fn from(s: &SubSource) -> Self {
        match s {
            SubSource::Embedded => Self::Embedded,
            SubSource::File(p) => Self::File { path: path_string(p) },
        }
    }
}

impl From<&SubtitleBurn> for SubSource {
    fn from(s: &SubtitleBurn) -> Self {
        match s {
            SubtitleBurn::Embedded => Self::Embedded,
            SubtitleBurn::File { path } => Self::File(PathBuf::from(path)),
        }
    }
}

/// A file on the Convert page.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ConvertItemRecord {
    pub path: String,
    /// The file name, as listed.
    pub name: String,
    pub category: MediaCategory,
    /// Bytes.
    pub size: u64,
    /// From ffprobe; `None` for images, or when it couldn't be read.
    pub duration_secs: Option<f64>,
    /// The format it becomes (one of [`ConvertChoiceListsRecord`]'s targets for its category).
    pub target: String,
    /// Settings for this file only; `None` follows the shared options.
    pub options: Option<ConvertOptionsRecord>,
    /// Only used when video stays video (see [`convert_item_makes_video`]).
    pub burn_subs: Option<SubtitleBurn>,
}

impl ConvertItemRecord {
    fn from_core(item: &ConvertItem, duration_secs: Option<f64>) -> Self {
        Self {
            path: path_string(&item.path),
            name: item.name(),
            category: item.category.into(),
            size: item.size,
            duration_secs,
            target: item.target.clone(),
            options: item.options.as_ref().map(Into::into),
            burn_subs: item.burn_subs.as_ref().map(Into::into),
        }
    }

    pub fn to_core(&self) -> ConvertItem {
        ConvertItem {
            path: PathBuf::from(&self.path),
            category: self.category.into(),
            size: self.size,
            target: self.target.clone(),
            options: self.options.as_ref().map(Into::into),
            burn_subs: self.burn_subs.as_ref().map(Into::into),
        }
    }
}

fn to_core(items: &[ConvertItemRecord]) -> Vec<ConvertItem> {
    items.iter().map(ConvertItemRecord::to_core).collect()
}

/// Files read for the page, and the names of those it can't convert.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ConvertFilesRecord {
    pub items: Vec<ConvertItemRecord>,
    pub skipped: Vec<String>,
}

/// The desktop's `add_paths`: a row per supported file (in the order given), with its
/// category's default format; `duration` reads a file's length (ffprobe in the engine).
pub fn convert_files(paths: Vec<String>, duration: impl Fn(&Path) -> Option<f64>) -> ConvertFilesRecord {
    let mut out = ConvertFilesRecord { items: Vec::new(), skipped: Vec::new() };
    for path in paths {
        let path = PathBuf::from(path);
        match ConvertItem::from_path(path.clone()) {
            Some(item) => {
                let secs = (item.category != Category::Image).then(|| duration(&item.path)).flatten();
                out.items.push(ConvertItemRecord::from_core(&item, secs));
            }
            None => out.skipped.push(
                path.file_name().map_or_else(|| path_string(&path), |n| n.to_string_lossy().into_owned()),
            ),
        }
    }
    out
}

/// What kind of file a name is, by its extension; `None` if Redline can't convert it.
#[uniffi::export]
pub fn classify_file_name(name: String) -> Option<MediaCategory> {
    classify(Path::new(&name)).map(Into::into)
}

/// Video in, video out (not GIF): the only case where subtitles can be burned in.
#[uniffi::export]
pub fn convert_item_makes_video(item: ConvertItemRecord) -> bool {
    item.to_core().makes_video()
}

/// The format all the files would be joined into, if "Join into one" makes sense for them.
#[uniffi::export]
pub fn convert_join_target(items: Vec<ConvertItemRecord>) -> Option<String> {
    let core = to_core(&items);
    convert_plan::join_target(&core).map(str::to_owned)
}

/// "→ name.ext": the first file's new name under `template`, as the page previews it.
#[uniffi::export]
pub fn rename_example(template: String, item: ConvertItemRecord) -> String {
    let core = item.to_core();
    format!("{}.{}", render_stem(&template, &core.stem(), 1, &core.target), core.target)
}

// ---- Choices ------------------------------------------------------------------------------

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct NumberChoiceRecord {
    pub value: u32,
    pub label: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SpeedChoiceRecord {
    pub value: f32,
    pub label: String,
}

/// What the Convert page's pickers offer, as the desktop shows them.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ConvertChoiceListsRecord {
    pub image_targets: Vec<String>,
    pub video_targets: Vec<String>,
    pub audio_targets: Vec<String>,
    /// x264 preset names, fastest first.
    pub encode_speeds: Vec<String>,
    pub max_heights: Vec<NumberChoiceRecord>,
    /// kbit/s.
    pub audio_bitrates: Vec<u32>,
    pub gif_widths: Vec<NumberChoiceRecord>,
    pub gif_fps: Vec<u32>,
    pub gif_dithers: Vec<ChoiceRecord>,
    pub gif_speeds: Vec<SpeedChoiceRecord>,
    /// Video and audio playback speeds.
    pub speeds: Vec<SpeedChoiceRecord>,
}

impl ConvertChoiceListsRecord {
    pub fn targets(&self, category: MediaCategory) -> &[String] {
        match category {
            MediaCategory::Image => &self.image_targets,
            MediaCategory::Video => &self.video_targets,
            MediaCategory::Audio => &self.audio_targets,
        }
    }
}

#[uniffi::export]
pub fn convert_choice_lists() -> ConvertChoiceListsRecord {
    let strings = |s: &[&str]| s.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
    let numbers = |s: &[(u32, &str)]| {
        s.iter().map(|(v, l)| NumberChoiceRecord { value: *v, label: (*l).to_owned() }).collect::<Vec<_>>()
    };
    let speeds = |s: &[(f32, &str)]| {
        s.iter().map(|(v, l)| SpeedChoiceRecord { value: *v, label: (*l).to_owned() }).collect::<Vec<_>>()
    };
    ConvertChoiceListsRecord {
        image_targets: strings(Category::Image.targets()),
        video_targets: strings(Category::Video.targets()),
        audio_targets: strings(Category::Audio.targets()),
        encode_speeds: strings(PRESETS),
        max_heights: numbers(MAX_HEIGHTS),
        audio_bitrates: AUDIO_BITRATES.to_vec(),
        gif_widths: numbers(GIF_WIDTHS),
        gif_fps: GIF_FPS.to_vec(),
        gif_dithers: GIF_DITHERS
            .iter()
            .map(|(v, l)| ChoiceRecord { value: (*v).to_owned(), label: (*l).to_owned() })
            .collect(),
        gif_speeds: speeds(GIF_SPEEDS),
        speeds: speeds(SPEEDS),
    }
}

// ---- Sections and encoders ----------------------------------------------------------------

/// The video encoders ffmpeg can use here, by ffmpeg name, as `Engine::detect_encoders`
/// found them. Empty until detection ran.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Default)]
pub struct EncodersRecord {
    pub working: Vec<String>,
}

impl EncodersRecord {
    pub fn to_core(&self) -> Encoders {
        Encoders::from_names(self.working.iter().cloned())
    }
}

impl From<&Encoders> for EncodersRecord {
    fn from(e: &Encoders) -> Self {
        Self { working: e.names().map(str::to_owned).collect() }
    }
}

/// Which option groups to show for some files, and the codec and encoder rows' contents.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ConvertSectionsRecord {
    pub video: bool,
    pub audio: bool,
    pub gif: bool,
    pub image: bool,
    /// Rotate and crop.
    pub picture: bool,
    /// Speed and loudness.
    pub timed: bool,
    pub codecs: Vec<CodecChoice>,
    /// The codec to show selected; when it differs from the options', the page writes it back
    /// (every file becomes WebM, which can't hold H.264 or H.265).
    pub codec: CodecChoice,
    pub codec_hint: String,
    /// Small print under the codec row.
    pub codec_notes: Vec<String>,
    pub encoders: Vec<EncoderChoice>,
    pub encoder_note: String,
}

/// The desktop's sections for `items` (the files following the shared options, or the one file
/// being edited) with options `o`.
#[uniffi::export]
pub fn convert_sections(
    items: Vec<ConvertItemRecord>,
    options: ConvertOptionsRecord,
    encoders: EncodersRecord,
) -> ConvertSectionsRecord {
    let core = to_core(&items);
    let s = Sections::of(&core);
    let enc = encoders.to_core();
    let mut o = ConvertOptions::from(&options);
    o.codec = s.fit_codec(o.codec);
    ConvertSectionsRecord {
        video: s.video,
        audio: s.audio,
        gif: s.gif,
        image: s.image,
        picture: s.picture(),
        timed: s.timed(),
        codecs: s.codecs().into_iter().map(Into::into).collect(),
        codec: o.codec.into(),
        codec_hint: convert_plan::codec_hint(o.codec).to_owned(),
        codec_notes: s.codec_notes(o.codec).into_iter().map(str::to_owned).collect(),
        encoders: convert_plan::encoder_choices(&enc, o.hardware).into_iter().map(Into::into).collect(),
        encoder_note: convert_plan::encoder_note(&enc, &o),
    }
}

/// "CPU", "NVIDIA"…
#[uniffi::export]
pub fn encoder_label(encoder: EncoderChoice) -> String {
    Hardware::from(encoder).label().to_owned()
}

/// "H.264", "AV1"…
#[uniffi::export]
pub fn codec_label(codec: CodecChoice) -> String {
    VideoCodec::from(codec).label().to_owned()
}

// ---- Presets ------------------------------------------------------------------------------

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ConvertPresetRecord {
    pub name: String,
    pub options: ConvertOptionsRecord,
    /// Output format per kind of file; `None` leaves those files' formats alone.
    pub image_target: Option<String>,
    pub video_target: Option<String>,
    pub audio_target: Option<String>,
}

impl From<&ConvertPreset> for ConvertPresetRecord {
    fn from(p: &ConvertPreset) -> Self {
        Self {
            name: p.name.clone(),
            options: (&p.options).into(),
            image_target: p.image_target.clone(),
            video_target: p.video_target.clone(),
            audio_target: p.audio_target.clone(),
        }
    }
}

impl From<&ConvertPresetRecord> for ConvertPreset {
    fn from(p: &ConvertPresetRecord) -> Self {
        Self {
            name: p.name.clone(),
            options: (&p.options).into(),
            image_target: p.image_target.clone(),
            video_target: p.video_target.clone(),
            audio_target: p.audio_target.clone(),
        }
    }
}

/// Whether `options` are this preset's (the encoder aside), to highlight it.
#[uniffi::export]
pub fn convert_preset_matches(preset: ConvertPresetRecord, options: ConvertOptionsRecord) -> bool {
    ConvertPreset::from(&preset).matches(&(&options).into())
}

/// The shared options and files after applying a preset.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PresetAppliedRecord {
    pub options: ConvertOptionsRecord,
    pub items: Vec<ConvertItemRecord>,
}

/// Applies a preset as the desktop does: its options (keeping the encoder), and its formats
/// for the files that follow the shared options.
#[uniffi::export]
pub fn apply_convert_preset(
    preset: ConvertPresetRecord,
    options: ConvertOptionsRecord,
    items: Vec<ConvertItemRecord>,
) -> PresetAppliedRecord {
    let mut shared = ConvertOptions::from(&options);
    let mut core = to_core(&items);
    ConvertPreset::from(&preset).apply(&mut shared, &mut core);
    PresetAppliedRecord {
        options: (&shared).into(),
        items: items
            .into_iter()
            .zip(core)
            .map(|(r, c)| ConvertItemRecord { target: c.target, ..r })
            .collect(),
    }
}

/// "Save current": a preset of `options`, with the formats in use where every file of a kind
/// agrees.
#[uniffi::export]
pub fn new_convert_preset(
    name: String,
    options: ConvertOptionsRecord,
    items: Vec<ConvertItemRecord>,
) -> ConvertPresetRecord {
    let core = to_core(&items);
    (&ConvertPreset::from_items(name.trim().to_owned(), (&options).into(), &core)).into()
}

// ---- Settings -----------------------------------------------------------------------------

/// The Convert page's remembered settings, inside [`crate::SettingsRecord`].
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ConvertSettingsRecord {
    /// Where converted files go (the app's own folder on Android).
    pub convert_dir: Option<String>,
    /// File name template: `{name}`, `{n}`, `{date}`, `{target}`.
    pub rename_template: String,
    /// The shared options.
    pub options: ConvertOptionsRecord,
    pub presets: Vec<ConvertPresetRecord>,
}

pub fn convert_settings_record(s: &Settings) -> ConvertSettingsRecord {
    ConvertSettingsRecord {
        convert_dir: s.convert_dir.as_deref().map(path_string),
        rename_template: s.rename_template.clone(),
        options: (&s.convert).into(),
        presets: s.convert_presets.iter().map(Into::into).collect(),
    }
}

/// Writes the Convert page's settings back. A blank folder keeps the current one: Android has
/// nowhere else ffmpeg could write to.
pub fn apply_convert_settings(r: &ConvertSettingsRecord, s: &mut Settings) {
    if let Some(dir) = r.convert_dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        s.convert_dir = Some(PathBuf::from(dir));
    }
    s.rename_template = r.rename_template.clone();
    s.convert = (&r.options).into();
    s.convert_presets = r.presets.iter().map(Into::into).collect();
}

// ---- Copied inputs ------------------------------------------------------------------------

/// Folders under the inputs folder that nothing uses any more: no path in `in_use` is inside
/// them. Kotlin copies each picked file into a folder of its own there.
pub fn stale_input_dirs(dirs: &[PathBuf], in_use: &[PathBuf]) -> Vec<PathBuf> {
    dirs.iter().filter(|d| !in_use.iter().any(|p| p.starts_with(d))).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: &str, category: MediaCategory, target: &str) -> ConvertItemRecord {
        ConvertItemRecord {
            path: path.into(),
            name: Path::new(path).file_name().unwrap().to_string_lossy().into_owned(),
            category,
            size: 10,
            duration_secs: None,
            target: target.into(),
            options: None,
            burn_subs: None,
        }
    }

    #[test]
    fn options_round_trip() {
        let mut o = ConvertOptions::default();
        o.codec = VideoCodec::Av1;
        o.hardware = Hardware::Intel;
        o.gif.direction = GifDirection::Boomerang;
        o.gif.length = 2.5;
        o.filters.crop = CropAspect::Portrait;
        o.filters.rotate = Rotation::Left;
        o.target_size_mb = 8.0;
        let r = ConvertOptionsRecord::from(&o);
        assert_eq!((r.codec, r.hardware, r.gif.direction), (CodecChoice::Av1, EncoderChoice::Intel, GifPlayback::Boomerang));
        assert_eq!(ConvertOptions::from(&r), o);
        assert_eq!(default_convert_options(), (&ConvertOptions::default()).into());

        let mut item = record("/c/a.mkv", MediaCategory::Video, "mp4");
        item.burn_subs = Some(SubtitleBurn::File { path: "/c/a.srt".into() });
        item.options = Some(r);
        let core = item.to_core();
        assert_eq!(core.burn_subs, Some(SubSource::File("/c/a.srt".into())));
        assert_eq!(ConvertItemRecord::from_core(&core, None), item);
    }

    #[test]
    fn reads_files_like_the_desktop_page() {
        let dir = std::env::temp_dir().join(format!("redline-mobile-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (clip, pic, text) = (dir.join("clip.MKV"), dir.join("pic.png"), dir.join("notes.txt"));
        for f in [&clip, &pic, &text] {
            std::fs::write(f, b"12345").unwrap();
        }
        let paths = [&clip, &pic, &text, &dir.join("gone.mp3")].map(|p| path_string(p)).to_vec();
        let r = convert_files(paths, |_| Some(42.0));
        assert_eq!(r.items.len(), 2);
        assert_eq!((r.items[0].category, r.items[0].target.as_str(), r.items[0].size), (MediaCategory::Video, "mp4", 5));
        assert_eq!(r.items[0].duration_secs, Some(42.0));
        assert_eq!((r.items[1].name.as_str(), r.items[1].duration_secs), ("pic.png", None), "images aren't probed");
        assert_eq!(r.skipped, ["notes.txt", "gone.mp3"]);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(classify_file_name("Song.FLAC".into()), Some(MediaCategory::Audio));
        assert_eq!(classify_file_name("doc.pdf".into()), None);
    }

    #[test]
    fn joins_previews_and_sections() {
        let a = record("/v/a.mkv", MediaCategory::Video, "mp4");
        let b = record("/v/b.mov", MediaCategory::Video, "mp3");
        assert_eq!(convert_join_target(vec![a.clone(), b.clone()]).as_deref(), Some("mp4"));
        assert_eq!(convert_join_target(vec![a.clone()]), None);
        assert!(convert_item_makes_video(a.clone()) && !convert_item_makes_video(b.clone()));
        assert_eq!(rename_example("{n} - {name}".into(), a.clone()), "1 - a.mp4");

        let webm = record("/v/c.mkv", MediaCategory::Video, "webm");
        let s = convert_sections(vec![webm], default_convert_options(), EncodersRecord::default());
        assert!(s.video && s.audio && s.picture && s.timed && !s.image && !s.gif);
        assert_eq!(s.codec, CodecChoice::Vp9, "WebM only: H.264 moves to VP9");
        assert_eq!(s.codec_hint, "WebM only");
        assert_eq!(s.encoders, [EncoderChoice::Cpu]);
        assert_eq!(s.encoder_note, "No GPU encoder found");

        let enc = EncodersRecord { working: vec!["h264_nvenc".into(), "libx264".into()] };
        let mut o = default_convert_options();
        o.hardware = EncoderChoice::Nvidia;
        let s = convert_sections(vec![a, b], o, enc.clone());
        assert_eq!(s.encoders, [EncoderChoice::Cpu, EncoderChoice::Nvidia]);
        assert_eq!(s.codec, CodecChoice::H264);
        assert_eq!(EncodersRecord::from(&enc.to_core()), enc);

        let lists = convert_choice_lists();
        assert_eq!(lists.targets(MediaCategory::Image)[0], "png");
        assert!(lists.video_targets.contains(&"gif".to_owned()));
        assert_eq!(lists.max_heights[0].value, 0);
        assert_eq!(lists.gif_dithers[0].label, "Smooth");
    }

    #[test]
    fn presets_apply_and_save_like_the_desktop() {
        let settings = Settings::default();
        let discord: ConvertPresetRecord = (&settings.convert_presets[0]).into();
        let mut own = record("/v/b.mkv", MediaCategory::Video, "mkv");
        own.options = Some(default_convert_options());
        let items = vec![record("/v/a.mkv", MediaCategory::Video, "webm"), own, record("/m/c.wav", MediaCategory::Audio, "ogg")];
        let mut current = default_convert_options();
        current.hardware = EncoderChoice::Amd;
        assert!(!convert_preset_matches(discord.clone(), current.clone()));

        let applied = apply_convert_preset(discord.clone(), current, items.clone());
        assert!(convert_preset_matches(discord, applied.options.clone()));
        assert_eq!(applied.options.hardware, EncoderChoice::Amd, "the encoder stays");
        let targets: Vec<_> = applied.items.iter().map(|i| i.target.as_str()).collect();
        assert_eq!(targets, ["mp4", "mkv", "ogg"]);

        let saved = new_convert_preset("  Mine ".into(), applied.options, items);
        assert_eq!(saved.name, "Mine");
        assert_eq!((saved.video_target, saved.audio_target.as_deref()), (None, Some("ogg")));
    }

    #[test]
    fn settings_write_back() {
        let mut s = Settings::default();
        s.convert_dir = Some("/a/Converted".into());
        let mut r = convert_settings_record(&s);
        assert_eq!(r.presets.len(), 4);
        r.convert_dir = Some(" ".into());
        r.rename_template = "{n} {name}".into();
        r.options.crf = 30;
        r.presets.pop();
        apply_convert_settings(&r, &mut s);
        assert_eq!(s.convert_dir, Some(PathBuf::from("/a/Converted")), "a blank folder is ignored");
        assert_eq!((s.rename_template.as_str(), s.convert.crf, s.convert_presets.len()), ("{n} {name}", 30, 3));
    }

    #[test]
    fn finds_unused_input_copies() {
        let root = PathBuf::from("/cache/convert-inputs");
        let dirs = ["1", "2", "3"].map(|d| root.join(d)).to_vec();
        let in_use = vec![root.join("1").join("a.mp4"), root.join("3"), PathBuf::from("/elsewhere/2/x.mp4")];
        assert_eq!(stale_input_dirs(&dirs, &in_use), [root.join("2")]);
    }
}
