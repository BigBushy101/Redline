//! Local file conversion: routes images to the `image` crate and video/audio to ffmpeg.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use super::ffmpeg::{Encode, is_audio_target};
use super::filters::{Filters, SubSource};
use super::hwaccel::{self, Hardware, Rate, VideoCodec};
use super::{ToolPaths, command, ffmpeg, image_conv, run_streaming, unique_path};
use crate::jobs::{CancelHandle, Cancelled, Reporter};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default, Serialize, Deserialize)]
pub enum Category {
    #[default]
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
    pub codec: VideoCodec,
    pub hardware: Hardware,
    pub max_height: u32,
    pub audio_bitrate: u32,
    pub image_quality: u8,
    pub resize_width: u32,
    pub resize_height: u32,
    pub gif: GifOptions,
    /// Aim for this output size in MB (video only); 0 uses the quality setting.
    pub target_size_mb: f32,
    pub filters: Filters,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            crf: 23,
            preset: "medium".into(),
            codec: VideoCodec::H264,
            hardware: Hardware::Cpu,
            max_height: 0,
            audio_bitrate: 192,
            image_quality: 85,
            resize_width: 0,
            resize_height: 0,
            gif: GifOptions::default(),
            target_size_mb: 0.0,
            filters: Filters::default(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConvertRequest {
    pub input: PathBuf,
    pub category: Category,
    pub target: String,
    pub options: ConvertOptions,
    /// `None` writes next to the input.
    pub out_dir: Option<PathBuf>,
    pub burn_subs: Option<SubSource>,
    /// Output file name without extension; `None` keeps the input's name.
    pub output_stem: Option<String>,
    /// Move the original to the Recycle Bin once the conversion succeeds.
    pub recycle_source: bool,
}

/// Several files joined end to end into one.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConcatRequest {
    pub inputs: Vec<PathBuf>,
    pub target: String,
    pub options: ConvertOptions,
    pub out_dir: Option<PathBuf>,
    pub output_stem: String,
    pub recycle_sources: bool,
}

/// Fills a file name template: `{name}` (original name), `{n}` (position in the list),
/// `{date}` (today) and `{target}` (new extension). The result is safe as a file name.
pub fn render_stem(template: &str, name: &str, n: usize, target: &str) -> String {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let t = if template.trim().is_empty() { "{name}" } else { template };
    let raw = t
        .replace("{name}", name)
        .replace("{n}", &n.to_string())
        .replace("{date}", &date)
        .replace("{target}", target);
    let clean: String = raw
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_end_matches('.').trim().to_owned();
    if clean.is_empty() { name.to_owned() } else { clean }
}

/// Sends a finished job's source to the Recycle Bin; failures are ignored (the output is
/// already safe, and the original simply stays).
#[cfg(not(target_os = "android"))]
fn recycle(path: &Path) {
    let _ = trash::delete(path);
}

/// Android has no recycle bin, and deleting outright isn't what the setting promises, so the
/// original stays.
#[cfg(target_os = "android")]
fn recycle(_path: &Path) {}

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
    let stem = req.output_stem.clone().unwrap_or_else(|| {
        req.input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "output".into())
    });
    let output = unique_path(&dir, &stem, &req.target);

    let result = match req.category {
        Category::Image => {
            rep.progress(None, "Converting image…");
            image_conv::convert(&req.input, &output, &req.target, &req.options)
        }
        Category::Video | Category::Audio => run_ffmpeg(tools, req, &output, cancel, rep),
    };

    match result {
        Ok(()) => {
            if req.recycle_source && std::fs::metadata(&output).is_ok_and(|m| m.len() > 0) {
                recycle(&req.input);
            }
            Ok(Some(output))
        }
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
    let o = &req.options;
    let duration = tools
        .ffprobe
        .as_ref()
        .and_then(|p| ffmpeg::probe_duration(p, &req.input))
        .map(|d| match req.target.as_str() {
            "gif" => o.gif.output_duration(d),
            _ => d / o.filters.speed.max(0.01) as f64,
        })
        .filter(|d| *d > 0.0);
    if cancel.is_cancelled() {
        return Err(Cancelled.into());
    }

    let sized = o.target_size_mb > 0.0
        && !hwaccel::codecs_for(&req.target).is_empty()
        && req.category == Category::Video;
    let video_kbps = if sized {
        let d = duration.ok_or_else(|| {
            anyhow!("Can't aim for a file size: couldn't read how long this video is")
        })?;
        Some(ffmpeg::target_video_kbps(o.target_size_mb, d, o.audio_bitrate))
    } else {
        None
    };
    let two_pass = sized && ffmpeg::supports_two_pass(&req.target, o);
    let log_dir = std::env::temp_dir().join(format!(
        "redline-2pass-{}-{}",
        std::process::id(),
        output.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    if two_pass {
        std::fs::create_dir_all(&log_dir)?;
    }
    let log = log_dir.join("pass");

    let passes: &[Option<u8>] = if two_pass { &[Some(1), Some(2)] } else { &[None] };
    let mut result = Ok(());
    for (i, pass) in passes.iter().enumerate() {
        let enc = Encode {
            video_kbps,
            pass: pass.map(|n| (n, log.as_path())),
            subs: req.burn_subs.as_ref(),
        };
        let mut cmd = command(ffmpeg);
        cmd.args(ffmpeg::build_args(&req.input, output, &req.target, o, &enc));
        // Each pass gets an equal share of the bar.
        let (base, share) = (i as f32 / passes.len() as f32, 1.0 / passes.len() as f32);
        let label = match pass {
            Some(1) => "Pass 1 of 2 · ",
            Some(_) => "Pass 2 of 2 · ",
            None => "",
        };
        rep.progress(Some(base), format!("{label}Starting…"));
        result = run_streaming(cmd, cancel, |line| report_progress(rep, line, duration, base, share, label));
        if result.is_err() {
            break;
        }
    }
    if two_pass {
        let _ = std::fs::remove_dir_all(&log_dir);
    }
    result
}

fn report_progress(rep: &Reporter, line: &str, total: Option<f64>, base: f32, share: f32, label: &str) {
    if let Some(secs) = ffmpeg::parse_progress(line) {
        match total {
            Some(total) => {
                let frac = (secs / total).clamp(0.0, 1.0) as f32;
                let overall = base + frac * share;
                rep.progress(
                    Some(overall),
                    format!(
                        "{label}{:.0}% · {} / {}",
                        overall * 100.0,
                        crate::format::fmt_duration(secs),
                        crate::format::fmt_duration(total)
                    ),
                );
            }
            None => rep.progress(None, format!("{label}{}", crate::format::fmt_duration(secs))),
        }
    } else if line == "progress=end" && base + share >= 0.999 {
        rep.processing("Finishing…");
    }
}

/// What the joiner needs to know about each input.
#[derive(Debug, Default, Clone)]
struct Probe {
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<String>,
    has_video: bool,
    has_audio: bool,
    duration: f64,
}

fn probe(ffprobe: &Path, path: &Path) -> Result<Probe> {
    let out = command(ffprobe)
        .args(["-v", "error", "-show_entries"])
        .arg("stream=codec_type,width,height,r_frame_rate:stream_disposition=attached_pic:format=duration")
        .args(["-of", "json"])
        .arg(path)
        .output()
        .context("failed to run ffprobe")?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("couldn't read {}", path.display()))?;
    let mut p = Probe {
        duration: v["format"]["duration"]
            .as_str()
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.0),
        ..Default::default()
    };
    for s in v["streams"].as_array().into_iter().flatten() {
        match s["codec_type"].as_str() {
            Some("video") if s["disposition"]["attached_pic"].as_i64() != Some(1) && !p.has_video => {
                p.has_video = true;
                p.width = s["width"].as_u64().map(|w| w as u32);
                p.height = s["height"].as_u64().map(|h| h as u32);
                p.fps = s["r_frame_rate"].as_str().filter(|f| *f != "0/0").map(str::to_owned);
            }
            Some("audio") => p.has_audio = true,
            _ => {}
        }
    }
    if p.duration <= 0.0 {
        bail!("{} has no readable duration", path.display());
    }
    Ok(p)
}

/// ffmpeg arguments that join `inputs` (with their probes) into `output`. Video is scaled
/// and padded to the first input's size and frame rate; silent inputs get generated silence.
fn concat_args(inputs: &[(PathBuf, Probe)], target: &str, o: &ConvertOptions, output: &Path) -> Vec<String> {
    let video = !is_audio_target(target);
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]
        .into_iter()
        .map(String::from)
        .collect();
    for (path, _) in inputs {
        a.push("-i".into());
        a.push(path.display().to_string());
    }
    let mut silence = Vec::new();
    for (i, (_, p)) in inputs.iter().enumerate() {
        if p.has_audio {
            silence.push(None);
        } else {
            silence.push(Some(inputs.len() + silence.iter().flatten().count()));
            a.extend(["-f", "lavfi", "-t"].map(String::from));
            a.push(format!("{}", p.duration));
            a.push("-i".into());
            a.push("anullsrc=r=48000:cl=stereo".into());
        }
        let _ = i;
    }

    let first = inputs.iter().map(|(_, p)| p).find(|p| p.has_video);
    let even = |v: u32| (v / 2 * 2).max(2);
    let (w, h) = first
        .and_then(|p| Some((even(p.width?), even(p.height?))))
        .unwrap_or((1280, 720));
    let fps = first.and_then(|p| p.fps.clone()).unwrap_or_else(|| "30".into());

    let mut graph = Vec::new();
    let mut joined = String::new();
    for (i, _) in inputs.iter().enumerate() {
        if video {
            graph.push(format!(
                "[{i}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps},format=yuv420p[v{i}]"
            ));
            joined.push_str(&format!("[v{i}]"));
        }
        let src = silence[i].map_or(format!("{i}:a"), |s| format!("{s}:a"));
        graph.push(format!(
            "[{src}]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}]"
        ));
        joined.push_str(&format!("[a{i}]"));
    }
    let n = inputs.len();
    let (vlabel, alabel) = ("vc", "ac");
    if video {
        graph.push(format!("{joined}concat=n={n}:v=1:a=1[{vlabel}][{alabel}]"));
    } else {
        graph.push(format!("{joined}concat=n={n}:v=0:a=1[{alabel}]"));
    }
    let mut vout = format!("[{vlabel}]");
    let mut vchain = o.filters.video_chain(None);
    if o.max_height > 0 {
        vchain.push(format!("scale=-2:'min({},ih)'", o.max_height));
    }
    if video && !vchain.is_empty() {
        graph.push(format!("[{vlabel}]{}[vout]", vchain.join(",")));
        vout = "[vout]".into();
    }
    let mut aout = format!("[{alabel}]");
    let achain = o.filters.audio_chain();
    if !achain.is_empty() {
        graph.push(format!("[{alabel}]{}[aout]", achain.join(",")));
        aout = "[aout]".into();
    }
    a.push("-filter_complex".into());
    a.push(graph.join(";"));
    if video {
        a.extend(["-map".into(), vout]);
    }
    a.extend(["-map".into(), aout]);

    if video {
        let codec = hwaccel::effective_codec(target, o.codec);
        a.extend(hwaccel::video_encoder_args(codec, o.hardware, Rate::Crf(o.crf), &o.preset));
        if codec == VideoCodec::H265 && target != "mkv" {
            a.extend(["-tag:v", "hvc1"].map(String::from));
        }
        let audio = if target == "webm" { "libopus" } else { "aac" };
        a.extend(["-c:a".into(), audio.into(), "-b:a".into(), format!("{}k", o.audio_bitrate)]);
        if target == "mp4" || target == "mov" {
            a.extend(["-movflags", "+faststart"].map(String::from));
        }
    } else {
        a.extend(ffmpeg::audio_codec_args(target, o.audio_bitrate));
    }
    a.extend(["-progress", "pipe:1", "-nostats"].map(String::from));
    a.push(output.display().to_string());
    a
}

pub fn run_concat(
    tools: &ToolPaths,
    req: &ConcatRequest,
    cancel: &CancelHandle,
    rep: &Reporter,
) -> Result<Option<PathBuf>> {
    let ffmpeg = tools.ffmpeg.as_ref().ok_or_else(|| anyhow!("ffmpeg is not installed"))?;
    let ffprobe = tools.ffprobe.as_ref().ok_or_else(|| anyhow!("ffprobe is not installed"))?;
    if req.inputs.len() < 2 {
        bail!("Joining needs at least two files");
    }
    if !is_audio_target(&req.target) && hwaccel::codecs_for(&req.target).is_empty() {
        bail!("Can't join into {}", req.target.to_uppercase());
    }
    rep.progress(None, "Reading files…");
    let mut probed = Vec::new();
    for path in &req.inputs {
        probed.push((path.clone(), probe(ffprobe, path)?));
    }
    if !is_audio_target(&req.target) && !probed.iter().any(|(_, p)| p.has_video) {
        bail!("None of these files has video; join them into an audio format instead");
    }
    let total = probed.iter().map(|(_, p)| p.duration).sum::<f64>()
        / req.options.filters.speed.max(0.01) as f64;

    let dir = match &req.out_dir {
        Some(d) => d.clone(),
        None => req.inputs[0].parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("can't create {}", dir.display()))?;
    let output = unique_path(&dir, &req.output_stem, &req.target);

    let mut cmd = command(ffmpeg);
    cmd.args(concat_args(&probed, &req.target, &req.options, &output));
    rep.progress(Some(0.0), "Starting…");
    let result = run_streaming(cmd, cancel, |line| report_progress(rep, line, Some(total), 0.0, 1.0, ""));
    match result {
        Ok(()) => {
            if req.recycle_sources && std::fs::metadata(&output).is_ok_and(|m| m.len() > 0) {
                for input in &req.inputs {
                    recycle(input);
                }
            }
            Ok(Some(output))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&output);
            Err(e)
        }
    }
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
    fn renders_file_name_templates() {
        assert_eq!(render_stem("{name}", "clip", 1, "mp4"), "clip");
        assert_eq!(render_stem("", "clip", 1, "mp4"), "clip");
        assert_eq!(render_stem("{n} - {name} ({target})", "a", 3, "webm"), "3 - a (webm)");
        assert_eq!(render_stem("x: y/z?.", "a", 1, "mp4"), "x_ y_z_");
        assert_eq!(render_stem("...", "a", 1, "mp4"), "a");
        assert!(render_stem("{date}", "a", 1, "mp4").starts_with("20"));
    }

    #[test]
    fn builds_concat_graph() {
        let v = |w, h, audio| Probe {
            width: Some(w),
            height: Some(h),
            fps: Some("30/1".into()),
            has_video: true,
            has_audio: audio,
            duration: 5.0,
        };
        let inputs = vec![
            (PathBuf::from("a.mp4"), v(1921, 1080, true)),
            (PathBuf::from("b.mp4"), v(640, 480, false)),
        ];
        let o = ConvertOptions::default();
        let s = concat_args(&inputs, "mp4", &o, Path::new("out.mp4")).join(" ");
        assert!(s.contains("-i a.mp4 -i b.mp4 -f lavfi -t 5 -i anullsrc=r=48000:cl=stereo"));
        assert!(s.contains("[1:v]scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080"));
        assert!(s.contains("[2:a]aresample=48000"), "silence stands in for b's audio");
        assert!(s.contains("[v0][a0][v1][a1]concat=n=2:v=1:a=1[vc][ac]"));
        assert!(s.contains("-map [vc] -map [ac] -c:v libx264"));
        assert!(s.ends_with("out.mp4"));

        let audio = concat_args(&inputs, "mp3", &o, Path::new("o.mp3")).join(" ");
        assert!(audio.contains("[a0][a1]concat=n=2:v=0:a=1[ac]") && !audio.contains("[0:v]"));
        assert!(audio.contains("-c:a libmp3lame"));
    }

    #[test]
    fn default_target_is_offered() {
        for c in [Category::Image, Category::Video, Category::Audio] {
            assert!(c.targets().contains(&c.default_target()));
        }
    }
}
