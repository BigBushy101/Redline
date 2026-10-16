//! Local file conversion: routes images to the `image` crate and video/audio to ffmpeg.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use super::{ToolPaths, command, ffmpeg, image_conv, run_streaming, unique_path};
use crate::jobs::{CancelHandle, Cancelled, Reporter};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Category {
    Image,
    Video,
    Audio,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Image => "Image",
            Category::Video => "Video",
            Category::Audio => "Audio",
        }
    }

    pub fn targets(self) -> &'static [&'static str] {
        match self {
            Category::Image => &["png", "jpg", "webp", "avif", "gif", "bmp", "tiff", "ico"],
            Category::Video => &[
                "mp4", "mkv", "webm", "mov", "avi", "gif", "mp3", "m4a", "flac", "wav", "ogg",
            ],
            Category::Audio => &["mp3", "m4a", "flac", "wav", "ogg", "opus"],
        }
    }

    pub fn default_target(self) -> &'static str {
        match self {
            Category::Image => "png",
            Category::Video => "mp4",
            Category::Audio => "mp3",
        }
    }
}

pub fn classify(path: &Path) -> Option<Category> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "ico" | "tga" | "qoi" => {
            Category::Image
        }
        // Animated GIFs are treated as images; convert a video *to* gif for the reverse.
        "gif" => Category::Image,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "flv" | "wmv" | "mpg" | "mpeg" | "ts"
        | "3gp" | "ogv" => Category::Video,
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "opus" | "wma" | "aiff" | "aif"
        | "alac" => Category::Audio,
        _ => return None,
    })
}

pub const PRESETS: &[&str] = &["ultrafast", "veryfast", "fast", "medium", "slow", "veryslow"];
pub const MAX_HEIGHTS: &[(u32, &str)] = &[
    (0, "Original"),
    (2160, "2160p"),
    (1440, "1440p"),
    (1080, "1080p"),
    (720, "720p"),
    (480, "480p"),
];
pub const AUDIO_BITRATES: &[u32] = &[96, 128, 160, 192, 256, 320];

pub const GIF_WIDTHS: &[(u32, &str)] = &[
    (0, "Original"),
    (1080, "1080 px"),
    (800, "800 px"),
    (640, "640 px"),
    (480, "480 px"),
    (320, "320 px"),
    (240, "240 px"),
];
pub const GIF_FPS: &[u32] = &[5, 10, 12, 15, 20, 24, 30, 50];
/// ffmpeg `paletteuse` dither modes and what to call them.
pub const GIF_DITHERS: &[(&str, &str)] = &[
    ("sierra2_4a", "Smooth"),
    ("floyd_steinberg", "Sharp"),
    ("bayer", "Pattern"),
    ("none", "None"),
];
pub const GIF_SPEEDS: &[(f32, &str)] = &[
    (0.25, "0.25×"),
    (0.5, "0.5×"),
    (1.0, "1×"),
    (1.5, "1.5×"),
    (2.0, "2×"),
    (4.0, "4×"),
];

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum GifDirection {
    Forward,
    Reverse,
    /// Forward then backward, so it loops seamlessly.
    Boomerang,
}

/// Video → GIF settings.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct GifOptions {
    pub fps: u32,
    /// Output width in px; 0 keeps the source width.
    pub width: u32,
    /// Palette size, 2–256.
    pub colors: u32,
    pub dither: String,
    /// Only re-encode the parts of each frame that changed. Smaller files for mostly still clips.
    pub diff_only: bool,
    /// 0 loops forever, otherwise the number of times it plays.
    pub plays: u32,
    pub speed: f32,
    pub direction: GifDirection,
    /// Clip start in seconds.
    pub start: f64,
    /// Clip length in seconds of source time; 0 runs to the end.
    pub length: f64,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            fps: 15,
            width: 480,
            colors: 256,
            dither: "sierra2_4a".into(),
            diff_only: false,
            plays: 0,
            speed: 1.0,
            direction: GifDirection::Forward,
            start: 0.0,
            length: 0.0,
        }
    }
}

impl GifOptions {
    /// How long the GIF runs, given the source's duration.
    pub fn output_duration(&self, source: f64) -> f64 {
        let rest = (source - self.start).max(0.0);
        let clip = if self.length > 0.0 { self.length.min(rest) } else { rest };
        let d = clip / self.speed.max(0.01) as f64;
        if self.direction == GifDirection::Boomerang { d * 2.0 } else { d }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct ConvertOptions {
    pub crf: u8,
    pub preset: String,
    pub max_height: u32,
    pub audio_bitrate: u32,
    pub image_quality: u8,
    pub resize_width: u32,
    pub resize_height: u32,
    pub gif: GifOptions,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            crf: 23,
            preset: "medium".into(),
            max_height: 0,
            audio_bitrate: 192,
            image_quality: 85,
            resize_width: 0,
            resize_height: 0,
            gif: GifOptions::default(),
        }
    }
}

#[derive(Clone)]
pub struct ConvertRequest {
    pub input: PathBuf,
    pub category: Category,
    pub target: String,
    pub options: ConvertOptions,
    /// `None` writes next to the input.
    pub out_dir: Option<PathBuf>,
}

pub fn run(
    tools: &ToolPaths,
    req: &ConvertRequest,
    cancel: &CancelHandle,
    rep: &Reporter,
) -> Result<Option<PathBuf>> {
    let dir = match &req.out_dir {
        Some(d) => d.clone(),
        None => req
            .input
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("can't create {}", dir.display()))?;
    let stem = req
        .input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    let output = unique_path(&dir, &stem, &req.target);

    let result = match req.category {
        Category::Image => {
            rep.progress(None, "Converting image…");
            image_conv::convert(&req.input, &output, &req.target, &req.options)
        }
        Category::Video | Category::Audio => run_ffmpeg(tools, req, &output, cancel, rep),
    };

    match result {
        Ok(()) => Ok(Some(output)),
        Err(e) => {
            // Don't leave half-written files behind.
            let _ = std::fs::remove_file(&output);
            Err(e)
        }
    }
}

fn run_ffmpeg(
    tools: &ToolPaths,
    req: &ConvertRequest,
    output: &Path,
    cancel: &CancelHandle,
    rep: &Reporter,
) -> Result<()> {
    let ffmpeg = tools
        .ffmpeg
        .as_ref()
        .ok_or_else(|| anyhow!("ffmpeg is not installed"))?;
    let duration = tools
        .ffprobe
        .as_ref()
        .and_then(|p| ffmpeg::probe_duration(p, &req.input))
        .map(|d| match req.target.as_str() {
            "gif" => req.options.gif.output_duration(d),
            _ => d,
        })
        .filter(|d| *d > 0.0);
    if cancel.is_cancelled() {
        return Err(Cancelled.into());
    }

    let mut cmd = command(ffmpeg);
    cmd.args(ffmpeg::build_args(&req.input, output, &req.target, &req.options));
    rep.progress(Some(0.0), "Starting…");
    run_streaming(cmd, cancel, |line| {
        if let Some(secs) = ffmpeg::parse_progress(line) {
            match duration {
                Some(total) => {
                    let frac = (secs / total).clamp(0.0, 1.0) as f32;
                    rep.progress(
                        Some(frac),
                        format!(
                            "{:.0}% · {} / {}",
                            frac * 100.0,
                            crate::ui::fmt_duration(secs),
                            crate::ui::fmt_duration(total)
                        ),
                    );
                }
                None => rep.progress(None, crate::ui::fmt_duration(secs)),
            }
        } else if line == "progress=end" {
            rep.processing("Finishing…");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(classify(Path::new("a.PNG")), Some(Category::Image));
        assert_eq!(classify(Path::new("b.jpeg")), Some(Category::Image));
        assert_eq!(classify(Path::new("c.mkv")), Some(Category::Video));
        assert_eq!(classify(Path::new("d.flac")), Some(Category::Audio));
        assert_eq!(classify(Path::new("e.txt")), None);
        assert_eq!(classify(Path::new("noext")), None);
    }

    #[test]
    fn gif_output_duration() {
        let mut g = GifOptions::default();
        assert_eq!(g.output_duration(10.0), 10.0);
        g.start = 4.0;
        assert_eq!(g.output_duration(10.0), 6.0);
        g.length = 2.0;
        g.speed = 2.0;
        assert_eq!(g.output_duration(10.0), 1.0);
        g.direction = GifDirection::Boomerang;
        assert_eq!(g.output_duration(10.0), 2.0);
        g.start = 20.0;
        assert_eq!(g.output_duration(10.0), 0.0);
    }

    #[test]
    fn default_target_is_offered() {
        for c in [Category::Image, Category::Video, Category::Audio] {
            assert!(c.targets().contains(&c.default_target()));
        }
    }
}
