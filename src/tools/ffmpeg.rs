//! ffmpeg argument building and `-progress` parsing for video/audio conversion.

use std::path::Path;

use super::convert::{ConvertOptions, GifDirection, GifOptions};
use super::filters::SubSource;
use super::hwaccel::{self, Hardware, Rate, VideoCodec};

pub fn is_audio_target(target: &str) -> bool {
    matches!(target, "mp3" | "m4a" | "flac" | "wav" | "ogg" | "opus")
}

pub fn audio_codec_args(target: &str, bitrate_k: u32) -> Vec<String> {
    let br = format!("{bitrate_k}k");
    let v: Vec<&str> = match target {
        "mp3" => vec!["-c:a", "libmp3lame", "-b:a", &br],
        "m4a" => vec!["-c:a", "aac", "-b:a", &br],
        "ogg" => vec!["-c:a", "libvorbis", "-b:a", &br],
        "opus" => vec!["-c:a", "libopus", "-b:a", &br],
        "flac" => vec!["-c:a", "flac"],
        "wav" => vec!["-c:a", "pcm_s16le"],
        _ => vec![],
    };
    v.into_iter().map(String::from).collect()
}

fn scale_filter(max_height: u32) -> Option<String> {
    (max_height > 0).then(|| format!("scale=-2:'min({max_height},ih)'"))
}

/// `-vf` graph for video → GIF: speed, frame rate, size, direction, then a two-pass palette.
fn gif_filter(g: &GifOptions) -> String {
    let mut chain = Vec::new();
    if (g.speed - 1.0).abs() > f32::EPSILON {
        chain.push(format!("setpts=PTS/{}", g.speed));
    }
    chain.push(format!("fps={}", g.fps.max(1)));
    if g.width > 0 {
        chain.push(format!("scale={}:-2:flags=lanczos", g.width));
    }
    if g.direction == GifDirection::Reverse {
        chain.push("reverse".into());
    }
    let mut graph = chain.join(",");
    if g.direction == GifDirection::Boomerang {
        graph.push_str(",split[fw][bw0];[bw0]reverse[bw];[fw][bw]concat=n=2:v=1:a=0");
    }

    let stats = if g.diff_only { "diff" } else { "full" };
    let mut use_opts = format!("dither={}", g.dither);
    if g.dither == "bayer" {
        use_opts.push_str(":bayer_scale=3");
    }
    if g.diff_only {
        use_opts.push_str(":diff_mode=rectangle");
    }
    format!(
        "{graph},split[s0][s1];[s0]palettegen=max_colors={}:stats_mode={stats}[p];[s1][p]paletteuse={use_opts}",
        g.colors.clamp(2, 256)
    )
}

/// `-loop` value: 0 is forever, -1 plays once, n repeats n more times.
fn gif_loop(plays: u32) -> String {
    match plays {
        0 => "0".into(),
        1 => "-1".into(),
        n => (n - 1).to_string(),
    }
}

/// Per-run choices that aren't part of the saved options.
#[derive(Default, Clone, Copy)]
pub struct Encode<'a> {
    /// Average video bitrate in kbit/s (target-size mode) instead of the quality setting.
    pub video_kbps: Option<u32>,
    /// Two-pass encoding: pass number and the pass log prefix.
    pub pass: Option<(u8, &'a Path)>,
    pub subs: Option<&'a SubSource>,
}

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

fn join_chain(mut chain: Vec<String>, last: Option<String>) -> Option<String> {
    chain.extend(last);
    (!chain.is_empty()).then(|| chain.join(","))
}

/// Full ffmpeg argument list for converting `input` into `output` (format `target`).
pub fn build_args(
    input: &Path,
    output: &Path,
    target: &str,
    o: &ConvertOptions,
    enc: &Encode,
) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]
        .into_iter()
        .map(String::from)
        .collect();
    // Clip as input options so the times are in source time and seeking is fast.
    if target == "gif" {
        if o.gif.start > 0.0 {
            a.extend(["-ss".into(), format!("{}", o.gif.start)]);
        }
        if o.gif.length > 0.0 {
            a.extend(["-t".into(), format!("{}", o.gif.length)]);
        }
    }
    a.push("-i".into());
    a.push(input.display().to_string());

    let first_pass = matches!(enc.pass, Some((1, _)));
    let subs = enc.subs.map(|s| s.filter(input));
    let vchain = o.filters.video_chain(subs.as_deref());
    let achain = join_chain(o.filters.audio_chain(), None);
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));

    if is_audio_target(target) {
        push(&["-vn", "-map_metadata", "0"]);
        for s in audio_codec_args(target, o.audio_bitrate) {
            push(&[&s]);
        }
        if let Some(af) = &achain {
            push(&["-af", af]);
        }
    } else {
        let vf = join_chain(vchain, scale_filter(o.max_height));
        match target {
            "mp4" | "mkv" | "mov" | "webm" => {
                let codec = hwaccel::effective_codec(target, o.codec);
                let rate = enc.video_kbps.map_or(Rate::Crf(o.crf), Rate::Bitrate);
                for s in hwaccel::video_encoder_args(codec, o.hardware, rate, &o.preset) {
                    push(&[&s]);
                }
                if let Some((n, log)) = enc.pass {
                    push(&["-pass", &n.to_string(), "-passlogfile", &log.display().to_string()]);
                }
                // Apple players only take HEVC tagged hvc1.
                if codec == VideoCodec::H265 && target != "mkv" {
                    push(&["-tag:v", "hvc1"]);
                }
                if let Some(vf) = &vf {
                    push(&["-vf", vf]);
                }
                if first_pass {
                    push(&["-an"]);
                } else {
                    let audio = if target == "webm" { "libopus" } else { "aac" };
                    push(&["-c:a", audio, "-b:a", &format!("{}k", o.audio_bitrate)]);
                    if let Some(af) = &achain {
                        push(&["-af", af]);
                    }
                }
                if !first_pass && (target == "mp4" || target == "mov") {
                    push(&["-movflags", "+faststart"]);
                }
            }
            "avi" => {
                push(&["-c:v", "mpeg4", "-q:v", "4"]);
                if let Some(vf) = &vf {
                    push(&["-vf", vf]);
                }
                push(&["-c:a", "libmp3lame", "-b:a", &format!("{}k", o.audio_bitrate)]);
                if let Some(af) = &achain {
                    push(&["-af", af]);
                }
            }
            "gif" => {
                // Rotate/crop/flip first; the GIF chain has its own speed and size.
                let gif = match join_chain(o.filters.geometry(), None) {
                    Some(g) => format!("{g},{}", gif_filter(&o.gif)),
                    None => gif_filter(&o.gif),
                };
                push(&["-vf", &gif, "-loop", &gif_loop(o.gif.plays), "-an"]);
            }
            _ => {}
        }
    }

    push(&["-progress", "pipe:1", "-nostats"]);
    if first_pass {
        push(&["-f", "null", NULL_DEVICE]);
    } else {
        a.push(output.display().to_string());
    }
    a
}

/// Whether target-size mode can run two passes with this encoder (others get one
/// constrained-bitrate pass).
pub fn supports_two_pass(target: &str, o: &ConvertOptions) -> bool {
    o.hardware == Hardware::Cpu
        && matches!(
            hwaccel::effective_codec(target, o.codec),
            VideoCodec::H264 | VideoCodec::Vp9
        )
}

/// Video bitrate that lands a file of `duration` seconds near `size_mb` (decimal MB, so
/// it stays under limits like Discord's), after the audio's share.
pub fn target_video_kbps(size_mb: f32, duration: f64, audio_kbps: u32) -> u32 {
    let total = size_mb as f64 * 8000.0 * 0.96 / duration.max(0.1);
    (total - audio_kbps as f64).max(100.0) as u32
}

/// Parses one line of `-progress pipe:1` output into seconds processed.
/// (`out_time_ms` is, despite its name, microseconds, same as `out_time_us`.)
pub fn parse_progress(line: &str) -> Option<f64> {
    let (key, value) = line.split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => value.trim().parse::<f64>().ok().map(|us| us / 1_000_000.0),
        _ => None,
    }
}

/// Media duration in seconds via ffprobe.
pub fn probe_duration(ffprobe: &Path, input: &Path) -> Option<f64> {
    let out = super::command(ffprobe)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(input)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| *d > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_progress() {
        assert_eq!(parse_progress("out_time_us=1500000"), Some(1.5));
        assert_eq!(parse_progress("out_time_ms=2000000"), Some(2.0));
        assert_eq!(parse_progress("out_time_us=N/A"), None);
        assert_eq!(parse_progress("progress=continue"), None);
        assert_eq!(parse_progress("garbage"), None);
    }

    #[test]
    fn mp4_args() {
        let o = ConvertOptions {
            crf: 20,
            max_height: 720,
            ..Default::default()
        };
        let a = build_args(&PathBuf::from("in.mov"), &PathBuf::from("out.mp4"), "mp4", &o, &Encode::default());
        let s = a.join(" ");
        assert!(s.contains("-c:v libx264"));
        assert!(s.contains("-crf 20"));
        assert!(s.contains("scale=-2:'min(720,ih)'"));
        assert!(s.contains("-movflags +faststart"));
        assert_eq!(a.last().unwrap(), "out.mp4");
    }

    #[test]
    fn gif_args() {
        let mut o = ConvertOptions::default();
        let s = build_args(&PathBuf::from("in.mp4"), &PathBuf::from("o.gif"), "gif", &o, &Encode::default()).join(" ");
        assert!(s.contains("fps=15,scale=480:-2:flags=lanczos,split[s0][s1]"));
        assert!(s.contains("palettegen=max_colors=256:stats_mode=full"));
        assert!(s.contains("paletteuse=dither=sierra2_4a"));
        assert!(s.contains("-loop 0"));
        assert!(!s.contains("-ss") && !s.contains("setpts"));

        o.gif = GifOptions {
            fps: 10,
            width: 0,
            colors: 64,
            dither: "bayer".into(),
            diff_only: true,
            plays: 3,
            speed: 2.0,
            direction: GifDirection::Boomerang,
            start: 1.5,
            length: 4.0,
        };
        let a = build_args(&PathBuf::from("in.mp4"), &PathBuf::from("o.gif"), "gif", &o, &Encode::default());
        let s = a.join(" ");
        assert!(s.starts_with("-hide_banner -nostdin -y -loglevel error -ss 1.5 -t 4 -i in.mp4"));
        assert!(s.contains("setpts=PTS/2,fps=10,split[fw][bw0];[bw0]reverse[bw];[fw][bw]concat"));
        assert!(!s.contains("flags=lanczos"));
        assert!(s.contains("max_colors=64:stats_mode=diff"));
        assert!(s.contains("dither=bayer:bayer_scale=3:diff_mode=rectangle"));
        assert!(s.contains("-loop 2"));

        o.gif.plays = 1;
        o.gif.direction = GifDirection::Reverse;
        let s = build_args(&PathBuf::from("in.mp4"), &PathBuf::from("o.gif"), "gif", &o, &Encode::default()).join(" ");
        assert!(s.contains("fps=10,reverse,split[s0][s1]"));
        assert!(s.contains("-loop -1"));
    }

    #[test]
    fn filters_two_pass_and_size_target() {
        use crate::tools::filters::{Filters, Rotation};
        let o = ConvertOptions {
            max_height: 720,
            filters: Filters {
                rotate: Rotation::Right,
                speed: 2.0,
                normalize: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let sub = SubSource::File(PathBuf::from("s.srt"));
        let log = PathBuf::from("log");
        let enc = Encode {
            video_kbps: Some(800),
            pass: Some((2, &log)),
            subs: Some(&sub),
        };
        let s = build_args(&PathBuf::from("in.mp4"), &PathBuf::from("o.mp4"), "mp4", &o, &enc).join(" ");
        assert!(s.contains("-b:v 800k") && !s.contains("-crf"));
        assert!(s.contains("-pass 2 -passlogfile log"));
        assert!(s.contains("-vf transpose=1,subtitles=s.srt,setpts=PTS/2,scale=-2:'min(720,ih)'"));
        assert!(s.contains("-af atempo=2,loudnorm"));
        assert!(s.ends_with("o.mp4"));

        let p1 = Encode { pass: Some((1, &log)), ..enc };
        let a = build_args(&PathBuf::from("in.mp4"), &PathBuf::from("o.mp4"), "mp4", &o, &p1);
        let s = a.join(" ");
        assert!(s.contains("-pass 1") && s.contains("-an") && !s.contains("-c:a"));
        assert_eq!(&a[a.len() - 3..], ["-f", "null", NULL_DEVICE]);

        let gif = build_args(&PathBuf::from("in.mp4"), &PathBuf::from("o.gif"), "gif", &o, &Encode::default()).join(" ");
        assert!(gif.contains("-vf transpose=1,fps=15"), "{gif}");

        let mp3 = build_args(&PathBuf::from("in.mp4"), &PathBuf::from("o.mp3"), "mp3", &o, &Encode::default()).join(" ");
        assert!(mp3.contains("-af atempo=2,loudnorm"));

        // 10 MB over 60 s with 128 kbps audio.
        assert_eq!(target_video_kbps(10.0, 60.0, 128), 1152);
        assert_eq!(target_video_kbps(1.0, 3600.0, 128), 100, "floor");
        assert!(supports_two_pass("mp4", &ConvertOptions::default()));
        assert!(!supports_two_pass("mp4", &ConvertOptions { hardware: Hardware::Nvidia, ..Default::default() }));
    }

    #[test]
    fn audio_from_video_drops_video() {
        let o = ConvertOptions {
            audio_bitrate: 256,
            ..Default::default()
        };
        let a = build_args(&PathBuf::from("in.mp4"), &PathBuf::from("out.mp3"), "mp3", &o, &Encode::default());
        let s = a.join(" ");
        assert!(s.contains("-vn"));
        assert!(s.contains("-c:a libmp3lame -b:a 256k"));
        let flac = build_args(&PathBuf::from("in.wav"), &PathBuf::from("o.flac"), "flac", &o, &Encode::default()).join(" ");
        assert!(flac.contains("-c:a flac") && !flac.contains("-b:a"));
    }
}
