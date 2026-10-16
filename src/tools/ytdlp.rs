//! Driving yt-dlp: metadata lookup, argument building and progress parsing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use super::{ToolPaths, command, run_streaming};
use crate::jobs::{AlreadyDownloaded, CancelHandle, Reporter};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Video,
    Audio,
}

pub const VIDEO_QUALITIES: &[(u32, &str)] = &[
    (0, "Best available"),
    (2160, "2160p (4K)"),
    (1440, "1440p"),
    (1080, "1080p"),
    (720, "720p"),
    (480, "480p"),
    (360, "360p"),
];
pub const CONTAINERS: &[&str] = &["mp4", "mkv", "webm"];
pub const AUDIO_FORMATS: &[&str] = &["mp3", "m4a", "opus", "flac", "wav"];
/// `0` is yt-dlp's "best VBR".
pub const AUDIO_BITRATES: &[(&str, &str)] = &[
    ("0", "Best (VBR)"),
    ("320K", "320 kbps"),
    ("256K", "256 kbps"),
    ("192K", "192 kbps"),
    ("128K", "128 kbps"),
];

pub fn is_lossless(audio_format: &str) -> bool {
    matches!(audio_format, "flac" | "wav")
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct DownloadOptions {
    pub mode: Mode,
    pub max_height: u32,
    pub container: String,
    pub audio_format: String,
    pub audio_bitrate: String,
    pub embed_thumbnail: bool,
    pub embed_metadata: bool,
    pub embed_subs: bool,
    pub playlist: bool,
    /// Cut sponsor/self-promo segments using SponsorBlock (YouTube only).
    pub remove_sponsors: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            mode: Mode::Video,
            max_height: 1080,
            container: "mp4".into(),
            audio_format: "mp3".into(),
            audio_bitrate: "0".into(),
            embed_thumbnail: true,
            embed_metadata: true,
            embed_subs: false,
            playlist: false,
            remove_sponsors: false,
        }
    }
}

impl DownloadOptions {
    pub fn summary(&self) -> String {
        match self.mode {
            Mode::Video => {
                let q = if self.max_height == 0 {
                    "Best".to_owned()
                } else {
                    format!("{}p", self.max_height)
                };
                format!("Video · {} · {q}", self.container.to_uppercase())
            }
            Mode::Audio => {
                let fmt = self.audio_format.to_uppercase();
                if is_lossless(&self.audio_format) {
                    format!("Audio · {fmt}")
                } else {
                    let br = AUDIO_BITRATES
                        .iter()
                        .find(|(v, _)| *v == self.audio_bitrate)
                        .map_or(self.audio_bitrate.as_str(), |(_, l)| l);
                    format!("Audio · {fmt} · {br}")
                }
            }
        }
    }
}

/// Only download part of the media. Times are in seconds; `end: None` runs to the end.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Trim {
    pub start: f64,
    pub end: Option<f64>,
    /// Re-encode around the cut points so they land exactly, instead of on the nearest keyframe.
    pub precise: bool,
}

impl Trim {
    /// `*START-END` for `--download-sections`.
    fn section(&self) -> String {
        let end = self.end.map_or("inf".to_owned(), |e| format!("{e}"));
        format!("*{}-{end}", self.start)
    }

    /// Filename-safe label like `0m05s-1m20s`, so different clips don't overwrite each other.
    pub fn label(&self) -> String {
        let t = |s: f64| {
            let s = s.max(0.0).round() as u64;
            match s / 3600 {
                0 => format!("{}m{:02}s", s / 60, s % 60),
                h => format!("{h}h{:02}m{:02}s", (s / 60) % 60, s % 60),
            }
        };
        let end = self.end.map_or("end".to_owned(), t);
        format!("{}-{end}", t(self.start))
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadRequest {
    pub url: String,
    pub options: DownloadOptions,
    pub out_dir: PathBuf,
    pub trim: Option<Trim>,
    /// yt-dlp `--download-archive` file; links recorded there are skipped.
    pub archive: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub is_playlist: bool,
    pub entries: usize,
    pub extractor: Option<String>,
}

#[derive(Deserialize)]
struct RawInfo {
    #[serde(rename = "_type")]
    kind: Option<String>,
    title: Option<String>,
    uploader: Option<String>,
    channel: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    #[serde(default)]
    thumbnails: Vec<RawThumb>,
    #[serde(default)]
    entries: Vec<serde_json::Value>,
    playlist_count: Option<usize>,
    extractor_key: Option<String>,
}

#[derive(Deserialize)]
struct RawThumb {
    url: String,
}

fn base_command(tools: &ToolPaths) -> Result<std::process::Command> {
    let ytdlp = tools
        .ytdlp
        .as_ref()
        .ok_or_else(|| anyhow!("yt-dlp is not installed"))?;
    let mut cmd = command(ytdlp);
    cmd.env("PYTHONIOENCODING", "utf-8")
        .args(["--encoding", "utf-8", "--no-warnings", "--ignore-config"]);
    if let Some(dir) = tools.ffmpeg.as_ref().and_then(|f| f.parent()) {
        cmd.arg("--ffmpeg-location").arg(dir);
    }
    if let Some(deno) = &tools.deno {
        cmd.arg("--js-runtimes")
            .arg(format!("deno:{}", deno.display()));
    }
    Ok(cmd)
}

/// `yt-dlp -J --flat-playlist URL`, parsed into the bits the UI shows.
pub fn fetch_info(tools: &ToolPaths, url: &str) -> Result<MediaInfo> {
    let mut cmd = base_command(tools)?;
    cmd.args(["-J", "--flat-playlist", "--", url]);
    let out = cmd.output().context("failed to run yt-dlp")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .rev()
            .find(|l| l.starts_with("ERROR:"))
            .or_else(|| err.lines().rev().find(|l| !l.trim().is_empty()))
            .unwrap_or("yt-dlp failed")
            .trim_start_matches("ERROR:")
            .trim();
        bail!("{}", friendly_error(line));
    }
    parse_info(&out.stdout)
}

fn parse_info(json: &[u8]) -> Result<MediaInfo> {
    let raw: RawInfo = serde_json::from_slice(json).context("unexpected yt-dlp output")?;
    let is_playlist = raw.kind.as_deref() == Some("playlist");
    Ok(MediaInfo {
        title: raw.title.unwrap_or_else(|| "Untitled".into()),
        uploader: raw.uploader.or(raw.channel),
        duration: raw.duration,
        thumbnail: raw
            .thumbnail
            .or_else(|| raw.thumbnails.last().map(|t| t.url.clone())),
        is_playlist,
        entries: raw.playlist_count.unwrap_or(raw.entries.len()),
        extractor: raw.extractor_key,
    })
}

/// Adds a hint to errors whose cause isn't obvious from yt-dlp's wording.
pub fn friendly_error(msg: &str) -> String {
    let lower = msg.to_lowercase();
    if lower.contains("js runtime") || lower.contains("javascript runtime") || lower.contains("n challenge") {
        format!("{msg}\n\nYouTube needs a JavaScript runtime. Install Deno from Settings > Tools.")
    } else if lower.contains("sign in to confirm") {
        format!("{msg}\n\nYouTube is rate-limiting this network. Try again later.")
    } else if lower.contains("unsupported url") {
        "That link isn't from a supported site.".to_owned()
    } else {
        msg.to_owned()
    }
}

pub fn build_args(req: &DownloadRequest) -> Vec<String> {
    let o = &req.options;
    let mut a: Vec<String> = Vec::new();
    let mut push = |s: &str| a.push(s.to_owned());

    push("--newline");
    push("--progress");
    push("--progress-template");
    push("download:RL|%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s");
    push("--progress-template");
    push("postprocess:RLPP|%(progress.postprocessor)s");
    push("--print");
    push("before_dl:RLTITLE|%(title)s");
    push("--print");
    push("after_move:RLFILE|%(filepath)s");
    push("--no-mtime");
    push(if o.playlist { "--yes-playlist" } else { "--no-playlist" });

    match o.mode {
        Mode::Video => {
            push("-f");
            push("bv*+ba/b");
            let mut sort = Vec::new();
            if o.max_height > 0 {
                sort.push(format!("res:{}", o.max_height));
            }
            match o.container.as_str() {
                // h264/aac keeps mp4 files playable everywhere.
                "mp4" => sort.extend(["vcodec:h264".into(), "acodec:m4a".into()]),
                "webm" => sort.extend(["vcodec:vp9".into(), "acodec:opus".into()]),
                _ => {}
            }
            if !sort.is_empty() {
                push("-S");
                push(&sort.join(","));
            }
            push("--merge-output-format");
            push(&o.container);
            if o.embed_subs {
                push("--embed-subs");
                push("--sub-langs");
                push("en.*,en");
            }
            if o.embed_thumbnail && o.container != "webm" {
                push("--embed-thumbnail");
            }
        }
        Mode::Audio => {
            push("-f");
            push("ba/b");
            push("-x");
            push("--audio-format");
            push(&o.audio_format);
            if !is_lossless(&o.audio_format) {
                push("--audio-quality");
                push(&o.audio_bitrate);
            }
            if o.embed_thumbnail && o.audio_format != "wav" {
                push("--embed-thumbnail");
            }
        }
    }
    if o.embed_metadata {
        push("--embed-metadata");
    }
    if o.remove_sponsors {
        push("--sponsorblock-remove");
        push("sponsor,selfpromo,interaction");
    }
    if let Some(archive) = &req.archive {
        a.push("--download-archive".into());
        a.push(archive.display().to_string());
    }
    let mut push = |s: &str| a.push(s.to_owned());
    if let Some(t) = &req.trim {
        push("--download-sections");
        push(&t.section());
        if t.precise {
            push("--force-keyframes-at-cuts");
        }
    }

    push("-P");
    a.push(req.out_dir.display().to_string());
    a.push("-o".into());
    a.push(match (&req.trim, o.playlist) {
        (_, true) => {
            "%(playlist_title,playlist|Playlist)s/%(playlist_index|0)03d - %(title)s.%(ext)s".into()
        }
        (Some(t), false) => format!("%(title)s [%(id)s] {}.%(ext)s", t.label()),
        (None, false) => "%(title)s [%(id)s].%(ext)s".into(),
    });
    a.push("--".into());
    a.push(req.url.clone());
    a
}

#[derive(Debug, PartialEq)]
pub enum Line {
    Progress { percent: Option<f32>, speed: String, eta: String },
    PostProcess(String),
    Title(String),
    File(PathBuf),
    /// An item was skipped because it's in the download archive.
    Archived,
    Other,
}

pub fn parse_line(line: &str) -> Line {
    if let Some(rest) = line.strip_prefix("RL|") {
        let mut parts = rest.split('|').map(str::trim);
        let pct = parts.next().unwrap_or("");
        let speed = parts.next().unwrap_or("").to_owned();
        let eta = parts.next().unwrap_or("").to_owned();
        let percent = pct
            .trim_end_matches('%')
            .trim()
            .parse::<f32>()
            .ok()
            .map(|p| p / 100.0);
        let clean = |s: String| if s == "NA" || s.contains("Unknown") { String::new() } else { s };
        Line::Progress {
            percent,
            speed: clean(speed),
            eta: clean(eta),
        }
    } else if let Some(rest) = line.strip_prefix("RLPP|") {
        Line::PostProcess(rest.trim().to_owned())
    } else if let Some(rest) = line.strip_prefix("RLTITLE|") {
        Line::Title(rest.trim().to_owned())
    } else if let Some(rest) = line.strip_prefix("RLFILE|") {
        Line::File(PathBuf::from(rest.trim()))
    } else if line.contains("has already been recorded in the archive") {
        Line::Archived
    } else {
        Line::Other
    }
}

fn postprocessor_label(name: &str) -> String {
    match name {
        "Merger" => "Merging video and audio…".into(),
        "ExtractAudio" => "Extracting audio…".into(),
        "EmbedThumbnail" => "Embedding cover art…".into(),
        "Metadata" | "FFmpegMetadata" => "Writing metadata…".into(),
        "EmbedSubtitle" => "Embedding subtitles…".into(),
        "MoveFiles" => "Finishing…".into(),
        "SponsorBlock" | "ModifyChapters" => "Removing sponsor segments…".into(),
        other => format!("Processing ({other})…"),
    }
}

pub fn download(
    tools: &ToolPaths,
    req: &DownloadRequest,
    cancel: &CancelHandle,
    rep: &Reporter,
) -> Result<Option<PathBuf>> {
    std::fs::create_dir_all(&req.out_dir)
        .with_context(|| format!("can't create {}", req.out_dir.display()))?;
    let mut cmd = base_command(tools)?;
    cmd.args(build_args(req));

    let mut last_file: Option<PathBuf> = None;
    let mut items = 0usize;
    let mut archived = 0usize;
    run_streaming(cmd, cancel, |line| match parse_line(line) {
        Line::Progress { percent, speed, eta } => {
            let mut detail = Vec::new();
            if let Some(p) = percent {
                detail.push(format!("{:.0}%", p * 100.0));
            }
            if !speed.is_empty() {
                detail.push(speed);
            }
            if !eta.is_empty() {
                detail.push(format!("{eta} left"));
            }
            if req.options.playlist && items > 0 {
                detail.insert(0, format!("Item {items}"));
            }
            rep.progress(percent, detail.join(" · "));
        }
        Line::PostProcess(name) => rep.processing(postprocessor_label(&name)),
        Line::Title(t) => {
            items += 1;
            rep.title(t);
        }
        Line::File(p) => last_file = Some(p),
        Line::Archived => archived += 1,
        Line::Other => {}
    })
    .map_err(|e| {
        if e.is::<crate::jobs::Cancelled>() {
            e
        } else {
            anyhow!(friendly_error(&format!("{e:#}")))
        }
    })?;

    if last_file.is_none() && archived > 0 {
        return Err(AlreadyDownloaded.into());
    }
    // For playlists, point "open" at the playlist folder rather than the last track.
    Ok(match (req.options.playlist && items > 1, last_file) {
        (true, Some(f)) => f.parent().map(Path::to_path_buf),
        (_, f) => f,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_lines() {
        assert_eq!(
            parse_line("RL| 62.5%|   2.10MiB/s|00:12"),
            Line::Progress {
                percent: Some(0.625),
                speed: "2.10MiB/s".into(),
                eta: "00:12".into()
            }
        );
        assert_eq!(
            parse_line("RL|  N/A|Unknown B/s|NA"),
            Line::Progress {
                percent: None,
                speed: String::new(),
                eta: String::new()
            }
        );
        assert_eq!(parse_line("RLPP|Merger"), Line::PostProcess("Merger".into()));
        assert_eq!(parse_line("RLTITLE|Some song"), Line::Title("Some song".into()));
        assert_eq!(
            parse_line("RLFILE|C:\\out\\a [x].mp4"),
            Line::File(PathBuf::from("C:\\out\\a [x].mp4"))
        );
        assert_eq!(parse_line("[youtube] abc: Downloading webpage"), Line::Other);
        assert_eq!(
            parse_line("[download] Song has already been recorded in the archive"),
            Line::Archived
        );
    }

    fn req(options: DownloadOptions) -> DownloadRequest {
        DownloadRequest {
            url: "https://example.com/v".into(),
            options,
            out_dir: PathBuf::from("out"),
            ..Default::default()
        }
    }

    #[test]
    fn archive_args() {
        let mut r = req(DownloadOptions::default());
        assert!(!build_args(&r).contains(&"--download-archive".to_string()));
        r.archive = Some(PathBuf::from("arch.txt"));
        assert!(has_pair(&build_args(&r), "--download-archive", "arch.txt"));
    }

    fn has_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    #[test]
    fn video_args() {
        let args = build_args(&req(DownloadOptions {
            max_height: 720,
            container: "mp4".into(),
            ..Default::default()
        }));
        assert!(has_pair(&args, "-S", "res:720,vcodec:h264,acodec:m4a"));
        assert!(has_pair(&args, "--merge-output-format", "mp4"));
        assert!(args.contains(&"--no-playlist".to_string()));
        assert_eq!(args.last().unwrap(), "https://example.com/v");
        assert!(!args.contains(&"-x".to_string()));
    }

    #[test]
    fn audio_args() {
        let args = build_args(&req(DownloadOptions {
            mode: Mode::Audio,
            audio_format: "mp3".into(),
            audio_bitrate: "192K".into(),
            ..Default::default()
        }));
        assert!(args.contains(&"-x".to_string()));
        assert!(has_pair(&args, "--audio-format", "mp3"));
        assert!(has_pair(&args, "--audio-quality", "192K"));

        let flac = build_args(&req(DownloadOptions {
            mode: Mode::Audio,
            audio_format: "flac".into(),
            ..Default::default()
        }));
        assert!(!flac.contains(&"--audio-quality".to_string()));
    }

    #[test]
    fn trim_args() {
        let mut r = req(DownloadOptions::default());
        r.trim = Some(Trim {
            start: 5.0,
            end: Some(80.5),
            precise: true,
        });
        let args = build_args(&r);
        assert!(has_pair(&args, "--download-sections", "*5-80.5"));
        assert!(args.contains(&"--force-keyframes-at-cuts".to_string()));
        assert!(has_pair(&args, "-o", "%(title)s [%(id)s] 0m05s-1m21s.%(ext)s"));

        r.trim = Some(Trim {
            start: 3700.0,
            end: None,
            precise: false,
        });
        let args = build_args(&r);
        assert!(has_pair(&args, "--download-sections", "*3700-inf"));
        assert!(!args.contains(&"--force-keyframes-at-cuts".to_string()));
        assert!(has_pair(&args, "-o", "%(title)s [%(id)s] 1h01m40s-end.%(ext)s"));

        assert!(!build_args(&req(DownloadOptions::default())).contains(&"--download-sections".to_string()));
    }

    #[test]
    fn parses_info_json() {
        let info = parse_info(
            br#"{"_type":"video","title":"T","channel":"C","duration":61.0,"thumbnails":[{"url":"a"},{"url":"b"}]}"#,
        )
        .unwrap();
        assert_eq!(info.title, "T");
        assert_eq!(info.uploader.as_deref(), Some("C"));
        assert_eq!(info.thumbnail.as_deref(), Some("b"));
        assert!(!info.is_playlist);

        let pl = parse_info(br#"{"_type":"playlist","title":"P","entries":[{},{},{}]}"#).unwrap();
        assert!(pl.is_playlist);
        assert_eq!(pl.entries, 3);
    }
}
