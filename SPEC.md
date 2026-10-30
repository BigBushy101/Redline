# Redline — Context Spec Sheet

> **What this is.** A single-file briefing on the Redline codebase: what it does, how it is put together, the data it keeps, and the rules each feature follows. Written to be dropped into an AI context window or read by a new contributor before changing code. It describes the code as it stands at the snapshot below; it is not a roadmap.
>
> **Snapshot:** 0.1.4 · after commit 0160dab (2026-10-25), plus the 0.1.4 streaming changes · written 2026-10-29
> **Size:** ~25.8k lines of Rust (including tests) and ~9.4k lines of Kotlin.
>
> **How to use it:** read 3 (architecture) first, then the feature section you are touching; 4-5 explain the data and where it lives; 12 lists the rules that must not be broken; appendices A and B are quick lookups.

**Contents:** 1 Product · 2 Repository map · 3 Architecture · 4 Domain model · 5 Persistence · 6 Feature rules · 7 Desktop UI · 8 Android app · 9 Cross-platform differences · 10 Build, run, release · 11 Testing · 12 Conventions and invariants · 13 Gaps, risks and questions · A Constants · B Change map · C Documents

---

## 1. Product

Redline is a desktop-and-phone tool for **getting media, converting it, and playing it**:

1. **Download** — paste a link (YouTube, SoundCloud, Bandcamp, Vimeo, Mixcloud, any of yt-dlp's ~1000 sites), optionally trim it, pick formats/subtitles/tags, and queue it. yt-dlp downloads; ffmpeg merges and post-processes.
2. **Convert** — drop local files and turn them into another format, apply filters, burn subtitles, target a file size, or join several files into one. Images go through the image crate; everything else through ffmpeg.
3. **Listen** — stream links or play local files with a 10-band EQ, variable speed, A-B loop, shuffle/repeat and a queue built from playlists that mix links and files. Saved music from each platform is kept in sync with the original.

### Platform support

| Capability | Windows desktop | Android |
| --- | :---: | :---: |
| Download with yt-dlp | yes | yes |
| Convert/join with ffmpeg (images too) | yes | yes |
| Saved by platform (synced collections) | yes | yes |
| Playlists mixing links and local files | yes | yes |
| Local library scan | yes (filesystem) | yes (MediaStore) |
| Player: queue, shuffle, repeat, speed, EQ, A-B loop | yes | yes |
| Export/import saved links | yes | yes |
| Dead-link detection + Find replacement | yes | yes |
| Windows media keys / SMTC overlay | yes | n/a |
| Android media session, notification, audio focus | n/a | yes |
| Share sheet (link to Download, media to Convert) | n/a | yes |

Desktop is Windows-first: the Win32-specific parts (SMTC, keep-awake, sleep/shutdown, the tools installer) sit behind cfg gates, and the rest of the core has unix code paths (cpal output, killpg, trash). Linux/macOS builds are not a tested target and would need ffmpeg installed by hand.

---

## 2. Repository map

~~~text
Redline/
├─ Cargo.toml                 workspace: ".", redline-core, redline-mobile; package redline 0.1.3
├─ src/                       DESKTOP UI (7.0k LOC, egui/eframe 0.36)
│  ├─ main.rs                 NativeOptions: 1200x780, min 900x600, drag and drop
│  ├─ app.rs                  App state, Page enum, frame loop, playback orchestration
│  ├─ task.rs                 Task<T>: one-shot background result polled each frame
│  ├─ theme.rs                dark Spotify-like theme, runtime accent, shared widgets
│  └─ ui/                     download, convert, saved, playlists, local, queue, history,
│                             settings, now_bar, player_panels, replace, tracks, presets, sidebar
├─ redline-core/              SHARED LOGIC (13.8k LOC, no UI dependency)
│  └─ src/
│     ├─ tools/               yt-dlp driver + worker, ffmpeg, filters, hwaccel, image_conv, install, e2e
│     ├─ player/              engine (mod.rs), decode, render, dsp, cache, ab_loop, output, aaudio, tests, e2e
│     ├─ jobs.rs              job queue + worker threads + persistence
│     ├─ library.rs           Track / Playlist / NowPlaying
│     ├─ saved.rs             platform collections, sync, export/import
│     ├─ linkcheck.rs         link status cache, checks, replacements
│     ├─ local.rs             local song scan cache
│     ├─ settings.rs          Settings, AppDirs, data/download dirs
│     ├─ convert_plan.rs      UI list to Convert/Concat jobs
│     ├─ download_plan.rs     lookup + choices to Download job
│     ├─ presets.rs           built-in + user presets
│     ├─ queue_watch.rs       announcements, keep-awake, after-queue action
│     ├─ platform.rs          Platform + canonical URL
│     ├─ format.rs            durations, sizes, timestamps, ago()
│     ├─ system.rs            notifications, sleep/shutdown, keep awake
│     ├─ media_keys.rs        Windows SMTC (cfg windows)
│     └─ android.rs           Android init: folders, bundled tools, tool symlinks
├─ redline-mobile/            UNIFFI BRIDGE (5.0k LOC)
│  └─ src/                    engine.rs, music.rs, saved.rs, convert.rs, records.rs, format.rs, error.rs
├─ android/                   ANDROID APP (9.4k LOC Kotlin/Compose + Gradle)
│  ├─ app/src/main/java/app/redline/
│  │  ├─ MainActivity.kt      one activity, share intents, NavModel
│  │  ├─ RedlineApp.kt        Application: owns RedlineState, Coil loader
│  │  ├─ RedlineEngine.kt     paths + youtubedl-android init produces Engine
│  │  ├─ state/               RedlineState, MusicState, ConvertInputs, ImportedMusic, LocalFiles
│  │  ├─ ui/                  theme, components, download, convert, saved, playlists, local,
│  │  │                       queue, history, library, player, settings
│  │  ├─ work/                DownloadService, PlaybackService, Notifications
│  │  └─ util/                Art, AudioCover, OpenFile, StoragePaths, Time
│  ├─ app/build.gradle.kts    compileSdk 37, minSdk 26, versionName 0.1.3, versionCode 4
│  └─ gradle/libs.versions.toml
├─ assets/                    Inter-Regular.ttf, Inter-Bold.ttf (embedded)
├─ scripts/release.ps1        builds both artefacts into dist/, optional gh release
├─ PLAYER_PROMPT.md           historical brief: core/UI split + player phases 0-5
├─ SAVED_MUSIC_PLAN.md        historical brief: Saved-by-platform feature
├─ dist/, target/, android/apk/   build outputs (git-ignored)
└─ .optim-jev/                local tooling scratch, git-ignored
~~~

No CI configuration and no LICENSE file are present. .claude/settings.local.json allows two local tool permissions and is git-ignored.

---

## 3. Architecture

### 3.1 Crates and dependency direction

| Crate / app | Kind | Depends on | Responsibility |
| --- | --- | --- | --- |
| redline 0.1.3 | bin + workspace root | redline-core, eframe 0.36, egui_extras, egui-phosphor 0.14, rfd 0.17, open 5, raw-window-handle 0.6 | the egui UI; owns UI-only state (text fields, open panels, toasts) |
| redline-core 0.1.0 | lib | ureq 3, zip 9, serde, serde_json, image 0.25, anyhow, chrono, rtrb 0.4; desktop: directories, open, trash, notify-rust, cpal 0.18.2; windows: souvlaki 0.8.3, windows-sys 0.59; android: ndk 0.9; unix: libc | everything Redline does without drawing it |
| redline-mobile 0.1.0 | cdylib + lib | redline-core, uniffi 0.32.2 | UniFFI surface for Kotlin, package app.redline.core |
| Android app | Kotlin/Compose | AndroidX, Compose BOM 2026.09, Coil 3.6.3, youtubedl-android 0.18.1, JNA 5.19.1 | UI, Android services, storage, notifications |

**Hard rule:** redline-core must never depend on egui/eframe. It is drivable and testable headless; the Android app proves it. If a feature needs a repaint, it calls a **Wake** callback, never egui.

### 3.2 Runtime threads (desktop)

| Thread | Created by | Work |
| --- | --- | --- |
| UI / main | eframe | one App::logic pass + one App::ui pass per frame; polls every Task result |
| Job workers | JobManager::pump | one OS thread per running job, at most settings.concurrency (UI allows 1-6) |
| Player engine | Player::spawn, thread name "player" | owns the audio output and decoder; applies Command messages; wakes the UI about 10x/s while playing |
| Stream reader | player::decode, one per active stream | ffmpeg stdout into a lock-free ring buffer |
| Cache filler | player::cache | decodes the current link to a temp file as fast as it can |
| Audio callback | cpal / AAudio | Renderer::render pulls the ring buffer, applies speed/EQ, writes device buffers; never blocks or locks |
| Link-check workers | linkcheck.rs, thread name "redline-linkcheck", up to 3 | oEmbed / yt-dlp --simulate checks, throttled |
| yt-dlp worker | tools::ytdlp_worker | long-lived yt-dlp child driven by an extractor plugin + request channel (stream look-ups); warmed at app start |
| Installer | tools::install | downloads yt-dlp/ffmpeg/Deno once |
| One-shot tasks | task::Task | encoder detection, saved syncs, scans, lookups, yt-dlp update |
| SMTC callback | souvlaki | media-key events; play/pause go straight to the player |

Android adds the engine queue thread, a music follower thread, a saved-sync thread, Kotlin Dispatchers.IO.limitedParallelism(1) for quick FFI calls, Dispatchers.IO for yt-dlp calls, and the two foreground services.

### 3.3 The desktop frame loop

This eframe 0.36 build calls two methods each frame (App::logic and App::ui, rather than the stock update).

**App::logic(ctx)** — in order:

1. Installer finished: relocate ToolPaths, tell the player and link checks, invalidate the settings page version, re-detect encoders.
2. Copy the player snapshot into App::player_state.
3. sync_player: push changed volume/speed/EQ; reset speed on a new load when configured.
4. follow_queue: catch NowPlaying up with the player's self-advances; hand SetNext the next track's playable source.
5. watch_failures: PlayWatch notes failed links; skip when it will not play; keep the failure for the notice.
6. Windows media keys: apply Next/Prev/Seek/Raise; update the overlay.
7. Poll page tasks: playlists adding scans, saved lookups/syncs/launch refresh, local scan, replacement searches.
8. Network settings changed: push SetNetwork to the player and the link checks.
9. Encoder detection result.
10. jobs.pump starts due jobs; returns the delay until the next scheduled one (request_repaint_after).
11. Finished jobs: history.record, watch.job_ended, collect (title, status); announce when unfocused.
12. QueueWatch.update: keep-awake and the after-queue action (open folder / sleep / shut down).
13. save_if_dirty on jobs, history, library, saved; save settings if changed.

**App::ui(ui)** — dropped files, shutdown countdown, replace dialog, player panels, bottom now-bar (84 px), left sidebar (260 px), central page, toast (4 s), failure notice (15 s), drag-hover overlay.

### 3.4 Wake contract

~~~rust
pub type Wake = Arc<dyn Fn() + Send + Sync>;   // core/src/lib.rs
pub fn no_wake() -> Wake;                       // headless use and tests
~~~

Desktop sets it to ctx.request_repaint (src/task.rs::waker); Android sets it to a listener that pokes the engine/music thread; tests use no_wake. Every producer (Reporter, JobManager, LinkCheck, Player, page Tasks) calls it after changing state the UI shows. The UI is pull-only: it reads snapshots each frame and never receives pushes.

### 3.5 Android run loop

* **Engine thread** (Rust): wakes on a Signal (condvar + flags) from API calls or workers; each step runs JobManager::pump, records finished jobs into History + a finished batch, saves dirty files and changed settings. It sleeps up to 30 s idle; scheduled jobs shorten it.
* **Music follower thread**: mirrors the desktop's player work (applies settings, follows the queue on advances, handles failures, saves playlists, runs saved syncs), then nudges Kotlin.
* **Kotlin** collapses nudges in a CONFLATED channel, pulls snapshots on the IO dispatcher, and publishes them as StateFlow on the main thread. on_change is called at most about 10x/s.

---

## 4. Domain model

### 4.1 Playback and library (redline-core)

**player::Source** — Local(PathBuf) or Link(String) (a page URL, resolved at play time). label() gives a file stem or the URL.

**player::TrackInfo** — title, artist, album, duration, thumbnail (URL or file path).

**player::Status** — Idle, Loading, Playing, Paused, Ended, Error(String).

**player::PlayerState** — status, source, track, position (seconds, source time), duration, buffering, volume, load_id (counts loads), advances (counts self-advances), ab_loop (AbLoop), next_failed (link + error).

**library::Track**

| Field | Type | Notes |
| --- | --- | --- |
| source | Source | link or local file |
| title | String | |
| artist | Option<String> | |
| duration | Option<f64> | seconds |
| thumbnail | Option<String> | image URL or file:// path |
| added_at | i64 | Unix seconds |
| replaced_from | Option<String> | the first link a replacement ever replaced |

Track methods: new, swap_link(Candidate), is_link, platform() (from the URL), key() (canonical link, or the file path as a string), matches(query) over title and artist.

**library::Playlist** — id: u64, name, tracks: Vec<Track>, duration().

**library::Library** — playlists: Vec<Playlist>, dirty, unreadable (the file existed but failed to parse; nothing is discarded until a save succeeds). Operations: get / get_mut / create / rename / delete / add / remove_track / replace_link / move_track; load and save_if_dirty.

**library::NowPlaying**

| Field | Meaning |
| --- | --- |
| tracks | tracks as added; stable indices |
| order | play order (indices into tracks); shuffled when shuffle is on |
| pos | position inside order |
| shuffle | bool |
| repeat | Repeat::Off / All / One |
| playlist | Option<PlaylistId> the queue started from |

Rules: start() replaces the queue and reshuffles when shuffle is on; append adds after the end; insert_next ("Play next") inserts after the current track in both list order and play order; remove keeps the current track current; jump goes by track index; has_next/has_prev account for repeat-all; next/prev are not held back by repeat-one, but peek_after_end/after_end repeat the same track under repeat-one; clear resets; set_shuffle reshuffles from a seed.

**library::Repeat** cycles Off, All, One, Off.

**player::Command** — Load(Source, start_at), Play, Pause, Toggle, Seek(secs), SetVolume(f32), SetSpeed(f32), SetEq(EqSettings), Stop, SetTools(ToolPaths), SetNetwork(NetworkOptions), SetNext(Option<Source>), SetLoopA(f64), SetLoopB(f64), ClearLoop.

**player::PlayerSettings** — volume (default 0.8), speed (1.0), reset_speed (false), eq (enabled, flat).

**dsp::EqSettings** — enabled, preamp_db (-12..+6), bands_db [f32; 10].

### 4.2 Jobs and history

**JobId** = u64.

**JobSpec** — Download(DownloadRequest) | Convert(ConvertRequest) | Concat(ConcatRequest). Cloned into worker threads and serialised into queue.json.

**JobStatus** — Queued, Running { progress: Option<f32>, detail }, Processing(String), Done(Option<PathBuf>), Skipped(String), Failed(String), Cancelled. is_active = Queued | Running | Processing.

**Job** — id, title, subtitle, spec, status, not_before: Option<i64>, cancel: CancelHandle (not serialised).

**CancelHandle** — AtomicBool + Mutex<Option<Child>>; cancel() sets the flag and kills the tree; attach() kills immediately when already cancelled; wait() polls try_wait so cancellation can still take the lock.

**HistoryEntry** — title, subtitle, spec, output, finished_at; url() (downloads only), matches(query) over title, subtitle, URL and input paths.

**History** — entries newest first, dirty, by_url index keyed by platform::canonical (built once, invalidated on change). Only JobStatus::Done is recorded. Cap 2000.

### 4.3 Saved

**SavedTrack** — track: Track, removed_upstream: bool.

**Collection** — id, name, source_url, tracks (platform order), synced_at: Option<i64>, keep_offline, sync_error, extractor_platform (fallback only when the host is unrecognised).

**Saved** — collections: Vec<Collection>, songs: Vec<Track> (single saved links), dirty, keys cache (HashSet of canonical keys).

**SyncResult** — added: Vec<Track>, removed: Vec<Track>, restored: usize, order: Vec<SavedTrack>, is_unchanged().

**SaveOutcome** — Collection{id,name,songs} | Refreshed{id,name,sync} | Song{title} | AlreadySaved; message() words the toast.

**Import / MergeReport** — parsed export payload and the counts the toast reports.

### 4.4 Links

**LinkStatus** — Unknown (default), Alive, Restricted (private or blocked), Dead. is_dead(), label().

**Answer** — Status(LinkStatus) | NoAnswer | SlowDown (HTTP 429 or bot check).

**Checked** (on disk) — status, at: i64, deep: bool.

**FailedPlay** — track, error, skip: bool, playlist: Option<PlaylistId>.

**Candidate / Ranked** — Candidate: url, title, uploader, duration, thumbnail. Ranked: candidate, title_score, delta_secs, score; is_clear_match() = title_score >= 0.6 and |delta_secs| <= 3.

**FixItem** — index, track, ranked; clear_match() gives the pick that may be applied silently.

### 4.5 Download model

**Mode** — Video | Audio.

**DownloadOptions** (all persisted; defaults in brackets): mode (Video), max_height (1080; 0 = best), container (mp4), audio_format (mp3), audio_bitrate ("0" = best VBR), embed_thumbnail (true), embed_metadata (true), embed_subs (false), playlist (false), remove_sponsors (false), sub_langs ("en,en-US,en-GB,en-orig"), auto_subs (false), subs_mode (Embed), split_chapters (false), square_cover (false).

**SubsMode** — Embed | Srt.

**Tags** — title, artist, album (all String; empty leaves the site's value).

**NetworkOptions** — cookies_browser: Option<String> (firefox, chrome, edge, brave, opera, vivaldi, chromium), cookies_file: Option<PathBuf> (the browser wins), proxy, rate_limit, retries (0 = yt-dlp default).

**Trim** — start, end: Option<f64>, precise: bool; section() builds "*start-end"; label() builds "0m05s-1m20s" style for filenames.

**DownloadRequest** — url, options, out_dir, trim, archive: Option<PathBuf>, network, format: Option<String> (exact -f selector), tags: Option<Tags> (only user-edited tags).

**MediaInfo** — title, uploader, duration, thumbnail, is_playlist, entries, items: Vec<PlaylistItem>, extractor, formats: Vec<FormatInfo>, subtitle_langs, has_auto_captions, chapters, track, artist, album.

**PlaylistItem** — url, title, uploader, duration, thumbnail, availability: Option<LinkStatus> (from a flat listing).

**FormatInfo** — id, height, fps, vcodec, acodec, tbr, size; has_video(), has_audio(), estimated_size(), quality_label().

**HeightChoice** — height, label, size: Option<u64>.

**StreamInfo** — url, headers (passed to ffmpeg with -headers), title, artist, album, duration, thumbnail.

### 4.6 Convert model

**Category** — Image | Video | Audio, with label(), targets(), default_target().

| Category | Targets |
| --- | --- |
| Image | png, jpg, webp, avif, gif, bmp, tiff, ico |
| Video | mp4, mkv, webm, mov, avi, gif, mp3, m4a, flac, wav, ogg |
| Audio | mp3, m4a, flac, wav, ogg, opus |

classify(path) maps extensions; gif counts as Image (animated gifs convert as images); an unlisted extension returns None and the file is skipped.

**ConvertOptions** (defaults): crf 23, preset "medium", codec H264, hardware CPU, max_height 0, audio_bitrate 192, image_quality 85, resize 0x0, gif (15 fps, 480 px, 256 colours, sierra2_4a, not diff-only, loop forever, 1x, forward, start 0, length 0), target_size_mb 0 (quality mode), filters.

**Filters** — rotate (None / Right / Left / Half), flip, crop (None / Wide 16:9 / Tall 9:16 / Square / Portrait 4:5), speed (default 1.0), normalize (EBU R128 to -16 LUFS), gain_db.

**ConvertRequest** — input, category, target, options, out_dir (None = next to the input), burn_subs: Option<SubSource> (Embedded | File(PathBuf)), output_stem, recycle_source.

**ConcatRequest** — inputs, target, options, out_dir, output_stem, recycle_sources.

**VideoCodec** — H264, H265, Av1, Vp9; **Hardware** — Cpu, Nvidia, Intel, Amd. Encoder names: libx264, libx265, libsvtav1, libvpx-vp9, and *_nvenc / *_qsv / *_amf (VP9 is CPU-only).

### 4.7 Planning types

**LinkChoices** — info, pick_video, pick_audio, tags, tags_prefill; picked_format(mode) composes "137+140", "137+ba" or "137"; plan(url, trim, settings) produces a PlannedDownload.

**Schedule** — later, hour, minute; next() = the next local occurrence; not_before().

**PlannedDownload / PlannedJob** — title, subtitle, spec, used directly by the queue.

### 4.8 Settings

**Settings** fields and defaults: download_dir (Downloads/Redline), convert_dir (None = beside the source), ytdlp_path / ffmpeg_path / ffprobe_path / deno_path (None = locate), concurrency 2, skip_downloaded true, network (defaults), download (defaults), convert (defaults), rename_template "{name}", recycle_originals false, download_presets (4 built-ins), convert_presets (4 built-ins), notify true, player (defaults), refresh_saved_on_start true, local_folders (desktop: the user's Music dir; Android: empty), local_files (empty), auto_replace_dead false.

**AppDirs** — data + downloads; set once per process with settings::set_app_dirs (Android). Setting the same dirs again is fine; different dirs error. Desktop never sets them: data = %APPDATA%/Redline, downloads = ~/Downloads/Redline.

**format** helpers — fmt_duration (1:02:05), fmt_time (keeps tenths), parse_duration ("90", "1:30", "1:02:03", "12.5"), fmt_bytes, fmt_timestamp, ago, starts_at, ARROW_RIGHT glyph.

---

## 5. Persistence

Everything is plain JSON written non-atomically (write to path, ignore errors). Missing or unreadable files fall back to defaults; Library additionally remembers that its file was unreadable so nothing is dropped. Files live in settings::data_dir() (Android: Context.filesDir).

| File | Shape | Writer / policy |
| --- | --- | --- |
| settings.json | Settings (pretty JSON) | App logic, when the struct differs from the copy read at startup |
| queue.json | Job[] (compact) | JobManager save_if_dirty; status/progress included; mid-run jobs become Queued on load |
| history.json | HistoryEntry[] (compact) | History save_if_dirty; Done jobs only; cap 2000 |
| playlists.json | Playlist[] (compact) | Library save_if_dirty |
| saved.json | { collections, songs } | Saved save_if_dirty |
| linkcheck.json | canonical key to {status, at, deep} | saved periodically (every 5 s) while checks run |
| local.json | Cached[] {modified, track, covers} | rewritten on every local scan |
| archive.txt | yt-dlp download archive | yt-dlp appends |
| covers/<hash>.jpg | extracted cover art | library::cover_for; removed by forget_cover |
| bin/ | yt-dlp, ffmpeg, ffprobe, deno (desktop); links to libffmpeg.so and libffprobe.so (Android) | install / android::init |

A failed write is ignored (no retry, no error surfaced). There is no schema version field anywhere; serde defaults provide additive migration (persisted structs use #[serde(default)] where fields were added later).

---

## 6. Feature rules

### 6.1 Download

1. **Lookup** — ytdlp::fetch_info runs yt-dlp with -J --flat-playlist -- <url>, through base_command (UTF-8, no warnings, --ignore-config, --ffmpeg-location, --js-runtimes name:path, network args). The result becomes MediaInfo; the lookup also records playlist-item availability into the link cache.
2. **Defaults from the lookup** — a playlist link sets settings.download.playlist = true; a single video forces playlist = false in the job regardless of the setting; tags are prefilled from track/artist/uploader.
3. **Format choice** — the user may pick a max height or exact formats from the lookup. Video: -f "bv*+ba/b" with -S "res:H,vcodec:h264,acodec:m4a" (mp4) or VP9/OPUS (webm); an exact pick becomes "137+140", "137+ba" or "137". Audio: -f "ba/b", then -x --audio-format <fmt>, with --audio-quality unless flac/wav.
4. **Archive** — settings.skip_downloaded adds --download-archive archive.txt. Trimmed clips never use the archive (a clip of something downloaded before is still wanted). "Download anyway" (retry_ignoring_archive) clears it.
5. **Trim** — --download-sections "*start-end" (or "*start-inf" when there is no end) plus --force-keyframes-at-cuts when precise. The output template gets the trim label so clips do not overwrite each other. Trimming is disabled for playlists.
6. **Subtitles** — embed (video only) or write .srt beside the file (always for audio); --sub-langs from the setting or the default list; optional auto captions; --convert-subs srt for files.
7. **Post-processing** — --embed-thumbnail (not for webm/wav), optional square crop via --convert-thumbnails jpg and --ppa crop, --embed-metadata, --split-chapters (single videos, no trim, not playlists) with a second -o chapter template, optional SponsorBlock removal (sponsor, selfpromo, interaction).
8. **Output naming** — playlist: "<playlist>/<NNN> - <title>.<ext>"; trim: "<title> [<id>] <label>.<ext>"; else "<title> [<id>].<ext>", all under -P <out_dir>.
9. **Progress protocol** — yt-dlp runs with --newline --progress and custom --print templates: RL|percent|speed|eta, RLPP|postprocessor, RLTITLE|title, RLFILE|path. parse_line turns those into JobStatus updates; postprocessor names are humanised ("Merging video and audio…", "Extracting audio…", and so on). Playlist runs prefix the detail with "Item N".
10. **Completion** — the last reported file is the output; a playlist with more than one item reports its folder. If nothing was written and the archive was in play, the job is Skipped("Already downloaded"). User-edited tags (audio, single video) are written afterwards with ffmpeg -c copy (title/artist/album; mp3 gets -id3v2_version 3), keeping cover art.
11. **Side effects** — the job is pushed with an optional not_before ("Start later"); when lookup info exists the link is also saved quietly to Saved under its platform, so downloads appear in the library.

### 6.2 Convert and join

1. **Intake** — dropped or picked files are classified by extension; unsupported ones are skipped and listed. Duplicate paths are ignored. Audio files added here are also recorded in settings.local_files so Local lists them.
2. **Sections** — the options shown depend on the list: video (video in, video out), audio (audio in, or video to an audio target), gif (video to GIF), image. Codec choice only applies where a video container exists; VP9 appears only if some file becomes WebM; a codec the container cannot hold is fitted (H.264/H.265 become VP9 for all-WebM lists) and noted under the picker.
3. **Per-file settings** — each item may carry its own ConvertOptions and burn-in subtitles; otherwise it follows the shared settings. Presets only touch files that follow the shared options.
4. **Execution** — images use image_conv (the image crate, AVIF included); video and audio build an ffmpeg command (filters: rotate, flip, crop, speed, loudnorm, gain; subtitle burn-in). Output goes to convert_dir or beside the source; names come from render_stem(template, name, n, target), sanitised for Windows-illegal characters; unique_path appends " (n)" when taken.
5. **Size targeting** — target_size_mb > 0 (video only, known duration) computes a video bitrate and runs two passes when the target supports it; the progress bar splits between passes; temp logs are removed afterwards.
6. **Join** — allowed for two or more files of the same non-image category with a target that is audio or a video container. Uses the first file's options, names the output "<stem> (joined)", normalises geometry and frame rate to the first input, and generates silence for inputs without audio.
7. **Cleanup** — output is deleted if conversion fails; on success the original is moved to the Recycle Bin (desktop only) when recycle_originals is on and the output is non-empty. Android never deletes the original.
8. **Cancellation** — kills the ffmpeg process tree and reports Cancelled; no partial output remains.

### 6.3 Queue, scheduling and after-queue

* **Concurrency** — pump starts queued jobs until running equals concurrency.max(1). Jobs run in their own threads; the worker reports through Reporter (progress / processing / title) and a Finished event.
* **Scheduling** — a job with not_before waits; pump returns the delay to the earliest scheduled job (clamped 1-60 s) so the app can repaint then. Scheduled jobs count as active (keep-awake, badge, notification deferral).
* **Status rules** — retry only affects finished jobs and resets the cancel handle; remove keeps active jobs; clear_finished removes only finished ones; cancel of a queued job is immediate, of a running one kills the tree; cancel_all runs on App::drop so no yt-dlp or ffmpeg outlives the window.
* **Persistence** — the list round-trips; Running/Processing jobs are restored as Queued, and next_id resumes at max(id)+1.
* **Announcements** — cancelled jobs are ignored; a single job announces "Finished", "Failed" or the skip reason with its title; several announce "N jobs ended" plus the failure count; "· N still in the queue" is appended when work remains. Desktop only notifies when the window is not focused (settings.notify gates the OS notification; the attention request always happens).
* **After queue** — Nothing / OpenFolder / Sleep / ShutDown, deliberately not persisted. Fires once, only after at least one successful (Done/Skipped) job, when the queue empties; OpenFolder uses the last output folder or the download dir. Shutdown uses "shutdown /s /t 60" with a cancel window ("shutdown /a").
* **Keep awake** — SetThreadExecutionState(ES_SYSTEM_REQUIRED) while any job is active; released when the queue empties.

### 6.4 History

Finished jobs only (Done), newest first, capped at 2000. Rows expose the URL, whether the output still exists, and search over title, subtitle, URL and input paths. "Download again / Convert again" re-pushes the spec with the archive removed; removing and clearing are immediate. A URL is found through platform::canonical, so youtu.be/ID finds a download of watch?v=ID&t=5.

### 6.5 Saved (platform collections)

* **Model** — collections are whole platform playlists/sets/albums, kept in the platform's order; songs are single links saved on their own. Identity is the canonical link. Platform is derived from the URL every time, with yt-dlp's extractor only as a fallback.
* **Saving** — save_link(info, url): a playlist creates or refreshes the collection (matched by source_url); a single link adds a song unless its key already exists. The message comes from SaveOutcome.
* **Sync (one-way, pull only)** — sync_plan(collection, fresh) matches by key and produces the new list in platform order: existing entries keep their added_at; new ones are inserted at their platform position; entries missing upstream are kept and flagged removed_upstream (never deleted); returning tracks clear the flag. An empty fresh list is an error, not "everything was removed". apply_sync stores the order and synced_at. sync() re-lists with fetch_info + tracks_from_info; errors go through friendly_error and land in sync_error.
* **Refresh policy** — manual per collection and per platform; optionally on launch for collections older than 12 hours (settings.refresh_saved_on_start, on by default), one at a time in the background; no timer.
* **Playback** — a collection plays its non-removed tracks. Playback of any link prefers a downloaded copy found in History (playable_source), matched canonically.
* **Keep offline** — offline_jobs() returns planned audio downloads for non-removed tracks with no finished download in History; not_queued() filters what is already queued. Turning the toggle on queues them; a sync of an offline collection queues the new ones too.
* **Export / import** — export_json() writes collections + songs; parse_import accepts that shape and reports what it found; merge() unions by key and reports added/skipped.
* **Maintenance** — rename, remove, clear_removed, replace_link (a replacement rewrites the stored track by key), copy to a user playlist.

### 6.6 Playlists and Now playing

* User playlists are local only, mixing links and files; create / rename / delete / add / remove / move / replace-link. Reordering is index-based; deletion needs a second confirming click in the UI.
* Starting playback replaces NowPlaying (optionally shuffled from a random index), remembers the playlist id, and loads the first track through library::playable_source.
* "Play next" uses insert_next; "Add to queue" appends; remove keeps the current track; shuffle toggles reshuffle; repeat cycles off/all/one.
* Dead links are dropped before playback (LinkCheck::drop_dead), with the start index clamped. A failed link can be skipped after repeated attempts (skip_failed, cap MAX_SKIPS = 8) and is recorded so its rows show it.
* Prev goes back to the previous track, or restarts the current one when more than 3 s in.

### 6.7 Local

* Sources: settings.local_folders (searched recursively) plus settings.local_files (single songs added on Convert). Desktop defaults to the user's Music folder; Android starts empty and uses the MediaStore index instead of ffprobe.
* Audio only (Category::Audio); folders are walked in case-insensitive name order; duplicates (a file inside a chosen folder that is also listed) are removed by path.
* Tag cache: local.json keyed by path and modified time; only new or changed files run ffprobe; probes run 4 at a time; entries cached before cover support are re-probed once.
* Covers: cover.jpg / folder.jpg / front.jpg / png beside the file wins; otherwise embedded art is extracted with ffmpeg to data/covers/<hash>.jpg (desktop only; Android reads art itself through Coil).
* Deleting songs: extracted covers are removed for paths that no longer exist; "Remove from Local" only drops the settings.local_files entry.

### 6.8 Link checking and replacement

* **What is checked** — links in playlists, Saved collections and saved songs, whenever a list comes on screen or changes length. The cache is global (linkcheck.json) and versioned for UI invalidation.
* **How** — YouTube: oEmbed GET (8 s timeout, proxy honoured). Everything else, or a YouTube link already known to fail playback: yt-dlp --simulate -J --no-playlist with a 30 s deadline and process-tree kill. Flat playlist listings also record statuses (deleted/private stand-in titles, availability field).
* **Meaning of replies** — 200 Alive; 400/404 Dead; 401/403 Restricted; anything else (timeouts, network, 5xx, bot checks, "error code" lines) Unknown = ask later; 429 or rate-limit wording = SlowDown.
* **Cache lifetime** — 7 days for quick YouTube checks, 14 days for yt-dlp/deep checks. A deep "will not play" result is sticky: a later quick "alive" does not overwrite it.
* **Throttling** — 3 workers; unanswered checks retry after 15 s doubling (5 tries, then rest 30 min); after 3 consecutive unhelpful answers the queue pauses 10 s, doubling to 5 min; a rate limit pauses immediately and spaces checks 250 ms, doubling to 4 s; 20 good answers halve the gap. Results are saved periodically (every 5 s) while checks run.
* **Playback integration** — PlayWatch watches the player: a failed link is recorded (deep, so oEmbed cannot undo it), the queue may skip it, and the UI shows a failure notice with Find replacement for 15 s.
* **Replacement search** — query "ytsearch5:artist title" (artist's " - Topic" / "VEVO" suffix stripped, artist omitted when the title already contains it). Candidates are scored 0.7 times Dice word similarity (noise words removed) plus 0.3 times length score (1 - |delta| / 30 s); the track's own link and its replaced_from are excluded. is_clear_match = title >= 0.6 and |delta| <= 3 s.
* **Fix unavailable** — searches each dead or restricted track in a list sequentially; clear matches are swapped automatically when auto_replace_dead is on, the rest go to a review dialog. A swap keeps title, artist and added_at and remembers the first link in replaced_from.

### 6.9 Player

**Engine lifecycle** — the engine thread is spawned eagerly; the audio device is opened lazily on the first load. Output loss (device unplugged) closes and retries the default device, then resumes. Dropping Player sends Shutdown and stops playback.

**Loading a source** — resolution goes through the Media trait: links via ytdlp::resolve_stream (URL + headers + metadata), local files via ffprobe (missing file and "no sound" are errors). Resolution runs on a short-lived worker thread and comes back to the engine as a message (Resolved, or Prefetched for the look-ahead); Loading is published immediately. Link look-ups are asked of a yt-dlp worker kept running for them, started at app launch (warm_stream_worker) and after tool or network changes, and the worker's reply is trimmed to the stream fields. Resolved links are cached in memory for one hour (LINK_TTL) and reused for replays and seeks.

**Decoding** — ffmpeg writes interleaved stereo f32 at the device rate to stdout; the reader thread copies into an rtrb ring buffer (2 s); the audio callback drains it. Underrun writes silence and sets buffering. A seek kills the stream and starts a fresh ffmpeg at the new offset with its own ring buffer; the switch happens at the next callback.

**Stream URL expiry** — ffmpeg failing with 403/410 before any audio triggers one re-resolve and a resume at the current position. Other pre-audio refusals are retried up to 8 times, 1 s apart.

**Disk cache** — while a link is current and its duration is known to be at most 30 minutes, a filler thread decodes it to a temp file (tools::temp_dir); playback streams read that file at their own position. A reader that catches the filler waits when close, or starts a new filler (replacing the old one). The file is deleted when the cache is dropped (next load).

**Prefetch** — the app hands SetNext the source after the current track. Its link is resolved and its stream URL warmed while the current track plays; failures surface as PlayerState::next_failed before its turn. Loading a track that is still being prefetched waits for that answer instead of running yt-dlp twice.

**Ending** — an ended stream reports Ended; when a next source was set, the engine advances by itself and increments advances; the app then moves NowPlaying along by the same number. Playback therefore keeps going even when the UI is not running (minimised).

**Speed** — 0.5x to 2.0x through a Catmull-Rom variable-rate resampler in the audio callback; pitch follows (turntable style, no time-stretch). The target speed is eased over about 30 ms to avoid clicks. Reported position and duration stay in source time.

**EQ** — 10 peaking biquads at 31, 62, 125, 250, 500, 1000, 2000, 4000, 8000 and 16000 Hz; plus/minus 12 dB per band, Q 1.41; preamp -12 to +6 dB; RBJ cookbook coefficients, only the changed band is recomputed; stereo state per channel; soft clipper last. Presets: Flat, Bass boost, Treble boost, Vocal, Loudness. Enabled/bypass flag plus preamp.

**A-B loop** — A and B markers on the current track; B must be at least 0.5 s after A; reaching B seeks to A. A new track clears both.

**Failure handling** — a link that fails resolution or decoding ends in Error(msg), worded by friendly_error; the UI keeps the failure for Find replacement.

**Media keys (Windows)** — souvlaki SMTC: Play/Pause/Toggle/Stop/SetPosition go directly to the player from the callback thread; Next/Previous/SeekBy(10 s)/Raise come back to the app through a channel. Metadata and playback state are pushed only on change (or more than 3 s of position drift); cover art only for http(s) thumbnails.

### 6.10 Presets

Built-ins seeded on first run (then stored in settings, editable and removable):

| Download presets | Convert presets |
| --- | --- |
| Podcast MP3 128k | Discord clip (10 MB): mp4, 720p, H.264, 128k |
| Music FLAC (square cover) | Web MP4 720p: CRF 24, preset slow |
| 720p MP4 | Small WebP: quality 80, resize 1920 |
| Archive MKV best (subtitles embedded) | Even-volume MP3: loudnorm, 192k |

A preset is "active" when its options match the current shared options (the hardware choice is ignored). Applying a preset sets the shared options and, for files that follow them, their targets; saving a preset remembers the targets when every file of a category agrees. UI bar: click to apply, right-click to rename or delete, + to save.

### 6.11 Tools

* **Locate order** — explicit path in Settings (must be a file), then bin/, then PATH. ffprobe prefers the one next to the chosen ffmpeg, then PATH, then bundled. Android's bundled tools fill any gaps; its yt-dlp is a Python script run through libpython.so, using the unpacked zipapp when available.
* **First-run install (desktop)** — yt-dlp latest release (per OS), ffmpeg and ffprobe from BtbN's win64 GPL zip (Windows; elsewhere the user must install ffmpeg), Deno latest (the JS runtime YouTube needs). Progress and step text are published; on success ToolPaths are relocated and the player and link checks are told.
* **yt-dlp update** — install::update_ytdlp runs yt-dlp -U (on Android it stages a copy of the zipapp, updates it, checks that it still runs, swaps it in and unpacks it) and reports the new version. Stale unpacked copies are removed the next time the app starts, before anything runs yt-dlp. Android updates at launch at most once a day.
* **Process hygiene** — tools::command suppresses console windows (CREATE_NO_WINDOW), applies Android's child environment, and puts children in their own process group on unix. kill_tree uses taskkill /T /F, or killpg(SIGKILL) on unix.
* **Streaming runner** — run_streaming feeds stdout lines to a callback while keeping the last 40 stderr lines; cancellation wins over exit status; failures report the last "ERROR:" line (or the last three lines).
* **Streaming warm-up** — a yt-dlp worker is started at app launch (desktop App::new, Android Engine::new) and after a yt-dlp update or a network-settings change, so the first song skips Python, yt-dlp and extractor startup. Three failed starts in a row pause starts for 60 s instead of disabling the worker for the session; an update retires idle workers. The worker's plugin also keeps YouTube's preprocessed player for QuickJS in memory and on disk (32 MB, 14 days), so the JS-challenge preprocessing is paid once per player rather than per song or per launch.

### 6.12 OS integration

| Feature | Desktop | Android |
| --- | --- | --- |
| Notifications | notify-rust, only when unfocused and settings.notify | NotificationCompat channels progress / finished; only when not in the foreground |
| Keep awake | SetThreadExecutionState | partial wake locks in the two services |
| Sleep | SetSuspendState (Windows) | not supported |
| Shutdown | "shutdown /s /t 60" with "/a" cancel | reports Unsupported |
| Reveal / open | explorer /select, or open::that_detached | FileProvider content:// intents |
| Media keys | SMTC via souvlaki | MediaSessionCompat in PlaybackService |

Reveal is a no-op on Android, where files are opened through intents.

---

## 7. Desktop UI (egui)

### 7.1 Window and shell

* Window title "Redline", 1200x780, minimum 900x600, file drag and drop enabled. Windows subsystem attribute hides the console in release builds.
* Layout: black shell (#000); left sidebar 260 px (logo, nav, tools card pinned to the bottom); bottom now-bar 84 px; central panel with the page (rounded #121212 area, header gradient from the accent).
* Central pages scroll; content has 32 px side padding. Sidebar scrolls when the Saved platform list grows, keeping the tools card visible.
* Accent is read at paint time. Saved platform pages swap in that platform's accent (header gradient, pills, sliders, toggles, glyphs); the sidebar logo, player bar and everything else stay Redline red.

### 7.2 Theme tokens

| Token | Value | Use |
| --- | --- | --- |
| BLACK | #000000 | window, sidebar, player bar |
| BG | #121212 | page background |
| CARD / CARD_HOVER | #181818 / #282828 | cards and rows |
| INPUT | #242424 | text fields, chips |
| ELEVATED | #282828 | popups, toasts, notices |
| TEXT / MUTED / SUBTLE | #FFFFFF / #B3B3B3 / #6A6A6A | text hierarchy |
| TRACK | #4D4D4D | slider track |
| Accent REDLINE | base #E8263A, hover #FF3B4E, pressed #C41A2D, header tint #6E121D | red accent |

Platform accents: YouTube #FF0033, SoundCloud #FF5500, Bandcamp #1DA0C3, Mixcloud #7B5CFF, Vimeo #1AB7EA, Other #9A9A9A. Hover is 15% toward white, pressed 18% toward black, header tint 45% of the base mixed into BG; pill fills darken until white text reaches 3:1 contrast.

Fonts: Inter Regular/Bold from assets, registered under the bold family; egui-phosphor regular supplies icons. The core's ARROW_RIGHT glyph is asserted equal to the icon font's arrow.

Shared widgets in theme.rs: heading, strong, muted, eyebrow, card, page_header, header gradient, pill/small-pill/ghost/icon buttons, chips, toggle, toggle_icon, progress_line, range_bar, nav_item, nav_sub_item, art_tile, option_row, seek_bar, play_button, line_slider, cover.

### 7.3 Pages

| Page | Contents |
| --- | --- |
| Download | URL bar with Look up (Enter works); lookup card (title, uploader, duration, chapters, formats, previous download time); Play / Add-to-playlist; Format card (video/audio mode, height or exact formats, container/audio format/bitrate, embed cover art, square cover, metadata, subtitles, download-whole-playlist, auto captions); Edit card (trim with range bar and time fields, Fast/Precise cuts, SponsorBlock removal, split chapters); Tags card (audio, single video only); Save-to row; schedule row; Download and Save buttons; quiet save into Saved after queueing |
| Convert | Tools banner when ffmpeg is missing; drop zone and file picker; presets bar; shared options (codec, encoder, CRF, preset, height, audio bitrate, image quality, resize, GIF options, target size, filters); file list with per-file rows, target picker, per-file editor, remove/reorder; Join when applicable; Convert queues one job per file or one join job; skipped-file notice |
| Saved (overview) | "Save a link" box, per-platform cards (glyph, collection/song counts, last sync), export/import, empty state |
| Saved (platform) | Tinted header; tabs Collections / Songs / All songs; search; collection cards; collection page with Play, Shuffle, Refresh (spinner + sync error), Keep offline, Copy to playlist, Open on platform, Rename, Remove; removed-upstream tags; Fix unavailable bar and replacement dialogs |
| Playlists | Playlist list and "New playlist"; Now playing view; playlist view with header (cover mosaic, counts, total time, Play, Shuffle), link box to add a link or playlist, add-files, search, track table, rename/delete with confirm |
| Local | Folder chips (add/remove, reset to Music), added-files chip, Play / Shuffle / Rescan, search, track table with Convert, add-to-playlist and remove-from-Local |
| Queue | Header summary (in progress, scheduled, finished); Clear finished / Cancel all; after-queue chips; newest-first job rows with status column, cancel/retry/retry-anyway/remove/reveal/play/add-to-playlist; empty state |
| History | Search, Clear history; rows with timestamp, actions (download/convert again, open, reveal, save, play, add to playlist, remove); "file moved or deleted" annotation |
| Settings | Tools card (yt-dlp/ffmpeg/ffprobe/Deno status and path override, download missing tools, update yt-dlp, open tools folder); Folders; Performance (concurrency 1-6, notifications); Downloading toggles (skip downloaded, refresh saved on startup, auto-replace); Network and sign-in (cookies browser/file, proxy, speed limit, retries); version line |

### 7.4 Player bar and overlays

* With a track loaded: cover/title/artist (errors and buffering replace the artist line), bookmark toggle for the current link track (filled when it is saved); centre: shuffle, previous, play/pause, next, repeat, seek bar with A/B markers, elapsed and total; right: volume slider and mute, EQ button, speed button (double-click resets to 1x), A/B/Clear buttons, jobs indicator that opens the Queue.
* With nothing loaded: the bar shows the current job (tile, title, detail, progress) or "Nothing queued".
* Speed and EQ popups open above the bar; EQ has vertical band sliders, preamp, presets and bypass.
* Toast: bottom-centre, 4 s. Failure notice: bottom-left for 15 s with "Find replacement" and Dismiss.
* Dropping files on the window: over the Playlists page with a playlist open they are added to that playlist; otherwise they go to Convert (audio files are also remembered in Local).

---

## 8. Android app

### 8.1 Process model

* One Application (RedlineApp) owns one RedlineState, which owns exactly one Rust Engine and one Music handle. Activity restarts and the services share them, so the queue and playback survive rotation and backgrounding.
* MainActivity is the only activity; shared links and files arrive as intents (ACTION_SEND / SEND_MULTIPLE) and are routed to Download or Convert.
* Two foreground services: DownloadService (dataSync) keeps the process alive while jobs run; PlaybackService (mediaPlayback) owns the media session, notification, audio focus, becoming-noisy receiver and a wake lock while playing.
* Navigation state (tab, Library section, player open) lives in NavModel, saved through SavedStateHandle. Settings is not in the bottom bar; it is reached from each page's header and Back returns to the page it was opened from.

### 8.2 Startup sequence

1. RedlineApp.onCreate: create notification channels, construct RedlineState, register the foreground tracker, call state.start().
2. state.start (off the main thread): YoutubeDL.init + FFmpeg.init, then RedlineEngine.paths() and Engine(...) with an EngineListener; build convert and download choice lists; engine.music(MusicListener) creates the Music handle (loads playlists, starts the player thread); MusicState is constructed.
3. Status becomes Ready; the Kotlin side starts the refresh loop, reads settings once, detects encoders, refreshes tool status, and updates yt-dlp at most once a day (SharedPreferences "tools" / ytdlpCheckedAt).
4. Any failure (including a missing libredline_mobile.so) becomes a Failed status with the message on screen.

### 8.3 UniFFI surface (package app.redline.core)

**Engine** (blocking calls): new(paths, listener), tools_status, update_ytdlp, fetch_info, queue_download, queue_snapshot, active_count, cancel, cancel_all, retry, retry_ignoring_archive, remove, clear_finished, drain_finished, history, remove_history, clear_history, download_again, detect_encoders, convert_files, queue_conversions, queue_join, sweep_convert_inputs, settings, set_settings, music.

**Music** (blocking calls): snapshot, queue, play, pause, toggle, seek, set_loop_a, set_loop_b, clear_loop, next, prev, stop, jump, set_shuffle, set_repeat, remove_from_queue, player_settings, set_volume, set_speed, set_reset_speed, set_eq, play_playlist, play_tracks, queue_tracks, play_next, play_file, play_history, playlists, playlist, create_playlist, rename_playlist, delete_playlist, add_tracks, remove_track, move_track, replace_playlist_track, replace_failed_track, fix_unavailable_playlist, check_links, link_check_version, find_replacements, sweep_imported, tracks_for_link, scan_files, local_file, track_for_history, download_for_offline, plus the Saved methods below.

**Saved methods on Music**: saved_version, saved_overview, saved_platform, saved_collection, saved_all_songs, is_saved, save_link, save_track, toggle_saved, remove_saved_song, export_saved, import_saved, refresh_collection, refresh_platform, rename_collection, remove_collection, clear_removed, set_keep_offline, copy_collection_to_playlist, replace_saved_track, fix_unavailable_collection, play_collection, play_saved_songs.

**Listeners**: EngineListener.on_change() and MusicListener.on_change(); both are called on Redline's own thread at most about 10x/s and must not block (the app hops to the main thread).

**Records crossing the bridge**: AndroidPathsRecord, ToolsStatusRecord, DownloadOptionsRecord, DownloadChoicesRecord, ChoiceRecord, QualityChoiceRecord, TagsRecord, TrimRecord, MediaInfoRecord, FormatRecord, HeightChoiceRecord, PlaylistItemRecord, JobRecord, JobState, FinishedRecord, FinishedBatchRecord, AnnouncementRecord, HistoryRecord, NetworkRecord, SettingsRecord, TrackRecord, TrackKind, LinkStatusRecord, ReplacementRecord, ReviewRecord, FixReportRecord, PlaylistSummaryRecord, PlaylistRecord, PlayerSnapshotRecord, PlayerStatus, PlayFailureRecord, QueueTrackRecord, EqRecord, PlayerSettingsRecord, EqPresetRecord, PlayerChoicesRecord, ConvertOptionsRecord, ConvertItemRecord, ConvertFilesRecord, ConvertSectionsRecord, ConvertPresetRecord, PresetAppliedRecord, ConvertSettingsRecord, EncodersRecord, AndroidPathsRecord, Platform, RefreshState, SavedPlatformRecord, SavedOverviewRecord, CollectionSummaryRecord, SavedTrackRecord, CollectionRecord, SavedPlatformPageRecord, SavedImportRecord, SaveOutcomeRecord, SaveKind.

**Errors**: one flat RedlineError enum (ToolsMissing / Invalid / Failed); each variant arrives in Kotlin as an exception whose message is ready to show. Failures from anyhow are formatted with the context chain ("{e:#}").

Free functions in the crate: download_choice_lists, convert_choice_lists, convert_files, convert_sections, apply_convert_preset, new_convert_preset, convert_preset_matches, rename_example, convert_join_target, classify_file_name, convert_item_makes_video, encoder_label, codec_label, default_convert_options, player_choices, convert_settings_record, apply_convert_settings, stale_input_dirs, fmt_bytes, fmt_duration, fmt_time, parse_duration, trim_problem.

### 8.4 Screens

| Screen | Contents |
| --- | --- |
| Download | URL bar with paste-from-clipboard, lookup, media card, ScheduleRow, FormatCard (mode, height or exact formats, embed cover art, square cover, metadata, subtitles + auto captions, download whole playlist), EditCard (trim + Fast/Precise cuts, SponsorBlock, split chapters), TagsCard, save-to-Saved, download button |
| Convert | Add files (SAF picker + shared files), file rows with target picker and per-file editor, options card, presets row with rename/delete/save dialogs, join, convert button |
| Library / Saved | Overview with platform cards, save box, export/import, platform page, all songs, collection page (play/shuffle, refresh, keep offline, copy to playlist, rename, remove, fix unavailable, replacement dialog) |
| Library / Playlists | Playlist list, detail with cover, Play/Shuffle, add bar, track rows with play/next/queue/offline/save/replace/up/down/remove, "Add to playlist" dialog |
| Library / Local | Permission request, folder chips, MediaStore scan, search, play, add to playlist |
| Queue | Job list, status text and progress, cancel / retry / retry anyway / remove, error dialog, empty state |
| History | Search, play, download again, open output, remove, clear, save |
| Player | Mini player above the bottom bar (art, title, status, play/pause, next); full player (large art, title/artist, seek bar, A-B loop controls, transport, speed and EQ cards, Now playing list with jump/remove) |
| Settings | Tools (status, update yt-dlp, open folder), defaults, network (cookies.txt, proxy, rate limit, retries), storage |

### 8.5 Services

**DownloadService** — started whenever active jobs exist; startForeground within the deadline; ongoing low-importance progress notification (current job title/progress); collects the queue StateFlow and refreshes the notification; holds a partial wake lock for up to 6 h; stops itself when active count hits 0; onTimeout gives the dataSync slot back (Android 15 cap); START_NOT_STICKY (the queue is restored from disk on the next launch).

**PlaybackService** — runs while playing, or paused with a queue behind it; owns a MediaSessionCompat (lock screen, headset, Bluetooth, system controls) and a MediaStyle notification; maps play/pause/next/prev/seek/stop back to MusicState; requests audio focus on the transition to playing, pauses on transient loss and resumes on gain when it was playing, pauses on becoming noisy; holds a partial wake lock while playing (1 h backstop); stops when nothing is loaded.

### 8.6 Storage, permissions, file handling

* data = context.filesDir; downloads = externalFilesDir(DOWNLOADS)/Redline (no permission needed); converted files = externalFilesDir(null)/Converted; imported songs = externalFilesDir(MUSIC) or filesDir/Music; convert input copies = cacheDir/convert-inputs.
* Permissions: INTERNET, FOREGROUND_SERVICE, FOREGROUND_SERVICE_DATA_SYNC, FOREGROUND_SERVICE_MEDIA_PLAYBACK, POST_NOTIFICATIONS (requested once), WAKE_LOCK, READ_MEDIA_AUDIO (33+) / READ_EXTERNAL_STORAGE (32-).
* content:// input cannot be read by ffmpeg, so picked or shared files are copied into app storage first: ConvertInputs (one folder per file, swept when no queued/failed job needs it) and ImportedMusic (playlist copies, held until their tracks are in place, swept when no playlist, queue entry or history entry uses them).
* FileProvider (authority <applicationId>.files) exposes downloads/conversions for "open with" and play intents; file_paths.xml covers external-files and internal Redline/Converted paths.
* youtubedl-android supplies libpython.so, libffmpeg.so, libffprobe.so and libqjs.so; Android init symlinks ffmpeg/ffprobe into bin/, hands over LD_LIBRARY_PATH, PYTHONHOME, HOME, SSL_CERT_FILE, PATH and TMPDIR, and unpacks the yt-dlp zipapp in the background.

### 8.7 Android-specific rules

* No browser-cookie support: network settings expose cookies.txt, proxy, rate limit and retries only.
* Volume is set to 1 on startup (the hardware buttons own volume; there is no slider to undo an old low value).
* No Recycle Bin: recycle_originals is a no-op.
* Local scans use MediaStore (tags come from the index) rather than ffprobe; covers are loaded by Coil, including embedded art through AudioCoverFetcher.
* Images/video/audio shared from other apps open Convert; a shared text link prefills Download.
* Deep links from notifications: EXTRA_OPEN_QUEUE (finished downloads) and EXTRA_OPEN_PLAYER (media notification).
* When a list is opened with auto-replace on, clear matches are fixed once per session (autoFixed set), so reopening does not re-search.

---

## 9. Cross-platform differences

| Concern | Desktop | Android |
| --- | --- | --- |
| Folder selection | rfd file dialogs, arbitrary paths | app-scoped dirs, SAF picker, MediaStore |
| Tool provisioning | first-run download into %APPDATA%/Redline/bin | shipped in the APK via youtubedl-android |
| JS runtime | Deno | QuickJS (libqjs.so) |
| Player output | cpal (default device, hot re-open) | AAudio via ndk |
| System media UI | SMTC (media keys + overlay) | MediaSessionCompat + MediaStyle notification |
| Background work | window may be minimised; threads keep running | foreground services + wake locks |
| Notifications | notify-rust when unfocused | NotificationCompat when not foreground |
| Saved settings | full Settings struct | SettingsRecord (subset: download dir, concurrency, skip/notify, download, network, convert, refresh, auto-replace) |
| Deletion | Recycle Bin (trash crate) | originals are kept |
| Import/export | file of saved links | same JSON via text share/paste |

---

## 10. Build, run, release

### 10.1 Windows

~~~powershell
cargo run --release            # build and launch
cargo test --workspace         # unit tests
cargo test --workspace -- --ignored --nocapture   # end-to-end (network, about 200 MB of tools)
~~~

Requirements: a recent stable Rust (edition 2024). yt-dlp, ffmpeg/ffprobe and Deno are fetched by the app into %APPDATA%/Redline/bin on first run. Note the documented pin: egui 0.36 with egui-phosphor 0.14 (do not bump one alone).

Profiles: release = opt-level 3 + strip; dependencies in dev builds use opt-level 2 (rav1e and friends); android-dev inherits dev with debug = false to keep libredline_mobile.so small.

### 10.2 Android

From android/:

~~~text
gradlew assembleDebug      -> android/apk/redline-debug.apk
gradlew assembleRelease    -> android/apk/redline-<versionName>.apk (signed when a key exists)
~~~

Requirements: JDK 17, Android SDK (compileSdk 37, minSdk 26, targetSdk 37), NDK found through ANDROID_NDK_HOME / ANDROID_NDK_ROOT or the SDK's ndk/ folder, cargo-ndk, and the Rust targets for x86_64 and arm64-v8a. ABIs built: x86_64 (emulator) and arm64-v8a (phones). Gradle runs two custom tasks per variant:

* uniffiBindings<Variant>: cargo build -p redline-mobile --features cli on the host, then uniffi-bindgen generate --library ... --language kotlin --no-format into a generated source dir.
* cargoNdk<Variant>: cargo ndk -t x86_64 -t arm64-v8a -P 26 -o <jniLibs> build -p redline-mobile (--release for release variants, --profile android-dev for debug). -Predline.skipRust=true skips it and yields an APK without the core.

Versions: versionCode 4, versionName 0.1.3, applicationId app.redline. packaging.jniLibs.useLegacyPackaging = true (the tools must be extracted to disk to run), matching extractNativeLibs and requestLegacyExternalStorage in the manifest.

### 10.3 Release script

scripts/release.ps1:

1. Reads the version from the [package] section of Cargo.toml and refuses to continue unless it equals versionName in android/app/build.gradle.kts.
2. Refuses to build without signing properties: ~/.redline-signing/keystore.properties (storeFile, storePassword, keyAlias, keyPassword) or the file named by REDLINE_SIGNING.
3. Fills in ANDROID_HOME, JAVA_HOME (Android Studio's jbr), and the newest NDK if unset; sets JAVA_TOOL_OPTIONS to a short socket directory to work around a Windows Gradle loopback failure.
4. Builds assembleRelease and cargo build --release, then copies to dist/Redline-<version>-android.apk and dist/Redline-<version>-windows-x64.exe.
5. With -Publish, runs gh release create v<version> with both files and generated notes.

Losing the signing key means users must uninstall (and lose app data) before installing an update.

---

## 11. Testing

**Totals:** 196 Rust tests on Windows (198 counting two unix-only tests), 17 of them `#[ignore]` end-to-end tests; 40 files contain tests. No Kotlin tests were found; Android is verified by building and running the app.

| Area | Tests | Notable coverage |
| --- | --- | --- |
| core: library | 11 (+1 ignored) | playlists, NowPlaying shuffle/repeat/insert/remove, tracks from MediaInfo |
| core: saved | 21 (+1 ignored) | serde round-trip, sync_plan add/remove/restore/reorder, platforms, all_songs dedupe, offline jobs |
| core: linkcheck | 15 | status mapping, throttling/retries, skip logic, ranking, clear match |
| core: player | 20 (engine) + 8 dsp + 4 cache + 4 decode + 2 render + 2 ab_loop | state transitions, seeks, capture-sink playback, EQ magnitude, resampler length/pitch, speed-change continuity |
| core: ytdlp | 19 | args for video/audio/subs/trim/archive/tags, progress parsing, error wording |
| core: convert, ffmpeg, filters, hwaccel, image | 5 + 5 + 3 + 2 + 1 | target choices, progress parsing, filter chains, encoder resolution |
| core: tools plumbing (mod, install, ytdlp_worker) | 3 + 2 + 3 | unique paths, tool lookup, kill tree, install, worker protocol, failure back-off |
| core: jobs, history, queue_watch, settings, format, platform, presets, local, download_plan, convert_plan, android | 2 / 1 / 2 / 1 / 1 / 2 / 1 / 2 / 1 / 2 / 2 | scheduling, resume, persistence, URL canonicalisation, planning |
| bridge (redline-mobile) | 6 + 15 + 2 + 6 + 2 + 1 | records mapping, engine step, music queue/transport, convert helpers |
| UI | 2 + 1 + 1 | theme contrast/accent, arrow glyph equality, speed labels |
| core: end-to-end (tools/e2e.rs, player/e2e.rs) | 11 + 4 | real yt-dlp/ffmpeg: download, convert, join, install, play, seek, stream from YouTube, warm worker reuse |

Area rows sum to 198, counting the two tests that only compile on unix; a Windows run executes 196. The 17 ignored tests are the 11 + 4 end-to-end ones plus one in library and one in saved (counted inside those rows). End-to-end tests are opt-in (cargo test --workspace -- --ignored --nocapture); they hit the network and download tools.

---

## 12. Conventions and invariants

1. **Core is UI-free.** redline-core has no egui/eframe dependency. UI text that lives in core (job titles/subtitles, messages) is English and not localised.
2. **One bridge from core to UI: snapshots + Wake.** Background work never touches UI state directly.
3. **Identity of a song** is platform::canonical(link), or the file path. Use Track::key() rather than comparing URLs.
4. **Dirty-flag persistence.** Library, History, JobManager and Saved mutate a dirty flag and write only in save_if_dirty, called once per frame / engine step. New persisted stores should follow the same pattern.
5. **Doc comments explain why, not what.** Match the surrounding style: short module docs, comments for non-obvious decisions.
6. **Errors:** anyhow in core (with .context), user-facing wording through friendly_error; the bridge flattens to RedlineError with the full context chain.
7. **UI style:** all colours, spacing and widgets come from theme:: helpers; nothing hard-codes a colour outside theme.rs except deliberate one-offs (for example the amber link warning).
8. **Platform cfg gates** live in core (settings dirs, cpal vs AAudio, trash vs no-op, SMTC, install). The UI is desktop-only; Android consumes the bridge.
9. **No blocking on the UI thread.** Long work goes through Task, the job queue, the player thread or the link-check workers; quick UI-thread file checks are tolerated (for example Path::is_file).
10. **Cancellation is cooperative.** CancelHandle is attached to every external process; run_streaming checks it before and after; killing a tree is platform-specific.
11. **Bounded concurrency:** jobs = settings.concurrency; link checks = 3; local probes = 4; the launch refresh of saved collections runs one collection at a time (a manual refresh is per collection); the audio callback does no allocation, locking or I/O.
12. **Keep new UI text in the UI layer** where practical; core messages should read like user-facing English and avoid leaking tool syntax (friendly_error rewrites raw yt-dlp errors).

---

## 13. Known gaps, risks and open questions

**Gaps (verified by inspection)**

* No CI, no LICENSE, no Kotlin test suite, no schema versioning or atomic writes for JSON state.
* Android has no browser-cookie support, no Recycle Bin, and no shutdown/sleep action; shutdown reports Unsupported.
* Desktop Linux/macOS builds without SMTC and expect the user to install ffmpeg themselves (install.rs only knows how to fetch it on Windows).
* ffmpeg is required for almost everything; images are the only conversion path that works without it.
* The codebase is English-only; localisation would require moving core-produced strings out of redline-core.
* .optim-jev/ and android/.gradle/ are local/untracked artefacts; dist/ carries 130 MB APKs.

**Risks**

* **yt-dlp / YouTube drift.** Stream URLs expire (cache TTL is 1 h by design), YouTube now needs a JS runtime, and bot checks are common. The friendly_error paths and configurable network settings exist because of this.
* **Link-check and replacement heuristics.** oEmbed cannot see geo-blocked or "not available" uploads; deep checks are sticky for 14 days. Ranking is heuristic; auto-replace only applies clear matches.
* **Thread/lifecycle complexity.** Two state owners (UI thread on desktop, engine thread on Android), prefetch races, cache filler replacement and device loss all have narrow timing assumptions; changes here need the player tests.
* **Non-atomic writes.** A crash during save can leave truncated JSON; loaders fall back to defaults (Library keeps an unreadable flag, others silently empty), which can look like data loss.
* **Version/tooling pins.** egui 0.36 with egui-phosphor 0.14; UniFFI 0.32.2 must match between the library and bindgen; the Android keystore is outside the repo.
* **APK size and store limits.** 130 MB because Python, ffmpeg/ffprobe and QuickJS ship per ABI; only x86_64 and arm64-v8a are built.
* **Android background limits.** Foreground-service start restrictions (Android 12+) and the 6 h dataSync cap are handled by logs and retries on the next foreground refresh, not by guarantees.

**Open questions worth asking before large changes**

* Should core-produced strings be localisable (would touch every PlannedJob and Job subtitle)?
* Should queue/history/saved writes become atomic (tmp file + rename)?
* Is Linux/macOS a supported target (would need an installer path and cpal device handling beyond Windows)?
* Should Saved sync become two-way or account-aware? The current design is deliberately one-way.

---

## Appendix A: constants worth knowing

| Constant | Value | Where |
| --- | --- | --- |
| Job concurrency | 1-6 in UI, default 2 | settings.rs / Settings page |
| Queue recheck clamp | 1-60 s | jobs.rs::pump |
| History cap | 2000 entries | history.rs |
| Saved stale threshold | 12 h (desktop + Android) | ui/saved.rs, mobile/saved.rs |
| Worker warm-up | at launch; 60 s pause after 3 failed starts | tools/ytdlp_worker.rs |
| Worker idle retirement | 5 min desktop, 15 min Android | tools/ytdlp_worker.rs |
| Preprocessed player cache | 32 MB, 14 days, memory + disk | tools/ytdlp_worker.py |
| Player wake while playing | 100 ms | player/mod.rs::WAKE_EVERY |
| Resolved link TTL | 1 h | player/mod.rs::LINK_TTL |
| Stream refusals | 8 tries, 1 s apart | player/mod.rs |
| Ring buffer | 2 s | player/render.rs::BUFFER_SECS |
| Link disk cache | 30 min max duration | player/cache.rs::MAX_SECS |
| Speed range | 0.5-2.0, eased 30 ms | dsp.rs |
| EQ | 10 bands, Q 1.41, 12 dB, preamp -12..+6 | dsp.rs |
| A-B minimum gap | 0.5 s | ab_loop.rs::MIN_LOOP |
| Link check workers | 3 | linkcheck.rs |
| Link check retries | 5, first after 15 s | linkcheck.rs |
| Check pause / max pause | 10 s doubling to 5 min | linkcheck.rs |
| Check gap / max gap | 250 ms to 4 s | linkcheck.rs |
| Check rest after no answers | 30 min | linkcheck.rs |
| Check cache life | 7 days quick, 14 days deep | linkcheck.rs::max_age |
| Max skips per run | 8 | linkcheck.rs::MAX_SKIPS |
| Replacement results | 5, clear match 0.6 + 3 s | linkcheck.rs |
| Local probe parallelism | 4 | local.rs |
| Prev restarts track after | 3 s | app.rs / mobile/music.rs |
| Media seek step | 10 s | media_keys.rs |
| Shutdown delay | 60 s | system.rs |
| Toast / failure notice | 4 s / 15 s | app.rs / now_bar.rs |
| Android engine nudge | 100 ms throttle, 30 s idle | mobile/engine.rs |
| Android wake locks | 6 h downloads, 1 h playback | work services |
| Window / sidebar / bar | 1200x780 (min 900x600), 260 px, 84 px | main.rs, app.rs |

---

## Appendix B: where to change what

| Task | Touch |
| --- | --- |
| Add a download option | tools/ytdlp.rs (DownloadOptions, default, build_args), mobile/records.rs (DownloadOptionsRecord + conversions), src/ui/download.rs (format card), Android DownloadScreen/FormatCard |
| Add a convert option | tools/convert.rs (ConvertOptions, defaults, ffmpeg args), tools/filters.rs if it is a filter, convert_plan.rs (sections), src/ui/convert.rs, mobile/convert.rs + ConvertOptions.kt |
| Add a page (desktop) | app.rs (Page + field + central match), ui/mod.rs, ui/sidebar.rs, new ui module |
| Add a page (Android) | ui/RedlineRoot.kt (Tab, when), new ui package, ViewModel, state wiring |
| Change player behaviour | player/mod.rs (commands/engine), dsp.rs (audio path), player/tests.rs, src/app.rs + now_bar.rs, mobile/music.rs |
| Change link checking | linkcheck.rs (status mapping, waits, ranking) and its tests |
| New persisted field | struct + #[serde(default)] + save/load, mobile records + Settings/Convert settings screens |
| New background job type | jobs.rs JobSpec + worker match, tools module, history/dedup implications, mobile job_record |
| Tool install/update | tools/install.rs, tools/mod.rs ToolPaths, settings page, mobile engine tools_status |
| Platform support | platform.rs (hosts, canonical, accent), theme::accent_for, saved grouping, mobile Platform enum |

---

## Appendix C: documents in the repo

| File | Role |
| --- | --- |
| README.md | features, requirements, build, release, layout, egui pin note |
| PLAYER_PROMPT.md | historical brief for the core/UI split and player phases 0-5 (speed, EQ, links, playlists, media keys) |
| SAVED_MUSIC_PLAN.md | historical brief for platform detection, accents, Saved collections, sync, keep-offline, launch refresh |
| SPEC.md | this document |

The two briefs are useful for why the architecture looks the way it does; they describe earlier states, so trust the code (and this sheet) where they disagree.
