//! What crosses to Kotlin: plain records mirroring redline-core's types, and the mappings
//! between them. Paths cross as strings (Kotlin has them as `File.absolutePath`). Everything
//! here is pure so it can be tested on the desktop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use redline_core::download_plan::LinkChoices;
use redline_core::format::starts_at;
use redline_core::history::{History, HistoryEntry};
use redline_core::jobs::{Job, JobSpec, JobStatus};
use redline_core::library::{self, NowPlaying, Playlist, Repeat, Track};
use redline_core::player::dsp::{self, EqSettings};
use redline_core::player::{PlayerSettings, PlayerState, Source, Status};
use redline_core::queue_watch;
use redline_core::settings::Settings;
use redline_core::tools::convert::{Category, classify};
use redline_core::tools::filters::SubSource;
use redline_core::tools::ytdlp::{
    self, AUDIO_BITRATES, AUDIO_FORMATS, CONTAINERS, DEFAULT_SUB_LANGS, DownloadOptions, FormatInfo,
    MediaInfo, Mode, NetworkOptions, PlaylistItem, SubsMode, Tags, Trim, VIDEO_QUALITIES,
};

use crate::convert::{ConvertSettingsRecord, apply_convert_settings, convert_settings_record};

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Where youtubedl-android put everything, as the Kotlin side computes it after
/// `YoutubeDL.init` and `FFmpeg.init` (see `RedlineEngine.kt`).
#[derive(uniffi::Record, Clone, Debug)]
pub struct AndroidPathsRecord {
    /// Settings, history, the queue and `bin/` (`Context.filesDir`).
    pub data_dir: String,
    /// Where downloads go until the user picks another folder.
    pub download_dir: String,
    /// Where converted files go. ffmpeg needs a real folder, so it's the app's own; used when
    /// the settings don't name one yet.
    pub convert_dir: String,
    /// `<nativeLibraryDir>/libpython.so`.
    pub python: String,
    /// `<noBackupFilesDir>/youtubedl-android/yt-dlp/yt-dlp`.
    pub ytdlp_script: String,
    /// yt-dlp's name for the JavaScript runtime, `quickjs`.
    pub js_runtime_name: String,
    /// `<nativeLibraryDir>/libqjs.so`.
    pub js_runtime: String,
    /// `<nativeLibraryDir>/libffmpeg.so`.
    pub ffmpeg: String,
    /// `<nativeLibraryDir>/libffprobe.so`.
    pub ffprobe: String,
    /// `LD_LIBRARY_PATH`, `PYTHONHOME` and the rest every tool runs with.
    pub env: HashMap<String, String>,
}

/// The tools' environment in a fixed order. Kotlin hands over a map, whose order isn't stable,
/// and redline-core only accepts the same environment again if it compares equal (a recreated
/// activity initialises again in the same process).
pub fn child_env(env: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut vars: Vec<_> = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    vars.sort();
    vars
}

#[cfg(target_os = "android")]
impl AndroidPathsRecord {
    pub fn to_core(&self) -> redline_core::android::AndroidPaths {
        redline_core::android::AndroidPaths {
            data_dir: PathBuf::from(&self.data_dir),
            download_dir: PathBuf::from(&self.download_dir),
            python: PathBuf::from(&self.python),
            ytdlp_script: PathBuf::from(&self.ytdlp_script),
            js_runtime: redline_core::tools::JsRuntime {
                name: self.js_runtime_name.clone(),
                path: PathBuf::from(&self.js_runtime),
            },
            ffmpeg: PathBuf::from(&self.ffmpeg),
            ffprobe: PathBuf::from(&self.ffprobe),
            env: child_env(&self.env),
        }
    }
}

/// Which tools Redline found, and whether it can download and convert.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ToolsStatusRecord {
    pub ytdlp: Option<String>,
    /// What runs the yt-dlp script (Python on Android).
    pub ytdlp_launcher: Option<String>,
    pub ytdlp_version: Option<String>,
    pub ffmpeg: Option<String>,
    pub ffprobe: Option<String>,
    pub js_runtime: Option<String>,
    pub ready_for_download: bool,
    pub ready_for_convert: bool,
    pub redline_version: String,
}

pub fn tools_status(tools: &redline_core::tools::ToolPaths, ytdlp_version: Option<String>) -> ToolsStatusRecord {
    let s = |p: &Option<PathBuf>| p.as_deref().map(path_string);
    ToolsStatusRecord {
        ytdlp: s(&tools.ytdlp),
        ytdlp_launcher: s(&tools.ytdlp_launcher),
        ytdlp_version,
        ffmpeg: s(&tools.ffmpeg),
        ffprobe: s(&tools.ffprobe),
        js_runtime: tools.js_runtime.as_ref().map(|js| format!("{} ({})", js.name, path_string(&js.path))),
        ready_for_download: tools.ready_for_download(),
        ready_for_convert: tools.ready_for_convert(),
        redline_version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

// ---- Download options -------------------------------------------------------------------

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadMode {
    Video,
    Audio,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleMode {
    /// Inside the video file.
    Embed,
    /// Separate .srt files next to the download.
    Srt,
}

/// The Download page's format choices; also the remembered defaults in [`SettingsRecord`].
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct DownloadOptionsRecord {
    pub mode: DownloadMode,
    /// 0 is "best available".
    pub max_height: u32,
    /// `mp4`, `mkv` or `webm` (see [`DownloadChoiceListsRecord`]).
    pub container: String,
    pub audio_format: String,
    /// `0` is the best VBR, else e.g. `320K`.
    pub audio_bitrate: String,
    pub embed_thumbnail: bool,
    pub embed_metadata: bool,
    pub embed_subs: bool,
    /// Download the whole playlist; ignored for links that aren't one.
    pub playlist: bool,
    pub remove_sponsors: bool,
    pub sub_langs: String,
    pub auto_subs: bool,
    pub subs_mode: SubtitleMode,
    pub split_chapters: bool,
    pub square_cover: bool,
}

impl From<&DownloadOptions> for DownloadOptionsRecord {
    fn from(o: &DownloadOptions) -> Self {
        Self {
            mode: match o.mode {
                Mode::Video => DownloadMode::Video,
                Mode::Audio => DownloadMode::Audio,
            },
            max_height: o.max_height,
            container: o.container.clone(),
            audio_format: o.audio_format.clone(),
            audio_bitrate: o.audio_bitrate.clone(),
            embed_thumbnail: o.embed_thumbnail,
            embed_metadata: o.embed_metadata,
            embed_subs: o.embed_subs,
            playlist: o.playlist,
            remove_sponsors: o.remove_sponsors,
            sub_langs: o.sub_langs.clone(),
            auto_subs: o.auto_subs,
            subs_mode: match o.subs_mode {
                SubsMode::Embed => SubtitleMode::Embed,
                SubsMode::Srt => SubtitleMode::Srt,
            },
            split_chapters: o.split_chapters,
            square_cover: o.square_cover,
        }
    }
}

impl From<&DownloadOptionsRecord> for DownloadOptions {
    fn from(o: &DownloadOptionsRecord) -> Self {
        Self {
            mode: match o.mode {
                DownloadMode::Video => Mode::Video,
                DownloadMode::Audio => Mode::Audio,
            },
            max_height: o.max_height,
            container: o.container.clone(),
            audio_format: o.audio_format.clone(),
            audio_bitrate: o.audio_bitrate.clone(),
            embed_thumbnail: o.embed_thumbnail,
            embed_metadata: o.embed_metadata,
            embed_subs: o.embed_subs,
            playlist: o.playlist,
            remove_sponsors: o.remove_sponsors,
            sub_langs: o.sub_langs.clone(),
            auto_subs: o.auto_subs,
            subs_mode: match o.subs_mode {
                SubtitleMode::Embed => SubsMode::Embed,
                SubtitleMode::Srt => SubsMode::Srt,
            },
            split_chapters: o.split_chapters,
            square_cover: o.square_cover,
        }
    }
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ChoiceRecord {
    pub value: String,
    pub label: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct QualityChoiceRecord {
    /// 0 is "best available".
    pub max_height: u32,
    pub label: String,
}

/// What the Download page's pickers offer, as the desktop shows them.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct DownloadChoiceListsRecord {
    pub video_qualities: Vec<QualityChoiceRecord>,
    pub containers: Vec<String>,
    pub audio_formats: Vec<String>,
    pub audio_bitrates: Vec<ChoiceRecord>,
    pub default_sub_langs: String,
}

#[uniffi::export]
pub fn download_choice_lists() -> DownloadChoiceListsRecord {
    DownloadChoiceListsRecord {
        video_qualities: VIDEO_QUALITIES
            .iter()
            .map(|(h, l)| QualityChoiceRecord { max_height: *h, label: (*l).to_owned() })
            .collect(),
        containers: CONTAINERS.iter().map(|c| (*c).to_owned()).collect(),
        audio_formats: AUDIO_FORMATS.iter().map(|c| (*c).to_owned()).collect(),
        audio_bitrates: AUDIO_BITRATES
            .iter()
            .map(|(v, l)| ChoiceRecord { value: (*v).to_owned(), label: (*l).to_owned() })
            .collect(),
        default_sub_langs: DEFAULT_SUB_LANGS.to_owned(),
    }
}

/// "Video · MP4 · 1080p", as the queue's subtitle line reads.
#[uniffi::export]
pub fn options_summary(options: DownloadOptionsRecord) -> String {
    DownloadOptions::from(&options).summary()
}

/// FLAC and WAV have no bitrate to pick.
#[uniffi::export]
pub fn is_lossless_audio(audio_format: String) -> bool {
    ytdlp::is_lossless(&audio_format)
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Default)]
pub struct TagsRecord {
    pub title: String,
    pub artist: String,
    pub album: String,
}

impl From<&Tags> for TagsRecord {
    fn from(t: &Tags) -> Self {
        Self { title: t.title.clone(), artist: t.artist.clone(), album: t.album.clone() }
    }
}

impl From<&TagsRecord> for Tags {
    fn from(t: &TagsRecord) -> Self {
        Self { title: t.title.clone(), artist: t.artist.clone(), album: t.album.clone() }
    }
}

/// Only download part of the media. Seconds; `end_secs: None` runs to the end.
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq)]
pub struct TrimRecord {
    pub start_secs: f64,
    pub end_secs: Option<f64>,
    /// Re-encode around the cuts so they land exactly instead of on keyframes.
    pub precise: bool,
}

/// The trim to send with the job, or why it can't be done; the desktop's trim checks. A trim
/// that keeps everything is refused rather than silently ignored, as there.
pub fn check_trim(trim: Option<TrimRecord>, duration: Option<f64>) -> Result<Option<Trim>, String> {
    let Some(t) = trim else { return Ok(None) };
    if t.start_secs < 0.0 || t.end_secs.is_some_and(|e| e < 0.0) {
        return Err("Times can't be negative.".into());
    }
    if t.end_secs.is_some_and(|e| e <= t.start_secs) {
        return Err("The end has to come after the start.".into());
    }
    if duration.is_some_and(|d| t.start_secs >= d) {
        return Err("The start is past the end of the video.".into());
    }
    let whole = t.start_secs <= 0.0
        && match (duration, t.end_secs) {
            (Some(d), Some(e)) => e >= d.ceil(),
            (_, None) => true,
            (None, Some(_)) => false,
        };
    if whole {
        return Err("Choose the part to keep.".into());
    }
    Ok(Some(Trim { start: t.start_secs, end: t.end_secs, precise: t.precise }))
}

/// Everything the Download page decided about one link.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct DownloadChoicesRecord {
    pub url: String,
    /// Also remembered as the new defaults, like the desktop page's choices.
    pub options: DownloadOptionsRecord,
    /// Exact format ids from [`MediaInfoRecord::formats`]; need a `fetch_info` of this link.
    pub pick_video: Option<String>,
    pub pick_audio: Option<String>,
    /// Edited audio tags. Only written if they differ from the looked-up ones, and only for
    /// single audio downloads.
    pub tags: Option<TagsRecord>,
    /// Ignored for playlists.
    pub trim: Option<TrimRecord>,
    /// Don't start before this Unix time.
    pub not_before: Option<i64>,
}

/// The desktop's per-link state, rebuilt from the looked-up info (if any) and the choices.
pub fn link_choices(info: Option<MediaInfo>, c: &DownloadChoicesRecord) -> LinkChoices {
    let mut link = LinkChoices::default();
    if let Some(info) = info {
        link.set_info(info);
        if let Some(t) = &c.tags {
            link.tags = t.into();
        }
    }
    link.pick_video = c.pick_video.clone().filter(|s| !s.is_empty());
    link.pick_audio = c.pick_audio.clone().filter(|s| !s.is_empty());
    link
}

// ---- Lookups ----------------------------------------------------------------------------

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct FormatRecord {
    pub id: String,
    pub ext: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f32>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    /// kbit/s.
    pub tbr: Option<f64>,
    /// Bytes, estimated from the bitrate when the site doesn't say.
    pub size: Option<u64>,
    pub note: Option<String>,
    /// e.g. `1080p60`, or the bitrate for audio.
    pub quality_label: String,
    pub has_video: bool,
    pub has_audio: bool,
}

fn format_record(f: &FormatInfo, duration: Option<f64>) -> FormatRecord {
    FormatRecord {
        id: f.id.clone(),
        ext: f.ext.clone(),
        width: f.width,
        height: f.height,
        fps: f.fps,
        vcodec: f.vcodec.clone(),
        acodec: f.acodec.clone(),
        tbr: f.tbr,
        size: f.estimated_size(duration),
        note: f.note.clone(),
        quality_label: f.quality_label(),
        has_video: f.has_video(),
        has_audio: f.has_audio(),
    }
}

/// One row of the quality picker: a distinct video height.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct HeightChoiceRecord {
    pub height: u32,
    pub fps: Option<f32>,
    /// Roughly what yt-dlp's pick at this height plus the best audio weighs.
    pub size: Option<u64>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PlaylistItemRecord {
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration_secs: Option<f64>,
    pub thumbnail: Option<String>,
}

impl From<&PlaylistItem> for PlaylistItemRecord {
    fn from(i: &PlaylistItem) -> Self {
        Self {
            url: i.url.clone(),
            title: i.title.clone(),
            uploader: i.uploader.clone(),
            duration_secs: i.duration,
            thumbnail: i.thumbnail.clone(),
        }
    }
}

/// A looked-up link.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct MediaInfoRecord {
    /// The link as looked up (trimmed); pass it back in [`DownloadChoicesRecord::url`].
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration_secs: Option<f64>,
    pub thumbnail: Option<String>,
    pub is_playlist: bool,
    pub entry_count: u32,
    pub items: Vec<PlaylistItemRecord>,
    pub extractor: Option<String>,
    pub formats: Vec<FormatRecord>,
    /// Distinct video heights, tallest first.
    pub heights: Vec<HeightChoiceRecord>,
    pub best_audio_size: Option<u64>,
    pub subtitle_langs: Vec<String>,
    pub has_auto_captions: bool,
    pub chapters: u32,
    /// Audio tags as the site names them; the tags editor starts from these.
    pub tags: TagsRecord,
}

pub fn media_info_record(url: &str, info: &MediaInfo) -> MediaInfoRecord {
    let d = info.duration;
    MediaInfoRecord {
        url: url.to_owned(),
        title: info.title.clone(),
        uploader: info.uploader.clone(),
        duration_secs: d,
        thumbnail: info.thumbnail.clone(),
        is_playlist: info.is_playlist,
        entry_count: info.entries as u32,
        items: info.items.iter().map(Into::into).collect(),
        extractor: info.extractor.clone(),
        formats: info.formats.iter().map(|f| format_record(f, d)).collect(),
        heights: ytdlp::height_choices(&info.formats, d)
            .into_iter()
            .map(|h| HeightChoiceRecord { height: h.height, fps: h.fps, size: h.size })
            .collect(),
        best_audio_size: ytdlp::best_audio_size(&info.formats, d),
        subtitle_langs: info.subtitle_langs.clone(),
        has_auto_captions: info.has_auto_captions,
        chapters: info.chapters as u32,
        tags: (&redline_core::download_plan::tags_from(info)).into(),
    }
}

// ---- Queue ------------------------------------------------------------------------------

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    /// Post-processing (merging, converting); no progress figure.
    Processing,
    Done,
    /// Ended without doing anything, e.g. the link was downloaded before.
    Skipped,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Processing)
    }
}

fn job_state(s: &JobStatus) -> JobState {
    match s {
        JobStatus::Queued => JobState::Queued,
        JobStatus::Running { .. } => JobState::Running,
        JobStatus::Processing(_) => JobState::Processing,
        JobStatus::Done(_) => JobState::Done,
        JobStatus::Skipped(_) => JobState::Skipped,
        JobStatus::Failed(_) => JobState::Failed,
        JobStatus::Cancelled => JobState::Cancelled,
    }
}

/// One row of the queue.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct JobRecord {
    pub id: u64,
    pub title: String,
    /// e.g. "Audio · MP3 · Best (VBR)".
    pub subtitle: String,
    pub state: JobState,
    /// 0..1 while running, when known.
    pub progress: Option<f32>,
    /// The status line: progress text, the start time of a scheduled job, the file name of a
    /// done one, the first line of an error.
    pub detail: String,
    /// The full error of a failed job.
    pub error: Option<String>,
    pub output_path: Option<String>,
    /// The folder the output is (or will be) in.
    pub output_dir: Option<String>,
    pub url: Option<String>,
    pub is_download: bool,
    /// Scheduled start, Unix seconds.
    pub not_before: Option<i64>,
    /// The files a conversion reads; empty for downloads.
    pub inputs: Vec<String>,
}

/// The desktop's text with its icon-font glyphs swapped for plain characters: the core words
/// conversions "MKV <arrow> MP4" with Phosphor's arrow, which Android's fonts don't have.
pub fn plain_text(s: &str) -> String {
    s.replace(redline_core::format::ARROW_RIGHT, "\u{2192}")
}

fn job_url(spec: &JobSpec) -> Option<String> {
    match spec {
        JobSpec::Download(r) => Some(r.url.clone()),
        _ => None,
    }
}

/// The files a job converts or joins, with a subtitle file it burns in.
pub fn job_inputs(spec: &JobSpec) -> Vec<PathBuf> {
    match spec {
        JobSpec::Download(_) => Vec::new(),
        JobSpec::Convert(r) => {
            let mut files = vec![r.input.clone()];
            if let Some(SubSource::File(f)) = &r.burn_subs {
                files.push(f.clone());
            }
            files
        }
        JobSpec::Concat(r) => r.inputs.clone(),
    }
}

pub fn job_record(job: &Job) -> JobRecord {
    let (progress, detail, error, output) = match &job.status {
        JobStatus::Queued => (
            None,
            job.not_before.map_or_else(|| "Waiting…".to_owned(), starts_at),
            None,
            None,
        ),
        JobStatus::Running { progress, detail } => (*progress, detail.clone(), None, None),
        JobStatus::Processing(msg) => (None, msg.clone(), None, None),
        JobStatus::Done(path) => (
            Some(1.0),
            path.as_deref()
                .and_then(Path::file_name)
                .map_or_else(|| "Done".to_owned(), |n| n.to_string_lossy().into_owned()),
            None,
            path.as_deref().map(path_string),
        ),
        JobStatus::Skipped(why) => (None, why.clone(), None, None),
        JobStatus::Failed(e) => (None, e.lines().next().unwrap_or("Failed").to_owned(), Some(e.clone()), None),
        JobStatus::Cancelled => (None, "Cancelled".to_owned(), None, None),
    };
    JobRecord {
        id: job.id,
        title: job.title.clone(),
        subtitle: plain_text(&job.subtitle),
        state: job_state(&job.status),
        progress,
        detail,
        error,
        output_path: output,
        output_dir: job.output_dir().as_deref().map(path_string),
        url: job_url(&job.spec),
        is_download: job.is_download(),
        not_before: job.not_before,
        inputs: job_inputs(&job.spec).iter().map(|p| path_string(p)).collect(),
    }
}

/// A job that ended, for a notification.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct FinishedRecord {
    pub job: JobRecord,
}

/// A notification's text, worded like the desktop's.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct AnnouncementRecord {
    pub title: String,
    pub body: String,
}

/// Jobs that ended since the last drain, and what to say about them.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct FinishedBatchRecord {
    pub jobs: Vec<FinishedRecord>,
    /// `None` when there's nothing worth saying (none ended, or all were cancelled).
    pub announcement: Option<AnnouncementRecord>,
    /// Jobs still queued or running.
    pub remaining: u32,
    /// The user's "notify me when jobs finish" setting.
    pub notify: bool,
}

pub fn finished_batch(jobs: Vec<FinishedRecord>, remaining: usize, notify: bool) -> FinishedBatchRecord {
    // The core words announcements from (title, status); only the outcome matters to it.
    let ended: Vec<_> = jobs
        .iter()
        .map(|f| {
            let status = match f.job.state {
                JobState::Failed => JobStatus::Failed(f.job.error.clone().unwrap_or_default()),
                JobState::Skipped => JobStatus::Skipped(f.job.detail.clone()),
                JobState::Cancelled => JobStatus::Cancelled,
                _ => JobStatus::Done(f.job.output_path.as_ref().map(PathBuf::from)),
            };
            (f.job.title.clone(), status)
        })
        .collect();
    let announcement = queue_watch::announcement(&ended, remaining)
        .map(|a| AnnouncementRecord { title: a.title, body: a.body });
    FinishedBatchRecord { jobs, announcement, remaining: remaining as u32, notify }
}

// ---- History ----------------------------------------------------------------------------

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct HistoryRecord {
    /// Position in the history (newest first); what `remove_history`/`download_again` take.
    pub index: u32,
    pub title: String,
    pub subtitle: String,
    pub url: Option<String>,
    pub output_path: Option<String>,
    /// Whether the output is still on disk.
    pub output_exists: bool,
    /// Unix seconds.
    pub finished_at: i64,
    pub is_download: bool,
    /// Whether "download/convert again" can work: always for downloads, and for conversions
    /// while every input is still there (Android's copies of picked files are deleted once
    /// converted).
    pub can_repeat: bool,
}

pub fn history_record(index: usize, e: &HistoryEntry) -> HistoryRecord {
    HistoryRecord {
        index: index as u32,
        title: e.title.clone(),
        subtitle: plain_text(&e.subtitle),
        url: e.url().map(str::to_owned),
        output_path: e.output.as_deref().map(path_string),
        output_exists: e.output.as_deref().is_some_and(Path::exists),
        finished_at: e.finished_at,
        is_download: matches!(e.spec, JobSpec::Download(_)),
        can_repeat: job_inputs(&e.spec).iter().all(|p| p.exists()),
    }
}

// ---- Settings ---------------------------------------------------------------------------

/// Sign-in and connection settings for every yt-dlp call. (No browser cookies on Android.)
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct NetworkRecord {
    /// A Netscape-format cookies.txt.
    pub cookies_file: Option<String>,
    /// e.g. `socks5://127.0.0.1:1080`; empty for none.
    pub proxy: String,
    /// e.g. `2M`; empty for unlimited.
    pub rate_limit: String,
    /// 0 keeps yt-dlp's default.
    pub retries: u32,
}

/// The settings the Download, Convert, Queue, Saved and Settings pages use. The player's are
/// [`PlayerSettingsRecord`]; the rest stay as they are when this is written back.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SettingsRecord {
    pub download_dir: String,
    /// Jobs at once, 1 to 6.
    pub concurrency: u32,
    /// Skip links already downloaded (yt-dlp's download archive).
    pub skip_downloaded: bool,
    /// Notify when jobs finish while Redline isn't in front.
    pub notify: bool,
    /// The Download page's remembered choices.
    pub download: DownloadOptionsRecord,
    pub network: NetworkRecord,
    /// The Convert page's shared options, presets, file names and folder.
    pub convert: ConvertSettingsRecord,
    /// Refresh saved collections not refreshed in the last 12 hours when Redline starts.
    pub refresh_saved_on_start: bool,
}

pub const MAX_CONCURRENCY: u32 = 6;

pub fn settings_record(s: &Settings) -> SettingsRecord {
    SettingsRecord {
        download_dir: path_string(&s.download_dir),
        concurrency: s.concurrency as u32,
        skip_downloaded: s.skip_downloaded,
        notify: s.notify,
        download: (&s.download).into(),
        network: NetworkRecord {
            cookies_file: s.network.cookies_file.as_deref().map(path_string),
            proxy: s.network.proxy.clone(),
            rate_limit: s.network.rate_limit.clone(),
            retries: s.network.retries,
        },
        convert: convert_settings_record(s),
        refresh_saved_on_start: s.refresh_saved_on_start,
    }
}

/// Writes `r` over the fields it covers, leaving the rest of `s` alone.
pub fn apply_settings(r: &SettingsRecord, s: &mut Settings) {
    if !r.download_dir.trim().is_empty() {
        s.download_dir = PathBuf::from(r.download_dir.trim());
    }
    s.concurrency = r.concurrency.clamp(1, MAX_CONCURRENCY) as usize;
    s.skip_downloaded = r.skip_downloaded;
    s.notify = r.notify;
    s.refresh_saved_on_start = r.refresh_saved_on_start;
    s.download = (&r.download).into();
    s.network = NetworkOptions {
        cookies_file: r.network.cookies_file.as_deref().filter(|f| !f.trim().is_empty()).map(PathBuf::from),
        proxy: r.network.proxy.clone(),
        rate_limit: r.network.rate_limit.clone(),
        retries: r.network.retries,
        ..s.network.clone()
    };
    apply_convert_settings(&r.convert, s);
}

// ---- Music ------------------------------------------------------------------------------

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// A page link (YouTube, SoundCloud…), streamed through yt-dlp when played.
    Link,
    /// A file on the device.
    File,
}

/// A playlist or queue entry. Goes back to Rust as it came (`offline` and `platform` are
/// worked out again there).
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct TrackRecord {
    pub kind: TrackKind,
    /// The link, or the file's path.
    pub location: String,
    pub title: String,
    pub artist: Option<String>,
    pub duration_secs: Option<f64>,
    /// Image URL.
    pub thumbnail: Option<String>,
    /// When it was added, Unix seconds.
    pub added_at: i64,
    /// The site a link is from, for its icon and color; `None` for files.
    pub platform: Option<crate::saved::Platform>,
    /// A link History has a downloaded copy of, which is what plays.
    pub offline: bool,
}

pub fn track_record(t: &Track, history: &History) -> TrackRecord {
    let (kind, location) = match &t.source {
        Source::Link(url) => (TrackKind::Link, url.clone()),
        Source::Local(p) => (TrackKind::File, path_string(p)),
    };
    TrackRecord {
        kind,
        location,
        title: t.title.clone(),
        artist: t.artist.clone(),
        duration_secs: t.duration,
        thumbnail: t.thumbnail.clone(),
        added_at: t.added_at,
        platform: t.platform().map(Into::into),
        offline: t.is_link() && matches!(library::playable_source(t, history), Source::Local(_)),
    }
}

impl TrackRecord {
    pub fn to_core(&self) -> Track {
        Track {
            source: match self.kind {
                TrackKind::Link => Source::Link(self.location.trim().to_owned()),
                TrackKind::File => Source::Local(PathBuf::from(&self.location)),
            },
            title: self.title.clone(),
            artist: self.artist.clone(),
            duration: self.duration_secs,
            thumbnail: self.thumbnail.clone(),
            added_at: self.added_at,
        }
    }
}

/// The track a History row stands for, as the desktop adds it to playlists: a download's link
/// (which plays its file while that's there), or a conversion's output while it exists.
pub fn history_track(entry: &HistoryEntry) -> Option<Track> {
    let output = entry.output.as_deref().filter(|p| playable_file(p));
    let source = match (entry.url(), output) {
        (Some(url), _) => Source::Link(url.to_owned()),
        (None, Some(p)) => Source::Local(p.to_path_buf()),
        _ => return None,
    };
    let title = match &entry.spec {
        JobSpec::Download(_) => entry.title.clone(),
        _ => source.label(),
    };
    Some(Track::new(source, title))
}

/// An audio or video file that's there to play.
pub fn playable_file(p: &Path) -> bool {
    p.is_file() && matches!(classify(p), Some(Category::Audio | Category::Video))
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PlaylistSummaryRecord {
    pub id: u64,
    pub name: String,
    pub track_count: u32,
    /// Of the tracks whose length is known.
    pub duration_secs: f64,
    /// The first track's picture that has one, for the cover.
    pub thumbnail: Option<String>,
}

pub fn playlist_summary(p: &Playlist) -> PlaylistSummaryRecord {
    PlaylistSummaryRecord {
        id: p.id,
        name: p.name.clone(),
        track_count: p.tracks.len() as u32,
        duration_secs: p.duration(),
        thumbnail: p.tracks.iter().find_map(|t| t.thumbnail.clone()),
    }
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PlaylistRecord {
    pub id: u64,
    pub name: String,
    pub duration_secs: f64,
    /// In playlist order; their positions are what `remove_track`/`move_track` take.
    pub tracks: Vec<TrackRecord>,
}

pub fn playlist_record(p: &Playlist, history: &History) -> PlaylistRecord {
    PlaylistRecord {
        id: p.id,
        name: p.name.clone(),
        duration_secs: p.duration(),
        tracks: p.tracks.iter().map(|t| track_record(t, history)).collect(),
    }
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerStatus {
    /// Nothing loaded.
    Idle,
    /// Looking the track up or waiting for its first audio.
    Loading,
    Playing,
    Paused,
    /// Played to the end with nothing after it.
    Ended,
    /// See [`PlayerSnapshotRecord::error`].
    Error,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    /// Back to the top after the last track.
    All,
    /// The current track again, until Next is pressed.
    One,
}

impl From<Repeat> for RepeatMode {
    fn from(r: Repeat) -> Self {
        match r {
            Repeat::Off => Self::Off,
            Repeat::All => Self::All,
            Repeat::One => Self::One,
        }
    }
}

impl From<RepeatMode> for Repeat {
    fn from(r: RepeatMode) -> Self {
        match r {
            RepeatMode::Off => Self::Off,
            RepeatMode::All => Self::All,
            RepeatMode::One => Self::One,
        }
    }
}

/// The player bar: what's playing, where it is, and the queue's shape.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PlayerSnapshotRecord {
    pub status: PlayerStatus,
    /// Why it stopped, when `status` is `Error`.
    pub error: Option<String>,
    /// The tags once the track is looked up; the queue's title (or the file name) until then.
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Image URL or file path.
    pub thumbnail: Option<String>,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    /// Waiting on the network or the decoder; nothing is heard.
    pub buffering: bool,
    /// Playing or about to (loading), as the play/pause button shows it.
    pub is_playing: bool,
    pub has_next: bool,
    pub has_prev: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// The current track's [`QueueTrackRecord::index`].
    pub queue_index: Option<u32>,
    /// Where the current track is in play order, from 0 ("3 of 10").
    pub queue_position: Option<u32>,
    pub queue_length: u32,
    /// The playlist the queue was started from.
    pub playlist_id: Option<u64>,
    /// Streaming a link (rather than a file, or a link's downloaded copy).
    pub is_link: bool,
    /// Counts loads, so one track's `Ended` can be told from the next's.
    pub load_id: u64,
    /// Changes whenever the queue's tracks, order or current track do; fetch `queue()` again
    /// only then.
    pub queue_version: u64,
}

pub fn player_snapshot(st: &PlayerState, now: &NowPlaying, queue_version: u64) -> PlayerSnapshotRecord {
    let current = now.current();
    let track = st.track.as_ref();
    let current_index = now.current_index();
    let label = st.source.as_ref().map(Source::label);
    PlayerSnapshotRecord {
        status: match st.status {
            Status::Idle => PlayerStatus::Idle,
            Status::Loading => PlayerStatus::Loading,
            Status::Playing => PlayerStatus::Playing,
            Status::Paused => PlayerStatus::Paused,
            Status::Ended => PlayerStatus::Ended,
            Status::Error(_) => PlayerStatus::Error,
        },
        error: match &st.status {
            Status::Error(e) => Some(e.clone()),
            _ => None,
        },
        // Until a track is looked up the player calls it by its URL or file name, and the
        // queue knows better.
        title: match (track, current) {
            (Some(t), Some(c)) if Some(&t.title) == label.as_ref() => Some(c.title.clone()),
            (Some(t), _) => Some(t.title.clone()),
            (None, c) => c.map(|c| c.title.clone()),
        },
        artist: track.and_then(|t| t.artist.clone()).or_else(|| current.and_then(|t| t.artist.clone())),
        album: track.and_then(|t| t.album.clone()),
        thumbnail: track
            .and_then(|t| t.thumbnail.clone())
            .or_else(|| current.and_then(|t| t.thumbnail.clone()))
            // A song file with no picture of its own: the app reads the art out of the file.
            .or_else(|| match &st.source {
                Some(Source::Local(p)) => Some(path_string(p)),
                _ => None,
            }),
        position_secs: st.position,
        duration_secs: st.duration.or_else(|| current.and_then(|t| t.duration)),
        buffering: st.buffering,
        is_playing: st.is_playing(),
        has_next: now.has_next(),
        has_prev: now.has_prev(),
        shuffle: now.shuffle,
        repeat: now.repeat.into(),
        queue_index: current_index.map(|i| i as u32),
        queue_position: current_index.and_then(|c| now.in_order().position(|(i, _)| i == c)).map(|p| p as u32),
        queue_length: now.tracks.len() as u32,
        playlist_id: now.playlist,
        is_link: matches!(st.source, Some(Source::Link(_))),
        load_id: st.load_id,
        queue_version,
    }
}

/// One row of Now playing.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct QueueTrackRecord {
    /// What `jump` and `remove_from_queue` take. Stays put when shuffle reorders the queue.
    pub index: u32,
    pub track: TrackRecord,
    pub is_current: bool,
}

/// Now playing in play order.
pub fn queue_records(now: &NowPlaying, history: &History) -> Vec<QueueTrackRecord> {
    let current = now.current_index();
    now.in_order()
        .map(|(i, t)| QueueTrackRecord { index: i as u32, track: track_record(t, history), is_current: current == Some(i) })
        .collect()
}

/// The equalizer, as the desktop's EQ panel edits it.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct EqRecord {
    /// Off bypasses it.
    pub enabled: bool,
    /// dB, [`PlayerChoicesRecord::preamp_min_db`] to `preamp_max_db`.
    pub preamp_db: f32,
    /// dB per band, ± [`PlayerChoicesRecord::band_db`], at [`PlayerChoicesRecord::eq_freqs`].
    pub bands_db: Vec<f32>,
}

impl From<&EqSettings> for EqRecord {
    fn from(e: &EqSettings) -> Self {
        Self { enabled: e.enabled, preamp_db: e.preamp_db, bands_db: e.bands_db.to_vec() }
    }
}

impl From<&EqRecord> for EqSettings {
    /// Out-of-range gains are clamped; missing bands are flat and extra ones ignored.
    fn from(e: &EqRecord) -> Self {
        let mut bands_db = [0.0; 10];
        for (b, v) in bands_db.iter_mut().zip(&e.bands_db) {
            *b = if v.is_finite() { v.clamp(-dsp::BAND_DB, dsp::BAND_DB) } else { 0.0 };
        }
        let preamp_db = if e.preamp_db.is_finite() { e.preamp_db } else { 0.0 };
        Self { enabled: e.enabled, preamp_db: preamp_db.clamp(dsp::PREAMP_MIN_DB, dsp::PREAMP_MAX_DB), bands_db }
    }
}

/// The player's saved controls.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PlayerSettingsRecord {
    /// 0 to 1.
    pub volume: f32,
    /// 0.5 to 2; pitch follows.
    pub speed: f32,
    /// Back to 1x whenever a new track loads.
    pub reset_speed: bool,
    pub eq: EqRecord,
    /// The EQ preset the bands match, if any.
    pub eq_preset: Option<String>,
}

pub fn player_settings_record(p: &PlayerSettings) -> PlayerSettingsRecord {
    PlayerSettingsRecord {
        volume: p.volume,
        speed: p.speed,
        reset_speed: p.reset_speed,
        eq: (&p.eq).into(),
        eq_preset: p.eq.preset().map(str::to_owned),
    }
}

/// A named EQ curve.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct EqPresetRecord {
    pub name: String,
    pub preamp_db: f32,
    pub bands_db: Vec<f32>,
}

/// What the speed and EQ controls offer, as the desktop's panels do.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PlayerChoicesRecord {
    pub min_speed: f32,
    pub max_speed: f32,
    /// The speed chips.
    pub speeds: Vec<f32>,
    /// Each band's centre, Hz.
    pub eq_freqs: Vec<f32>,
    /// Bands go from -this to +this dB.
    pub band_db: f32,
    pub preamp_min_db: f32,
    pub preamp_max_db: f32,
    pub eq_presets: Vec<EqPresetRecord>,
}

/// The desktop's speed chips.
const SPEEDS: [f32; 5] = [0.75, 1.0, 1.25, 1.5, 2.0];

#[uniffi::export]
pub fn player_choices() -> PlayerChoicesRecord {
    PlayerChoicesRecord {
        min_speed: dsp::MIN_SPEED,
        max_speed: dsp::MAX_SPEED,
        speeds: SPEEDS.to_vec(),
        eq_freqs: dsp::EQ_FREQS.to_vec(),
        band_db: dsp::BAND_DB,
        preamp_min_db: dsp::PREAMP_MIN_DB,
        preamp_max_db: dsp::PREAMP_MAX_DB,
        eq_presets: dsp::EQ_PRESETS
            .iter()
            .map(|(name, pre, bands)| EqPresetRecord { name: (*name).to_owned(), preamp_db: *pre, bands_db: bands.to_vec() })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redline_core::tools::ytdlp::DownloadRequest;

    #[test]
    fn env_is_sorted_so_reinit_matches() {
        let env: HashMap<_, _> = [("PYTHONHOME", "/p"), ("HOME", "/h"), ("LD_LIBRARY_PATH", "/l")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let keys: Vec<_> = child_env(&env).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["HOME", "LD_LIBRARY_PATH", "PYTHONHOME"]);
    }

    #[test]
    fn options_round_trip() {
        let mut o = DownloadOptions::default();
        o.mode = Mode::Audio;
        o.subs_mode = SubsMode::Srt;
        o.audio_bitrate = "320K".into();
        let r = DownloadOptionsRecord::from(&o);
        assert_eq!(r.mode, DownloadMode::Audio);
        assert_eq!(DownloadOptions::from(&r), o);
        assert_eq!(options_summary(r), "Audio · MP3 · 320 kbps");
    }

    #[test]
    fn choice_lists_match_the_desktop() {
        let c = download_choice_lists();
        assert_eq!(c.video_qualities[0].max_height, 0);
        assert!(c.containers.contains(&"mp4".to_owned()));
        assert_eq!(c.audio_bitrates[0].value, "0");
    }

    #[test]
    fn trims_are_checked_like_the_desktop() {
        let t = |start, end| Some(TrimRecord { start_secs: start, end_secs: end, precise: false });
        assert_eq!(check_trim(None, Some(60.0)), Ok(None));
        assert!(check_trim(t(10.0, Some(5.0)), Some(60.0)).is_err(), "end before start");
        assert!(check_trim(t(70.0, None), Some(60.0)).is_err(), "past the end");
        assert!(check_trim(t(0.0, Some(60.0)), Some(59.5)).is_err(), "keeps everything");
        assert!(check_trim(t(0.0, None), None).is_err(), "keeps everything");
        assert!(check_trim(t(-1.0, Some(5.0)), None).is_err());
        let ok = check_trim(t(5.0, Some(65.5)), Some(120.0)).unwrap().unwrap();
        assert_eq!((ok.start, ok.end), (5.0, Some(65.5)));
        assert!(check_trim(t(0.0, Some(30.0)), None).unwrap().is_some(), "unknown length, first 30 s");
    }

    fn info() -> MediaInfo {
        MediaInfo {
            title: "Video".into(),
            uploader: Some("Chan".into()),
            duration: Some(100.0),
            formats: vec![
                FormatInfo { id: "137".into(), height: Some(1080), vcodec: Some("avc1".into()), tbr: Some(800.0), ..Default::default() },
                FormatInfo { id: "140".into(), acodec: Some("mp4a".into()), tbr: Some(128.0), ..Default::default() },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn media_info_carries_sizes_and_tags() {
        let r = media_info_record("https://x/v", &info());
        assert_eq!(r.tags.artist, "Chan");
        assert_eq!(r.heights.len(), 1);
        assert_eq!(r.formats[0].quality_label, "1080p");
        assert_eq!(r.formats[0].size, Some(10_000_000), "estimated from bitrate and length");
        assert_eq!(r.best_audio_size, Some(1_600_000));
        assert_eq!(r.heights[0].size, Some(11_600_000), "video plus the best audio");
    }

    #[test]
    fn plans_from_choices_like_the_download_page() {
        let mut options: DownloadOptionsRecord = (&DownloadOptions::default()).into();
        options.mode = DownloadMode::Audio;
        let mut c = DownloadChoicesRecord {
            url: "https://x/v".into(),
            options,
            pick_video: None,
            pick_audio: Some("140".into()),
            tags: Some(TagsRecord { title: "Fixed".into(), artist: "Chan".into(), album: String::new() }),
            trim: None,
            not_before: None,
        };
        let mut settings = Settings::default();
        settings.download = (&c.options).into();
        let plan = link_choices(Some(info()), &c).plan(&c.url, None, &settings);
        let JobSpec::Download(r) = plan.spec else { panic!() };
        assert_eq!(r.format.as_deref(), Some("140"));
        assert_eq!(r.tags.map(|t| t.title).as_deref(), Some("Fixed"));

        // Without a lookup there's nothing to compare tags or picks against.
        let JobSpec::Download(r) = link_choices(None, &c).plan(&c.url, None, &settings).spec else { panic!() };
        assert_eq!((r.format, r.tags), (None, None));

        c.tags = None;
        let JobSpec::Download(r) = link_choices(Some(info()), &c).plan(&c.url, None, &settings).spec else { panic!() };
        assert_eq!(r.tags, None, "unchanged tags leave yt-dlp's metadata");
    }

    fn job(status: JobStatus) -> Job {
        let mut m = redline_core::jobs::JobManager::new();
        m.push(
            "Song".into(),
            "Audio · MP3".into(),
            JobSpec::Download(DownloadRequest { url: "https://x/a".into(), out_dir: "dl".into(), ..Default::default() }),
        );
        let mut job = m.jobs.remove(0);
        job.status = status;
        job
    }

    #[test]
    fn job_rows() {
        let r = job_record(&job(JobStatus::Running { progress: Some(0.5), detail: "50%".into() }));
        assert_eq!((r.state, r.progress, r.detail.as_str()), (JobState::Running, Some(0.5), "50%"));
        assert_eq!(r.url.as_deref(), Some("https://x/a"));
        assert!(r.is_download && r.state.is_active());

        let r = job_record(&job(JobStatus::Failed("Bad link\n\nhint".into())));
        assert_eq!(r.detail, "Bad link");
        assert_eq!(r.error.as_deref(), Some("Bad link\n\nhint"));

        let out = PathBuf::from("dl").join("Song.mp3");
        let r = job_record(&job(JobStatus::Done(Some(out.clone()))));
        assert_eq!(r.detail, "Song.mp3");
        assert_eq!(r.output_path, Some(path_string(&out)));
        assert_eq!(r.output_dir, Some("dl".to_owned()));
        assert!(!r.state.is_active());
    }

    #[test]
    fn icon_glyphs_become_plain_text() {
        let s = format!("MKV {} MP4", redline_core::format::ARROW_RIGHT);
        assert_eq!(plain_text(&s), "MKV \u{2192} MP4");
    }

    #[test]
    fn conversions_list_what_they_read() {
        use redline_core::tools::convert::{ConcatRequest, ConvertRequest};
        let convert = JobSpec::Convert(ConvertRequest {
            input: "/in/1/a.mkv".into(),
            burn_subs: Some(SubSource::File("/in/2/a.srt".into())),
            ..Default::default()
        });
        assert_eq!(job_inputs(&convert), [PathBuf::from("/in/1/a.mkv"), PathBuf::from("/in/2/a.srt")]);
        let join = JobSpec::Concat(ConcatRequest { inputs: vec!["/a.mp3".into(), "/b.mp3".into()], ..Default::default() });
        assert_eq!(job_inputs(&join).len(), 2);
        assert!(job_inputs(&JobSpec::Download(DownloadRequest::default())).is_empty());

        let mut m = redline_core::jobs::JobManager::new();
        m.push("a.mkv".into(), "MKV → MP4".into(), convert);
        let entry = HistoryEntry {
            title: "a.mkv".into(),
            subtitle: String::new(),
            spec: m.jobs[0].spec.clone(),
            output: None,
            finished_at: 0,
        };
        assert!(!history_record(0, &entry).can_repeat, "the copied input is gone");
        assert_eq!(job_record(&m.jobs[0]).inputs.len(), 2);
    }

    #[test]
    fn finished_batches_announce_like_the_desktop() {
        let done = FinishedRecord { job: job_record(&job(JobStatus::Done(None))) };
        let failed = FinishedRecord { job: job_record(&job(JobStatus::Failed("x".into()))) };
        let b = finished_batch(vec![done.clone()], 0, true);
        assert_eq!(b.announcement, Some(AnnouncementRecord { title: "Finished".into(), body: "Song".into() }));
        let b = finished_batch(vec![done, failed], 2, false);
        assert_eq!(b.announcement.unwrap().body, "1 finished, 1 failed · 2 still in the queue");
        assert!(!b.notify);
        let cancelled = FinishedRecord { job: job_record(&job(JobStatus::Cancelled)) };
        assert_eq!(finished_batch(vec![cancelled], 0, true).announcement, None);
    }

    #[test]
    fn settings_write_back_only_their_fields() {
        let mut s = Settings::default();
        s.network.cookies_browser = Some("firefox".into());
        s.convert_dir = Some("conv".into());
        let mut r = settings_record(&s);
        r.concurrency = 40;
        r.download_dir = "  ".into();
        r.network.proxy = "socks5://h:1".into();
        r.download.mode = DownloadMode::Audio;
        r.refresh_saved_on_start = false;
        let before = s.download_dir.clone();
        apply_settings(&r, &mut s);
        assert_eq!(s.concurrency, 6);
        assert_eq!(s.download_dir, before, "a blank folder is ignored");
        assert_eq!(s.network.proxy, "socks5://h:1");
        assert_eq!(s.network.cookies_browser.as_deref(), Some("firefox"), "kept");
        assert_eq!(s.convert_dir, Some(PathBuf::from("conv")), "kept");
        assert_eq!(s.download.mode, Mode::Audio);
        assert!(!s.refresh_saved_on_start);
        assert_eq!(settings_record(&s), SettingsRecord { concurrency: 6, download_dir: path_string(&before), ..r });
    }

    #[test]
    fn tracks_round_trip() {
        let mut link = Track::new(Source::Link("https://youtu.be/abc".into()), "Song".into());
        link.artist = Some("Band".into());
        link.duration = Some(200.5);
        link.thumbnail = Some("https://img/abc.jpg".into());
        let file = Track::new(Source::Local(PathBuf::from("music").join("a.flac")), "A".into());
        let history = History::default();
        for t in [&link, &file] {
            assert_eq!(&track_record(t, &history).to_core(), t);
        }
        let r = track_record(&link, &history);
        assert_eq!((r.kind, r.platform, r.offline), (TrackKind::Link, Some(crate::saved::Platform::YouTube), false));
        let r = track_record(&file, &history);
        assert_eq!((r.kind, r.platform, r.location), (TrackKind::File, None, path_string(&PathBuf::from("music").join("a.flac"))));
        let padded = TrackRecord { location: "  https://youtu.be/abc ".into(), ..track_record(&link, &history) };
        assert_eq!(padded.to_core(), link, "links are trimmed");
    }

    #[test]
    fn offline_tracks_and_history_rows() {
        let dir = std::env::temp_dir().join(format!("redline-mobile-offline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("song.mp3");
        std::fs::write(&file, b"").unwrap();
        let mut done = job(JobStatus::Done(Some(file.clone())));
        done.title = "Downloaded".into();
        let mut history = History::default();
        history.record(&done);
        let track = Track::new(Source::Link("https://x/a".into()), "Song".into());
        assert!(track_record(&track, &history).offline, "plays from the download");

        let row = history_track(&history.entries[0]).unwrap();
        assert_eq!((row.source, row.title.as_str()), (Source::Link("https://x/a".into()), "Downloaded"));
        let converted = HistoryEntry {
            title: "a.mkv".into(),
            subtitle: String::new(),
            spec: JobSpec::Convert(Default::default()),
            output: Some(file.clone()),
            finished_at: 0,
        };
        assert_eq!(history_track(&converted).unwrap().source, Source::Local(file.clone()));
        std::fs::remove_file(&file).unwrap();
        assert!(history_track(&converted).is_none(), "the output is gone");
        assert!(!track_record(&track, &history).offline, "streams again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshots_fill_in_from_the_queue() {
        let mut now = NowPlaying::default();
        let mut song = Track::new(Source::Link("https://x/a".into()), "Song".into());
        song.thumbnail = Some("https://img/a.jpg".into());
        now.start(vec![song, Track::new(Source::Local("b.mp3".into()), "B".into())], 0, Some(4));
        let mut st = PlayerState {
            status: Status::Loading,
            source: Some(Source::Link("https://x/a".into())),
            track: Some(redline_core::player::TrackInfo { title: "https://x/a".into(), ..Default::default() }),
            ..Default::default()
        };
        let s = player_snapshot(&st, &now, 7);
        assert_eq!(s.title.as_deref(), Some("Song"), "not the URL while it loads");
        assert_eq!(s.thumbnail.as_deref(), Some("https://img/a.jpg"));
        assert_eq!((s.status, s.is_playing, s.is_link), (PlayerStatus::Loading, true, true));
        assert_eq!((s.queue_index, s.queue_position, s.queue_length, s.playlist_id), (Some(0), Some(0), 2, Some(4)));
        assert!(s.has_next && !s.has_prev);
        assert_eq!(s.queue_version, 7);

        st.status = Status::Error("No sound".into());
        st.track = Some(redline_core::player::TrackInfo { title: "Real title".into(), ..Default::default() });
        let s = player_snapshot(&st, &now, 7);
        assert_eq!((s.status, s.error.as_deref(), s.title.as_deref()), (PlayerStatus::Error, Some("No sound"), Some("Real title")));
        let s = player_snapshot(&PlayerState::default(), &NowPlaying::default(), 0);
        assert_eq!((s.status, s.title, s.queue_index), (PlayerStatus::Idle, None, None));
        assert_eq!(Repeat::from(RepeatMode::One), Repeat::One);
    }

    #[test]
    fn eq_and_player_choices() {
        let eq = EqSettings::from(&EqRecord { enabled: true, preamp_db: f32::NAN, bands_db: vec![-99.0; 12] });
        assert_eq!((eq.preamp_db, eq.bands_db), (0.0, [-dsp::BAND_DB; 10]));
        let r = player_settings_record(&PlayerSettings::default());
        assert_eq!((r.volume, r.speed, r.eq_preset.as_deref()), (0.8, 1.0, Some("Flat")));
        assert_eq!(EqSettings::from(&r.eq), EqSettings::default());
        let c = player_choices();
        assert_eq!((c.eq_freqs.len(), c.eq_presets.len()), (10, dsp::EQ_PRESETS.len()));
        assert!(c.speeds.contains(&1.0) && c.min_speed == 0.5 && c.max_speed == 2.0);
    }
}
