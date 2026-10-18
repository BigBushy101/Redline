//! Driving yt-dlp: metadata lookup, argument building and progress parsing.

use std::collections::HashMap;
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

/// English and its regional variants, plus YouTube's original-language auto captions.
/// (`en.*` would also match dozens of machine translations like `en-de`.)
pub const DEFAULT_SUB_LANGS: &str = "en,en-US,en-GB,en-orig";

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
    /// yt-dlp `--sub-langs` list, comma separated (used when `embed_subs` is on).
    pub sub_langs: String,
    /// Fall back to auto-generated captions.
    pub auto_subs: bool,
    pub subs_mode: SubsMode,
    /// Also save each chapter as its own file.
    pub split_chapters: bool,
    /// Crop embedded cover art to a square, like album art.
    pub square_cover: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SubsMode {
    /// Inside the video file.
    #[default]
    Embed,
    /// Separate .srt files next to the download.
    Srt,
}

/// Tags written into a downloaded audio file, replacing what the site provided.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
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
            sub_langs: DEFAULT_SUB_LANGS.into(),
            auto_subs: false,
            subs_mode: SubsMode::Embed,
            split_chapters: false,
            square_cover: false,
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

/// Browsers yt-dlp can read cookies from, with display names.
pub const COOKIE_BROWSERS: &[(&str, &str)] = &[
    ("firefox", "Firefox"),
    ("chrome", "Chrome"),
    ("edge", "Edge"),
    ("brave", "Brave"),
    ("opera", "Opera"),
    ("vivaldi", "Vivaldi"),
    ("chromium", "Chromium"),
];

/// Sign-in and connection settings applied to every yt-dlp call.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct NetworkOptions {
    /// `--cookies-from-browser` value, e.g. `firefox`.
    pub cookies_browser: Option<String>,
    /// Netscape-format cookies.txt; used when no browser is picked.
    pub cookies_file: Option<PathBuf>,
    /// e.g. `socks5://127.0.0.1:1080`; empty for none.
    pub proxy: String,
    /// e.g. `2M` or `500K`; empty for unlimited.
    pub rate_limit: String,
    /// 0 keeps yt-dlp's default.
    pub retries: u32,
}

impl NetworkOptions {
    pub fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(b) = self.cookies_browser.as_deref().filter(|b| !b.is_empty()) {
            a.extend(["--cookies-from-browser".into(), b.to_owned()]);
        } else if let Some(f) = &self.cookies_file {
            a.extend(["--cookies".into(), f.display().to_string()]);
        }
        if !self.proxy.trim().is_empty() {
            a.extend(["--proxy".into(), self.proxy.trim().to_owned()]);
        }
        if !self.rate_limit.trim().is_empty() {
            a.extend(["-r".into(), self.rate_limit.trim().to_owned()]);
        }
        if self.retries > 0 {
            a.extend(["--retries".into(), self.retries.to_string()]);
        }
        a
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
    pub network: NetworkOptions,
    /// Exact yt-dlp format selector (e.g. `137+140`) picked from the lookup; overrides quality.
    pub format: Option<String>,
    pub tags: Option<Tags>,
}

#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub is_playlist: bool,
    pub entries: usize,
    /// A playlist's entries, as far as the flat listing tells.
    pub items: Vec<PlaylistItem>,
    pub extractor: Option<String>,
    pub formats: Vec<FormatInfo>,
    /// Languages with uploaded subtitles.
    pub subtitle_langs: Vec<String>,
    /// Whether auto-generated captions exist.
    pub has_auto_captions: bool,
    pub chapters: usize,
    pub track: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// An entry of a playlist lookup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaylistItem {
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
}

impl PlaylistItem {
    /// Reads a `--flat-playlist` entry. `None` if it has no usable link.
    fn from_flat(e: &serde_json::Value) -> Option<Self> {
        let s = |k: &str| e[k].as_str().map(str::to_owned).filter(|v| !v.is_empty());
        let url = s("webpage_url")
            .or_else(|| s("url"))
            .filter(|u| u.starts_with("http"))
            .or_else(|| {
                // YouTube's flat entries sometimes carry only the video id.
                (s("ie_key").as_deref() == Some("Youtube"))
                    .then(|| s("id").map(|id| format!("https://www.youtube.com/watch?v={id}")))
                    .flatten()
            })?;
        Some(Self {
            title: s("title")
                .or_else(|| title_from_link(&url))
                .unwrap_or_else(|| url.clone()),
            // SoundCloud's set entries name the artist only as the album's.
            uploader: s("uploader").or_else(|| s("channel")).or_else(|| s("album_artist")),
            duration: e["duration"].as_f64(),
            thumbnail: e["thumbnails"]
                .as_array()
                .and_then(|t| t.last())
                .and_then(|t| t["url"].as_str())
                .map(str::to_owned),
            url,
        })
    }
}

/// A readable stand-in title for a SoundCloud link whose listing gave none, made from its
/// slug: `…/forss/city-ports` → "City Ports". Resolving real titles would take a lookup per
/// track (about a second each), too slow for big sets and likes.
pub fn title_from_link(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?.split("://").nth(1)?;
    let mut segments = path.trim_end_matches('/').split('/');
    // Not `api-v2.soundcloud.com/tracks/ID`, the stubs that follow a set's first few tracks.
    if !matches!(segments.next()?, "soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com") {
        return None;
    }
    // Only `artist/track` links: sets, likes and short links don't name a track.
    let (Some(_artist), Some(slug), None) = (segments.next(), segments.next(), segments.next()) else {
        return None;
    };
    let words: Vec<String> = slug
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Most untitled items one lookup names. SoundCloud resolves about a track a second per
/// process, so this is roughly half a minute; the rest keep their links until a later sync.
pub const TITLE_LOOKUP_LIMIT: usize = 100;

/// [`fetch_info`], then looks up the titles of items the listing left untitled (SoundCloud
/// sets past their first few tracks), skipping links `known` already has names for.
pub fn fetch_titled(
    tools: &ToolPaths,
    net: &NetworkOptions,
    url: &str,
    known: &(dyn Fn(&str) -> bool + Sync),
) -> Result<MediaInfo> {
    let mut info = fetch_info(tools, net, url)?;
    fill_titles(tools, net, &mut info.items, known, TITLE_LOOKUP_LIMIT);
    Ok(info)
}

/// Names up to `limit` items whose title is still their link, in a few parallel yt-dlp
/// runs. Tracks that fail to resolve keep their link.
pub fn fill_titles(
    tools: &ToolPaths,
    net: &NetworkOptions,
    items: &mut [PlaylistItem],
    known: &(dyn Fn(&str) -> bool + Sync),
    limit: usize,
) {
    let urls: Vec<String> = items
        .iter()
        .filter(|i| i.title == i.url && !known(&i.url))
        .map(|i| i.url.clone())
        .take(limit)
        .collect();
    if urls.is_empty() {
        return;
    }
    let found: HashMap<String, Resolved> = std::thread::scope(|s| {
        let runs: Vec<_> = urls
            .chunks(urls.len().div_ceil(4))
            .map(|chunk| {
                s.spawn(move || {
                    let mut cmd = base_command(tools, net).ok()?;
                    cmd.args(["-i", "--print", RESOLVE_TEMPLATE, "--"]).args(chunk);
                    // A failed track fails the run but still prints the others.
                    let out = cmd.output().ok()?;
                    Some(parse_resolved(&String::from_utf8_lossy(&out.stdout)))
                })
            })
            .collect();
        runs.into_iter().filter_map(|r| r.join().ok().flatten()).flatten().collect()
    });
    for item in items {
        if let Some(r) = found.get(&item.url) {
            item.title = r.title.clone();
            item.uploader = item.uploader.take().or_else(|| r.uploader.clone());
            item.duration = item.duration.or(r.duration);
        }
    }
}

const RESOLVE_TEMPLATE: &str = "%(original_url)s\t%(uploader)s\t%(duration)s\t%(title)s";

#[derive(Debug, PartialEq)]
struct Resolved {
    title: String,
    uploader: Option<String>,
    duration: Option<f64>,
}

/// Reads [`RESOLVE_TEMPLATE`] lines, keyed by the link that was asked about.
fn parse_resolved(out: &str) -> HashMap<String, Resolved> {
    let field = |v: &str| (!v.is_empty() && v != "NA").then(|| v.to_owned());
    out.lines()
        .filter_map(|l| {
            let mut f = l.trim_end_matches('\r').splitn(4, '\t');
            let (url, uploader, duration, title) = (f.next()?, f.next()?, f.next()?, field(f.next()?)?);
            Some((
                url.to_owned(),
                Resolved {
                    title,
                    uploader: field(uploader),
                    duration: duration.parse().ok(),
                },
            ))
        })
        .collect()
}

/// One downloadable stream from the lookup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatInfo {
    pub id: String,
    pub ext: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f32>,
    /// `None` when the format has no video.
    pub vcodec: Option<String>,
    /// `None` when the format has no audio.
    pub acodec: Option<String>,
    /// Total bitrate in kbit/s.
    pub tbr: Option<f64>,
    /// Exact or approximate size in bytes.
    pub size: Option<u64>,
    pub note: Option<String>,
}

impl FormatInfo {
    pub fn has_video(&self) -> bool {
        self.vcodec.is_some()
    }

    pub fn has_audio(&self) -> bool {
        self.acodec.is_some()
    }

    /// Size in bytes, estimated from the bitrate when the site doesn't say.
    pub fn estimated_size(&self, duration: Option<f64>) -> Option<u64> {
        self.size
            .or_else(|| Some((self.tbr? * 1000.0 / 8.0 * duration?) as u64))
    }

    /// e.g. `1080p60`, or the format note for audio.
    pub fn quality_label(&self) -> String {
        match self.height {
            Some(h) if self.has_video() => match self.fps {
                Some(f) if f > 30.5 => format!("{h}p{}", f.round()),
                _ => format!("{h}p"),
            },
            _ => match self.tbr {
                Some(t) => format!("{t:.0} kbps"),
                None => self.note.clone().unwrap_or_default(),
            },
        }
    }
}

/// A quality the user can pick: one row per distinct video height.
#[derive(Clone, Debug, PartialEq)]
pub struct HeightChoice {
    pub height: u32,
    pub fps: Option<f32>,
    /// Best video at this height plus the best audio.
    pub size: Option<u64>,
}

/// Distinct video heights, tallest first, with the size yt-dlp's pick would roughly have.
pub fn height_choices(formats: &[FormatInfo], duration: Option<f64>) -> Vec<HeightChoice> {
    let audio = best_audio_size(formats, duration);
    let mut heights: Vec<u32> = formats
        .iter()
        .filter(|f| f.has_video())
        .filter_map(|f| f.height)
        .filter(|h| *h > 0)
        .collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    heights
        .into_iter()
        .map(|h| {
            let at: Vec<&FormatInfo> = formats
                .iter()
                .filter(|f| f.has_video() && f.height == Some(h))
                .collect();
            let best = at
                .iter()
                .max_by(|a, b| a.tbr.unwrap_or(0.0).total_cmp(&b.tbr.unwrap_or(0.0)));
            let video = best.and_then(|f| f.estimated_size(duration));
            let needs_audio = best.is_some_and(|f| !f.has_audio());
            HeightChoice {
                height: h,
                fps: at.iter().filter_map(|f| f.fps).reduce(f32::max),
                size: match (video, needs_audio) {
                    (Some(v), true) => Some(v + audio.unwrap_or(0)),
                    (v, _) => v,
                },
            }
        })
        .collect()
}

pub fn best_audio_size(formats: &[FormatInfo], duration: Option<f64>) -> Option<u64> {
    formats
        .iter()
        .filter(|f| f.has_audio() && !f.has_video())
        .max_by(|a, b| a.tbr.unwrap_or(0.0).total_cmp(&b.tbr.unwrap_or(0.0)))
        .and_then(|f| f.estimated_size(duration))
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
    #[serde(default)]
    formats: Vec<RawFormat>,
    #[serde(default)]
    subtitles: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    automatic_captions: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    chapters: Option<Vec<serde_json::Value>>,
    track: Option<String>,
    artist: Option<String>,
    album: Option<String>,
}

#[derive(Deserialize)]
struct RawFormat {
    format_id: Option<String>,
    ext: Option<String>,
    width: Option<f64>,
    height: Option<f64>,
    fps: Option<f64>,
    vcodec: Option<String>,
    acodec: Option<String>,
    tbr: Option<f64>,
    filesize: Option<f64>,
    filesize_approx: Option<f64>,
    format_note: Option<String>,
}

impl RawFormat {
    fn into_info(self) -> Option<FormatInfo> {
        let codec = |c: Option<String>| c.filter(|c| c != "none" && !c.is_empty());
        let f = FormatInfo {
            id: self.format_id?,
            ext: self.ext.unwrap_or_default(),
            width: self.width.map(|v| v as u32),
            height: self.height.map(|v| v as u32),
            fps: self.fps.map(|v| v as f32),
            vcodec: codec(self.vcodec),
            acodec: codec(self.acodec),
            tbr: self.tbr,
            size: self.filesize.or(self.filesize_approx).map(|v| v as u64),
            note: self.format_note,
        };
        // Storyboards and other image-only "formats".
        (f.has_video() || f.has_audio()).then_some(f)
    }
}

#[derive(Deserialize)]
struct RawThumb {
    url: String,
}

pub(crate) fn base_command(tools: &ToolPaths, net: &NetworkOptions) -> Result<std::process::Command> {
    let mut cmd = tools.ytdlp_command()?;
    cmd.env("PYTHONIOENCODING", "utf-8")
        .args(["--encoding", "utf-8", "--no-warnings", "--ignore-config"]);
    if let Some(dir) = tools.ffmpeg.as_ref().and_then(|f| f.parent()) {
        cmd.arg("--ffmpeg-location").arg(dir);
    }
    if let Some(js) = &tools.js_runtime {
        cmd.arg("--js-runtimes")
            .arg(format!("{}:{}", js.name, js.path.display()));
    }
    cmd.args(net.args());
    Ok(cmd)
}

/// `yt-dlp -J --flat-playlist URL`, parsed into the bits the UI shows.
pub fn fetch_info(tools: &ToolPaths, net: &NetworkOptions, url: &str) -> Result<MediaInfo> {
    let mut cmd = base_command(tools, net)?;
    cmd.args(["-J", "--flat-playlist", "--", url]);
    parse_info(&run_json(cmd)?)
}

/// Runs a yt-dlp `-J` command and returns its JSON, or its error made readable.
fn run_json(mut cmd: std::process::Command) -> Result<Vec<u8>> {
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
    Ok(out.stdout)
}

/// A direct audio stream for a page URL, valid for a few hours at most.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamInfo {
    pub url: String,
    /// Headers the stream's server expects, as `Name: value\r\n` lines for ffmpeg.
    pub headers: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
}

/// `yt-dlp -J -f bestaudio/best --no-playlist URL`: the stream to play `url` from.
pub fn resolve_stream(tools: &ToolPaths, net: &NetworkOptions, url: &str) -> Result<StreamInfo> {
    let mut cmd = base_command(tools, net)?;
    cmd.args(["-J", "-f", "bestaudio/best", "--no-playlist", "--", url]);
    parse_stream(&run_json(cmd)?)
}

#[derive(Deserialize)]
struct RawStream {
    url: Option<String>,
    #[serde(default)]
    http_headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    requested_formats: Vec<RawStream>,
    title: Option<String>,
    track: Option<String>,
    artist: Option<String>,
    uploader: Option<String>,
    channel: Option<String>,
    album: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    #[serde(default)]
    thumbnails: Vec<RawThumb>,
}

fn parse_stream(json: &[u8]) -> Result<StreamInfo> {
    let mut raw: RawStream = serde_json::from_slice(json).context("unexpected yt-dlp output")?;
    // A merged pick lists its parts; the audio one is last.
    if raw.url.is_none()
        && let Some(part) = raw.requested_formats.pop()
    {
        raw.url = part.url;
        raw.http_headers = part.http_headers;
    }
    let url = raw.url.ok_or_else(|| anyhow!("This link has no playable stream."))?;
    let headers: String = raw
        .http_headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect();
    Ok(StreamInfo {
        url,
        headers: (!headers.is_empty()).then_some(headers),
        title: raw.track.or(raw.title).unwrap_or_else(|| "Untitled".into()),
        artist: raw.artist.or(raw.uploader).or(raw.channel),
        album: raw.album,
        duration: raw.duration,
        thumbnail: raw.thumbnail.or_else(|| raw.thumbnails.last().map(|t| t.url.clone())),
    })
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
        items: raw.entries.iter().filter_map(PlaylistItem::from_flat).collect(),
        extractor: raw.extractor_key,
        formats: raw.formats.into_iter().filter_map(RawFormat::into_info).collect(),
        subtitle_langs: raw
            .subtitles
            .into_keys()
            .filter(|k| k != "live_chat")
            .collect(),
        has_auto_captions: !raw.automatic_captions.is_empty(),
        chapters: raw.chapters.map_or(0, |c| c.len()),
        track: raw.track,
        artist: raw.artist,
        album: raw.album,
    })
}

/// The settings [`friendly_error`] points to. Android has no tools installer or browser
/// cookies (its yt-dlp, Python and QuickJS ship with the app), only a cookies.txt file.
#[cfg(not(target_os = "android"))]
mod advice {
    pub const JS_RUNTIME: &str = "YouTube needs a JavaScript runtime. Install Deno from Settings > Tools.";
    pub const COOKIES_UNREADABLE: &str = "Redline couldn't read the browser's cookies. Close the browser and try again, \
                                          or switch to Firefox or a cookies.txt file in Settings > Network & sign-in.";
    pub const BOT_CHECK: &str = "Or sign in with browser cookies in Settings > Network & sign-in.";
    pub const SIGN_IN: &str = "This needs a signed-in account. Turn on browser cookies in Settings > Network & sign-in.";
}

#[cfg(target_os = "android")]
mod advice {
    pub const JS_RUNTIME: &str = "YouTube needs a JavaScript runtime, and the one built into Redline didn't work. \
                                  Reinstalling Redline may fix it.";
    pub const COOKIES_UNREADABLE: &str = "Redline couldn't read the cookies.txt file. Choose it again in Settings > Network & sign-in.";
    pub const BOT_CHECK: &str = "Or sign in with a cookies.txt from a signed-in browser in Settings > Network & sign-in.";
    pub const SIGN_IN: &str = "This needs a signed-in account. Choose a cookies.txt from a signed-in browser \
                               in Settings > Network & sign-in.";
}

/// Adds a hint to errors whose cause isn't obvious from yt-dlp's wording.
pub fn friendly_error(msg: &str) -> String {
    let lower = msg.to_lowercase();
    // yt-dlp's wording (curly or straight apostrophe) is all `--cookies` flags and wiki links,
    // and Android shows an error's first line, so this one is replaced rather than appended
    // to. The replacement still matches, so applying this twice changes nothing.
    if lower.contains("confirm you") && lower.contains("not a bot") {
        format!(
            "YouTube wants to confirm you're not a bot. This network has made too many requests; \
             try again later.\n\n{}",
            advice::BOT_CHECK
        )
    } else if lower.contains("js runtime") || lower.contains("javascript runtime") || lower.contains("n challenge") {
        format!("{msg}\n\n{}", advice::JS_RUNTIME)
    } else if lower.contains("could not copy") && lower.contains("cookie")
        || lower.contains("failed to decrypt")
        || lower.contains("cookie") && lower.contains("permission denied")
    {
        format!("{msg}\n\n{}", advice::COOKIES_UNREADABLE)
    } else if lower.contains("sign in to confirm your age")
        || lower.contains("age-restricted")
        || lower.contains("private video")
        || lower.contains("members-only")
        || lower.contains("login required")
        || lower.contains("requires authentication")
    {
        format!("{msg}\n\n{}", advice::SIGN_IN)
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
            if let Some(f) = &req.format {
                push(f);
            } else {
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
            }
            push("--merge-output-format");
            push(&o.container);
            if o.embed_subs && o.subs_mode == SubsMode::Embed {
                push("--embed-subs");
            }
            if o.embed_thumbnail && o.container != "webm" {
                push("--embed-thumbnail");
            }
        }
        Mode::Audio => {
            push("-f");
            push(req.format.as_deref().unwrap_or("ba/b"));
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
    if o.embed_subs {
        // Audio has nowhere to embed subtitles, so they're always saved alongside.
        if o.mode == Mode::Audio || o.subs_mode == SubsMode::Srt {
            push("--write-subs");
            push("--convert-subs");
            push("srt");
        }
        if o.auto_subs {
            push("--write-auto-subs");
        }
        push("--sub-langs");
        push(if o.sub_langs.trim().is_empty() { DEFAULT_SUB_LANGS } else { o.sub_langs.trim() });
    }
    let embeds_cover = o.embed_thumbnail
        && match o.mode {
            Mode::Video => o.container != "webm",
            Mode::Audio => o.audio_format != "wav",
        };
    if embeds_cover && o.square_cover {
        push("--convert-thumbnails");
        push("jpg");
        push("--ppa");
        push("ThumbnailsConvertor+FFmpeg_o:-c:v mjpeg -vf \"crop='min(iw,ih)':'min(iw,ih)'\"");
    }
    if o.embed_metadata {
        push("--embed-metadata");
    }
    let split = o.split_chapters && req.trim.is_none() && !o.playlist;
    if split {
        push("--split-chapters");
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
    if split {
        a.push("-o".into());
        a.push(
            "chapter:%(title)s [%(id)s] - chapters/%(section_number)02d - %(section_title)s.%(ext)s"
                .into(),
        );
    }
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
        "SplitChapters" | "FFmpegSplitChapters" => "Splitting chapters…".into(),
        "FFmpegSubtitlesConvertor" | "SubtitlesConvertor" => "Converting subtitles…".into(),
        "ThumbnailsConvertor" | "FFmpegThumbnailsConvertor" => "Preparing cover art…".into(),
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
    let mut cmd = base_command(tools, &req.network)?;
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

    // `--print` makes yt-dlp quiet, so the "already recorded" line usually isn't shown; a run
    // with an archive that started nothing and wrote nothing means everything was skipped.
    if last_file.is_none() && (archived > 0 || req.archive.is_some() && items == 0) {
        return Err(AlreadyDownloaded.into());
    }
    if let (Some(tags), Some(file)) = (&req.tags, &last_file)
        && !req.options.playlist
    {
        rep.processing("Writing tags…");
        write_tags(tools, file, tags, cancel)?;
    }
    // For playlists, point "open" at the playlist folder rather than the last track.
    Ok(match (req.options.playlist && items > 1, last_file) {
        (true, Some(f)) => f.parent().map(Path::to_path_buf),
        (_, f) => f,
    })
}

/// ffmpeg arguments that copy `input` to `output` with new title/artist/album tags.
fn tag_args(input: &Path, output: &Path, tags: &Tags, has_cover: bool) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-i"]
        .into_iter()
        .map(String::from)
        .collect();
    a.push(input.display().to_string());
    // Audio plus any cover image; data tracks (e.g. timed metadata) can break the muxer.
    a.extend(["-map", "0:a", "-map", "0:v?", "-map_metadata", "0", "-c", "copy"].map(String::from));
    let ext = input
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        // Keep the cover as cover art rather than a video track.
        "m4a" | "mp4" if has_cover => {
            a.extend(["-disposition:v", "attached_pic"].map(String::from))
        }
        "mp3" => a.extend(["-id3v2_version", "3"].map(String::from)),
        _ => {}
    }
    for (key, value) in [("title", &tags.title), ("artist", &tags.artist), ("album", &tags.album)] {
        if !value.trim().is_empty() {
            a.push("-metadata".into());
            a.push(format!("{key}={}", value.trim()));
        }
    }
    a.push(output.display().to_string());
    a
}

fn write_tags(tools: &ToolPaths, file: &Path, tags: &Tags, cancel: &CancelHandle) -> Result<()> {
    let ffmpeg = tools
        .ffmpeg
        .as_ref()
        .ok_or_else(|| anyhow!("ffmpeg is not installed"))?;
    let ext = file.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = file.with_extension(format!("tagging.{ext}"));
    let has_cover = tools.ffprobe.as_ref().is_some_and(|p| {
        command(p)
            .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=index", "-of", "csv=p=0"])
            .arg(file)
            .output()
            .is_ok_and(|o| !o.stdout.trim_ascii().is_empty())
    });
    let mut cmd = command(ffmpeg);
    cmd.args(tag_args(file, &tmp, tags, has_cover));
    if let Err(e) = run_streaming(cmd, cancel, |_| {}) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.context("couldn't write tags"));
    }
    std::fs::rename(&tmp, file).context("couldn't replace the file with the tagged copy")
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
    fn format_subs_chapter_args() {
        let mut r = req(DownloadOptions::default());
        r.format = Some("137+140".into());
        let a = build_args(&r);
        assert!(has_pair(&a, "-f", "137+140"));
        assert!(!a.contains(&"-S".to_string()));
        assert!(!a.contains(&"--sub-langs".to_string()));

        r.options.embed_subs = true;
        r.options.sub_langs = "de,fr".into();
        let a = build_args(&r);
        assert!(a.contains(&"--embed-subs".to_string()));
        assert!(has_pair(&a, "--sub-langs", "de,fr"));
        assert!(!a.contains(&"--write-subs".to_string()));

        r.options.subs_mode = SubsMode::Srt;
        r.options.auto_subs = true;
        let a = build_args(&r);
        assert!(!a.contains(&"--embed-subs".to_string()));
        assert!(has_pair(&a, "--convert-subs", "srt"));
        assert!(a.contains(&"--write-auto-subs".to_string()));

        r.options.split_chapters = true;
        let a = build_args(&r);
        assert!(a.contains(&"--split-chapters".to_string()));
        assert!(a.iter().any(|s| s.starts_with("chapter:")));
        r.trim = Some(Trim { start: 1.0, end: None, precise: false });
        assert!(!build_args(&r).contains(&"--split-chapters".to_string()));

        let mut audio = req(DownloadOptions {
            mode: Mode::Audio,
            square_cover: true,
            ..Default::default()
        });
        audio.format = Some("251".into());
        let a = build_args(&audio);
        assert!(has_pair(&a, "-f", "251"));
        assert!(has_pair(&a, "--convert-thumbnails", "jpg"));
        assert!(a.iter().any(|s| s.contains("crop='min(iw,ih)'")));
    }

    #[test]
    fn tag_args_copy_and_set_metadata() {
        let tags = Tags {
            title: " New ".into(),
            artist: "Band".into(),
            album: String::new(),
        };
        let a = tag_args(Path::new("a.m4a"), Path::new("a.tagging.m4a"), &tags, true).join(" ");
        assert!(a.contains("-map 0:a -map 0:v? -map_metadata 0 -c copy -disposition:v attached_pic"));
        assert!(a.contains("-metadata title=New -metadata artist=Band"));
        assert!(!a.contains("album="));
        assert!(a.ends_with("a.tagging.m4a"));
        let mp3 = tag_args(Path::new("a.mp3"), Path::new("o.mp3"), &tags, true).join(" ");
        assert!(mp3.contains("-id3v2_version 3") && !mp3.contains("disposition"));
    }

    #[test]
    fn height_choices_estimate_sizes() {
        let f = |id: &str, h: Option<u32>, v: bool, a: bool, tbr: f64, size: Option<u64>| FormatInfo {
            id: id.into(),
            height: h,
            fps: h.map(|_| 30.0),
            vcodec: v.then(|| "avc1".into()),
            acodec: a.then(|| "mp4a".into()),
            tbr: Some(tbr),
            size,
            ..Default::default()
        };
        let formats = vec![
            f("140", None, false, true, 128.0, Some(1_000)),
            f("137", Some(1080), true, false, 4000.0, Some(50_000)),
            f("136", Some(720), true, false, 2000.0, None),
            f("18", Some(360), true, true, 500.0, Some(9_000)),
        ];
        let c = height_choices(&formats, Some(10.0));
        assert_eq!(c.iter().map(|c| c.height).collect::<Vec<_>>(), vec![1080, 720, 360]);
        assert_eq!(c[0].size, Some(51_000));
        assert_eq!(c[1].size, Some(2_500_000 + 1_000), "720 estimated from bitrate");
        assert_eq!(c[2].size, Some(9_000), "combined format needs no extra audio");
        assert_eq!(formats[1].quality_label(), "1080p");
    }

    #[test]
    fn network_args() {
        assert!(NetworkOptions::default().args().is_empty());
        let mut n = NetworkOptions {
            cookies_browser: Some("firefox".into()),
            cookies_file: Some(PathBuf::from("c.txt")),
            proxy: " socks5://h:1 ".into(),
            rate_limit: "2M".into(),
            retries: 5,
        };
        let a = n.args();
        assert!(has_pair(&a, "--cookies-from-browser", "firefox"));
        assert!(!a.contains(&"--cookies".to_string()), "browser wins over file");
        assert!(has_pair(&a, "--proxy", "socks5://h:1"));
        assert!(has_pair(&a, "-r", "2M"));
        assert!(has_pair(&a, "--retries", "5"));
        n.cookies_browser = None;
        assert!(has_pair(&n.args(), "--cookies", "c.txt"));
        assert!(friendly_error("ERROR: Could not copy Chrome cookie database").contains("Firefox"));
        assert!(friendly_error("Sign in to confirm your age").contains("cookies"));
    }

    #[test]
    fn bot_check_error_leads_with_plain_words() {
        // As yt-dlp words it, after run_json drops the `ERROR:`.
        let raw = "[youtube] 6ueeeG0TBjs: Sign in to confirm you\u{2019}re not a bot. Use --cookies-from-browser \
                   or --cookies for the authentication. See  https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp  \
                   for how to manually pass cookies.";
        let msg = friendly_error(raw);
        let first = msg.lines().next().unwrap();
        assert!(first.starts_with("YouTube wants to confirm you're not a bot."), "{msg}");
        assert!(first.contains("try again later"), "{msg}");
        assert!(!msg.contains("--cookies") && !msg.contains("https://"), "{msg}");
        assert!(msg.contains("Settings > Network & sign-in"), "{msg}");
        assert!(friendly_error(&raw.replace('\u{2019}', "'")) == msg, "straight apostrophe");
        assert_eq!(friendly_error(&msg), msg, "the player may apply it again");
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
    fn parses_stream_json() {
        let s = parse_stream(
            br#"{"title":"Me at the zoo","uploader":"jawed","duration":19,"url":"https://cdn/a",
                "http_headers":{"User-Agent":"UA","Accept":"*/*"},"thumbnails":[{"url":"t1"},{"url":"t2"}]}"#,
        )
        .unwrap();
        assert_eq!(s.url, "https://cdn/a");
        assert_eq!(s.headers.as_deref(), Some("Accept: */*\r\nUser-Agent: UA\r\n"));
        assert_eq!(s.artist.as_deref(), Some("jawed"));
        assert_eq!(s.duration, Some(19.0));
        assert_eq!(s.thumbnail.as_deref(), Some("t2"));

        let merged = parse_stream(
            br#"{"title":"T","track":"Song","requested_formats":[{"url":"v"},{"url":"a","http_headers":{"X":"1"}}]}"#,
        )
        .unwrap();
        assert_eq!(merged.url, "a");
        assert_eq!(merged.title, "Song");
        assert_eq!(merged.headers.as_deref(), Some("X: 1\r\n"));
        assert!(parse_stream(br#"{"title":"T"}"#).is_err());
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

        let full = parse_info(
            br#"{"title":"T","formats":[{"format_id":"sb0","vcodec":"none","acodec":"none"},
                {"format_id":"137","ext":"mp4","height":1080,"fps":60,"vcodec":"avc1","acodec":"none","filesize_approx":5e7},
                {"format_id":"140","ext":"m4a","vcodec":"none","acodec":"mp4a.40.2","tbr":129.5}],
                "subtitles":{"de":[],"live_chat":[]},"automatic_captions":{"en":[]},
                "chapters":[{},{}],"artist":"A"}"#,
        )
        .unwrap();
        assert_eq!(full.formats.len(), 2, "storyboard dropped");
        assert_eq!(full.formats[0].size, Some(50_000_000));
        assert_eq!(full.formats[0].quality_label(), "1080p60");
        assert!(!full.formats[1].has_video() && full.formats[1].has_audio());
        assert_eq!(full.subtitle_langs, vec!["de".to_string()]);
        assert!(full.has_auto_captions);
        assert_eq!(full.chapters, 2);
        assert_eq!(full.artist.as_deref(), Some("A"));

        let pl = parse_info(br#"{"_type":"playlist","title":"P","entries":[{},{},{}]}"#).unwrap();
        assert!(pl.is_playlist);
        assert_eq!(pl.entries, 3);
        assert!(pl.items.is_empty(), "entries without links are skipped");
    }

    #[test]
    fn parses_flat_playlist_entries() {
        let pl = parse_info(
            br#"{"_type":"playlist","title":"Mix","playlist_count":4,"entries":[
                {"_type":"url","ie_key":"Youtube","id":"abc","url":"https://www.youtube.com/watch?v=abc",
                 "title":"One","channel":"Band","duration":201.0,"thumbnails":[{"url":"s"},{"url":"l"}]},
                {"_type":"url","ie_key":"Youtube","id":"def","url":"def","title":"Two"},
                {"_type":"url","url":"https://soundcloud.com/a/b","title":"Three","uploader":"A"},
                {"_type":"url","ie_key":"Other","id":"zzz"}]}"#,
        )
        .unwrap();
        assert_eq!(pl.entries, 4);
        let urls: Vec<_> = pl.items.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://www.youtube.com/watch?v=abc", "https://www.youtube.com/watch?v=def", "https://soundcloud.com/a/b"]
        );
        assert_eq!(pl.items[0].uploader.as_deref(), Some("Band"));
        assert_eq!(pl.items[0].duration, Some(201.0));
        assert_eq!(pl.items[0].thumbnail.as_deref(), Some("l"));
        assert_eq!(pl.items[2].uploader.as_deref(), Some("A"));
    }

    #[test]
    fn untitled_soundcloud_entries_get_titles_from_their_links() {
        let pl = parse_info(
            br#"{"_type":"playlist","title":"Soulhack","entries":[
                {"ie_key":"Soundcloud","url":"https://soundcloud.com/forss/city-ports","id":"290"},
                {"ie_key":"Soundcloud","url":"https://soundcloud.com/forss/funk-for-nerds?in=forss/sets/soulhack",
                 "id":"292","album_artist":"Forss"},
                {"ie_key":"Soundcloud","url":"https://on.soundcloud.com/AbCdE","id":"293"}]}"#,
        )
        .unwrap();
        let titles: Vec<_> = pl.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["City Ports", "Funk For Nerds", "https://on.soundcloud.com/AbCdE"]);
        assert_eq!(pl.items[0].uploader, None);
        assert_eq!(pl.items[1].uploader.as_deref(), Some("Forss"));
    }

    #[test]
    fn titles_from_links() {
        let t = |u: &str| title_from_link(u);
        assert_eq!(t("https://soundcloud.com/forss/city-ports/").as_deref(), Some("City Ports"));
        assert_eq!(t("https://m.soundcloud.com/a/über_track-2#t=1").as_deref(), Some("Über Track 2"));
        assert_eq!(t("https://soundcloud.com/forss/sets/soulhack"), None);
        assert_eq!(t("https://soundcloud.com/forss"), None);
        assert_eq!(t("https://www.youtube.com/watch?v=abc"), None);
        assert_eq!(t("https://api-v2.soundcloud.com/tracks/295"), None);
    }

    #[test]
    fn parses_resolved_titles() {
        let r = parse_resolved(
            "https://api-v2.soundcloud.com/tracks/295\tForss\t409.033\tJourneyman\r\n\
             https://x/1\tNA\tNA\tTab\there\n\
             https://x/2\tA\t1\tNA\n\
             garbage\n",
        );
        assert_eq!(r.len(), 2);
        assert_eq!(
            r["https://api-v2.soundcloud.com/tracks/295"],
            Resolved {
                title: "Journeyman".into(),
                uploader: Some("Forss".into()),
                duration: Some(409.033),
            }
        );
        assert_eq!(r["https://x/1"].title, "Tab\there");
        assert_eq!(r["https://x/1"].uploader, None);
    }

    fn program_and_args(cmd: &std::process::Command) -> (String, Vec<String>) {
        let s = |o: &std::ffi::OsStr| o.to_string_lossy().into_owned();
        (s(cmd.get_program()), cmd.get_args().map(s).collect())
    }

    #[test]
    fn base_command_desktop_shape() {
        let bin = PathBuf::from("bin");
        let tools = ToolPaths {
            ytdlp: Some(bin.join("yt-dlp.exe")),
            ffmpeg: Some(bin.join("ffmpeg.exe")),
            js_runtime: Some(crate::tools::JsRuntime { name: "deno".into(), path: bin.join("deno.exe") }),
            ..Default::default()
        };
        let (program, args) = program_and_args(&base_command(&tools, &NetworkOptions::default()).unwrap());
        assert_eq!(program, bin.join("yt-dlp.exe").to_string_lossy());
        assert_eq!(args[..4], ["--encoding", "utf-8", "--no-warnings", "--ignore-config"]);
        assert!(has_pair(&args, "--ffmpeg-location", &bin.to_string_lossy()));
        assert!(has_pair(&args, "--js-runtimes", &format!("deno:{}", bin.join("deno.exe").display())));

        assert!(base_command(&ToolPaths::default(), &NetworkOptions::default()).is_err());
    }

    #[test]
    fn base_command_through_launcher() {
        let lib = PathBuf::from("/data/app/lib/x86_64");
        let script = PathBuf::from("/data/user/0/app/no_backup/youtubedl-android/yt-dlp/yt-dlp");
        let bin = PathBuf::from("/data/user/0/app/files/bin");
        let tools = ToolPaths {
            ytdlp: Some(script.clone()),
            ytdlp_launcher: Some(lib.join("libpython.so")),
            ffmpeg: Some(bin.join("ffmpeg")),
            ffprobe: Some(bin.join("ffprobe")),
            js_runtime: Some(crate::tools::JsRuntime { name: "quickjs".into(), path: lib.join("libqjs.so") }),
        };
        let (program, args) = program_and_args(&base_command(&tools, &NetworkOptions::default()).unwrap());
        assert_eq!(program, lib.join("libpython.so").to_string_lossy());
        // The script comes first, then yt-dlp's own arguments.
        assert_eq!(args[0], script.to_string_lossy());
        assert_eq!(args[1..5], ["--encoding", "utf-8", "--no-warnings", "--ignore-config"]);
        assert!(has_pair(&args, "--ffmpeg-location", &bin.to_string_lossy()));
        assert!(has_pair(&args, "--js-runtimes", &format!("quickjs:{}", lib.join("libqjs.so").display())));
    }
}
