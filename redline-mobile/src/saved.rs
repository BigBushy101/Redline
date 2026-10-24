//! Saved music for Kotlin: what's been saved from each platform (playlists, sets and albums
//! kept in step with the original, and single songs), on [`Music`] since it plays through the
//! same queue. The desktop's Saved page logic (src/ui/saved.rs) without the page.
//!
//! Refreshing a collection lists it fresh with yt-dlp, which takes seconds to minutes, so it
//! runs on a thread of its own, one collection at a time; Kotlin sees which one through the
//! records' [`RefreshState`] and hears about it through the music listener.

use std::collections::VecDeque;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use redline_core::history::History;
use redline_core::library::{self, Track};
use redline_core::linkcheck::{self, LinkStatus};
use redline_core::platform::{self, Platform as CorePlatform};
use redline_core::player::Source;
use redline_core::saved::{self, Collection, CollectionId, Import, SaveOutcome, Saved, SavedTrack};

use crate::engine::State;
use crate::error::RedlineError;
use crate::music::{Inner, Music};
use crate::records::{FixReportRecord, ReplacementRecord, TrackRecord, track_record};

/// Collections not refreshed for this long are refreshed on launch (the desktop's 12 hours).
pub(crate) const STALE_SECS: i64 = 12 * 3600;

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

// ---- Records ----------------------------------------------------------------------------

/// Where a link is from. Kotlin picks each one's color and icon.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    YouTube,
    SoundCloud,
    Bandcamp,
    Mixcloud,
    Vimeo,
    Other,
}

impl From<CorePlatform> for Platform {
    fn from(p: CorePlatform) -> Self {
        match p {
            CorePlatform::YouTube => Self::YouTube,
            CorePlatform::SoundCloud => Self::SoundCloud,
            CorePlatform::Bandcamp => Self::Bandcamp,
            CorePlatform::Mixcloud => Self::Mixcloud,
            CorePlatform::Vimeo => Self::Vimeo,
            CorePlatform::Other => Self::Other,
        }
    }
}

impl From<Platform> for CorePlatform {
    fn from(p: Platform) -> Self {
        match p {
            Platform::YouTube => Self::YouTube,
            Platform::SoundCloud => Self::SoundCloud,
            Platform::Bandcamp => Self::Bandcamp,
            Platform::Mixcloud => Self::Mixcloud,
            Platform::Vimeo => Self::Vimeo,
            Platform::Other => Self::Other,
        }
    }
}

/// "YouTube", "SoundCloud"...
#[uniffi::export]
pub fn platform_label(platform: Platform) -> String {
    CorePlatform::from(platform).label().to_owned()
}

/// "just now", "3 h ago", "yesterday": the desktop's "Synced …" wording.
#[uniffi::export]
pub fn fmt_ago(unix_secs: i64) -> String {
    redline_core::format::ago(unix_secs, unix_now())
}

/// Where a collection's refresh is.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshState {
    Idle,
    /// In line behind another collection's refresh.
    Waiting,
    Running,
}

/// One card of the Saved overview: a platform with something saved from it.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SavedPlatformRecord {
    pub platform: Platform,
    pub label: String,
    pub collection_count: u32,
    /// Every song saved from it, once each (collections and single songs).
    pub song_count: u32,
    /// Songs saved on their own.
    pub single_count: u32,
    /// The latest refresh of any of its collections, Unix seconds.
    pub synced_at: Option<i64>,
    /// Its collections waiting for or in a refresh.
    pub refreshing: u32,
}

/// The Saved overview.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SavedOverviewRecord {
    /// Only platforms with at least one saved item, in the desktop sidebar's order.
    pub platforms: Vec<SavedPlatformRecord>,
    pub collection_count: u32,
    pub song_count: u32,
    /// [`Music::saved_version`] when this was read.
    pub version: u64,
}

/// A saved playlist, set or album, for lists.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct CollectionSummaryRecord {
    pub id: u64,
    /// From the platform, or as renamed.
    pub name: String,
    pub platform: Platform,
    /// The platform playlist / set / album link.
    pub url: String,
    /// Songs still on the platform (what Play plays).
    pub song_count: u32,
    /// Songs gone from the platform, kept and marked.
    pub removed_count: u32,
    /// Last successful refresh, Unix seconds.
    pub synced_at: Option<i64>,
    /// Why the last refresh failed; cleared by the next one that works.
    pub sync_error: Option<String>,
    pub keep_offline: bool,
    /// The first track's picture that has one.
    pub thumbnail: Option<String>,
    pub refresh: RefreshState,
}

/// A track of a collection (or of "All songs").
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SavedTrackRecord {
    pub track: TrackRecord,
    /// Gone from the platform at the last refresh ("Removed from {platform}").
    pub removed: bool,
}

/// A collection with its tracks, in the platform's order.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct CollectionRecord {
    pub summary: CollectionSummaryRecord,
    /// Of the songs still on the platform whose length is known.
    pub duration_secs: f64,
    /// Positions here are what `play_collection` takes.
    pub tracks: Vec<SavedTrackRecord>,
}

/// A platform's page: its collections and the songs saved on their own.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SavedPlatformPageRecord {
    pub platform: Platform,
    pub label: String,
    pub collections: Vec<CollectionSummaryRecord>,
    /// Newest first.
    pub songs: Vec<TrackRecord>,
    /// "All songs": every song saved from it, once each.
    pub all_song_count: u32,
    pub refreshing: u32,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveKind {
    /// A new collection.
    Collection,
    /// The playlist was already saved; it was refreshed instead.
    Refreshed,
    /// A single song.
    Song,
    AlreadySaved,
}

/// What `import_saved` did.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SavedImportRecord {
    /// Ready to show: what a Redline export added. Empty for a list of links.
    pub message: String,
    /// From a plain list of links: the ones not saved yet, to save one by one with
    /// `save_link`.
    pub links: Vec<String>,
    /// Links in the list that were saved already.
    pub skipped: u32,
}

/// What `save_link` did.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SaveOutcomeRecord {
    /// Ready to show, worded like the desktop's toast.
    pub message: String,
    pub kind: SaveKind,
    /// The collection saved or refreshed.
    pub collection_id: Option<u64>,
    /// Where it was saved, for "View".
    pub platform: Platform,
}

fn collection_summary(c: &Collection, syncs: &Syncs) -> CollectionSummaryRecord {
    CollectionSummaryRecord {
        id: c.id,
        name: c.name.clone(),
        platform: c.platform().into(),
        url: c.source_url.clone(),
        song_count: (c.tracks.len() - c.removed_count()) as u32,
        removed_count: c.removed_count() as u32,
        synced_at: c.synced_at,
        sync_error: c.sync_error.clone(),
        keep_offline: c.keep_offline,
        thumbnail: c.tracks.iter().find_map(|t| t.track.thumbnail.clone()),
        refresh: syncs.state(c.id),
    }
}

fn saved_track_record(t: &SavedTrack, history: &History) -> SavedTrackRecord {
    SavedTrackRecord { track: track_record(&t.track, history), removed: t.removed_upstream }
}

fn overview(saved: &Saved, syncs: &Syncs, version: u64) -> SavedOverviewRecord {
    let platforms: Vec<SavedPlatformRecord> = saved
        .platforms()
        .into_iter()
        .map(|(p, _)| {
            let (collections, songs) = saved.for_platform(p);
            SavedPlatformRecord {
                platform: p.into(),
                label: p.label().to_owned(),
                collection_count: collections.len() as u32,
                song_count: saved.all_songs(p).len() as u32,
                single_count: songs.len() as u32,
                synced_at: collections.iter().filter_map(|c| c.synced_at).max(),
                refreshing: collections.iter().filter(|c| syncs.state(c.id) != RefreshState::Idle).count() as u32,
            }
        })
        .collect();
    SavedOverviewRecord {
        collection_count: saved.collections.len() as u32,
        song_count: platforms.iter().map(|p| p.song_count).sum(),
        platforms,
        version,
    }
}

// ---- Refreshing -------------------------------------------------------------------------

/// Collections in line for a refresh, one at a time on a thread of its own.
#[derive(Default)]
pub(crate) struct Syncs {
    /// In order; `true` for the launch refresh's, skipped if refreshed since.
    waiting: VecDeque<(CollectionId, bool)>,
    running: Option<CollectionId>,
    /// The refresh thread is running (it may be between two collections).
    worker: bool,
}

impl Syncs {
    fn state(&self, id: CollectionId) -> RefreshState {
        if self.running == Some(id) {
            RefreshState::Running
        } else if self.waiting.iter().any(|(w, _)| *w == id) {
            RefreshState::Waiting
        } else {
            RefreshState::Idle
        }
    }
}

/// Takes in a fresh listing (or why there isn't one): the new list, or the error noted on the
/// collection. Returns the tracks new on the platform.
pub(crate) fn apply_fresh(saved: &mut Saved, id: CollectionId, fresh: Result<Vec<Track>, String>) -> Vec<Track> {
    let Some(c) = saved.get(id) else { return Vec::new() };
    match fresh.and_then(|f| saved::sync_plan(c, &f).map_err(|e| format!("{e:#}"))) {
        Ok(plan) => {
            let added = plan.added.clone();
            saved.apply_sync(id, plan);
            added
        }
        Err(e) => {
            saved.set_sync_error(id, e);
            Vec::new()
        }
    }
}

/// Queues downloads not already waiting or running in the queue. Returns how many.
fn queue_downloads(st: &mut State, planned: Vec<redline_core::download_plan::PlannedDownload>) -> usize {
    let planned = saved::not_queued(planned, &st.jobs);
    let n = planned.len();
    for job in planned {
        st.jobs.push(job.title, job.subtitle, job.spec);
    }
    n
}

/// After a refresh: downloads for the new tracks of a keep-offline collection.
fn keep_new_offline(saved: &Saved, id: CollectionId, added: &[Track], st: &mut State) -> usize {
    if added.is_empty() || !saved.get(id).is_some_and(|c| c.keep_offline) {
        return 0;
    }
    let planned = added
        .iter()
        .filter(|t| matches!(&t.source, Source::Link(u) if st.history.find_url(u).is_none()))
        .filter_map(|t| library::offline_download(t, &st.settings))
        .collect();
    queue_downloads(st, planned)
}

impl Inner {
    /// Changes Saved; the follower saves it and Kotlin hears about it.
    pub(crate) fn saved<R>(&self, f: impl FnOnce(&mut Saved) -> R) -> R {
        let r = {
            let mut ctl = self.ctl();
            let r = f(&mut ctl.saved);
            ctl.saved_version += 1;
            r
        };
        self.signal.poke();
        r
    }

    /// The launch refresh: collections not refreshed in 12 hours, oldest first, if the
    /// setting is on and yt-dlp is there.
    pub(crate) fn refresh_on_launch(self: &Arc<Self>) {
        {
            let st = self.engine.state();
            if !st.settings.refresh_saved_on_start || st.tools.ytdlp.is_none() {
                return;
            }
        }
        let stale = self.ctl().saved.stale(unix_now(), STALE_SECS);
        self.queue_syncs(stale, true);
    }

    /// Puts collections in line for a refresh and starts the refresh thread if it isn't
    /// running. Ones already in line stay where they are.
    pub(crate) fn queue_syncs(self: &Arc<Self>, ids: Vec<CollectionId>, only_stale: bool) {
        let start = {
            let mut ctl = self.ctl();
            for id in ids {
                if ctl.syncs.state(id) == RefreshState::Idle && ctl.saved.get(id).is_some() {
                    ctl.syncs.waiting.push_back((id, only_stale));
                }
            }
            ctl.saved_version += 1;
            let start = !ctl.syncs.worker && !ctl.syncs.waiting.is_empty();
            ctl.syncs.worker |= start;
            start
        };
        self.signal.poke();
        if start {
            let me = self.clone();
            let spawned = thread::Builder::new().name("redline-saved".into()).spawn(move || me.run_syncs());
            if spawned.is_err() {
                let mut ctl = self.ctl();
                ctl.syncs = Syncs::default();
                ctl.saved_version += 1;
            }
        }
    }

    /// The refresh thread: lists each collection in line fresh from its platform, without
    /// holding any lock while yt-dlp runs.
    fn run_syncs(&self) {
        loop {
            let next = {
                let mut ctl = self.ctl();
                let next = loop {
                    let Some((id, only_stale)) = ctl.syncs.waiting.pop_front() else { break None };
                    let Some(c) = ctl.saved.get(id) else { continue };
                    // Refreshed by hand since launch: skip it.
                    if only_stale && c.synced_at.is_some_and(|t| unix_now() - t < STALE_SECS) {
                        continue;
                    }
                    break Some((id, c.source_url.clone(), c.titled_links()));
                };
                if self.signal.stopped() {
                    ctl.syncs = Syncs::default();
                    return;
                }
                ctl.syncs.running = next.as_ref().map(|(id, ..)| *id);
                ctl.syncs.worker = next.is_some();
                ctl.saved_version += 1;
                next
            };
            self.signal.poke();
            let Some((id, url, titled)) = next else { return };
            let (tools, net) = {
                let st = self.engine.state();
                (st.tools.clone(), st.settings.network.clone())
            };
            let fresh = saved::sync(&tools, &net, &url, &titled).map_err(|e| format!("{e:#}"));
            self.finish_sync(id, fresh);
        }
    }

    /// Takes in a collection's refresh, and queues downloads of its new songs if it's kept
    /// offline.
    pub(crate) fn finish_sync(&self, id: CollectionId, fresh: Result<Vec<Track>, String>) {
        let queued = {
            let mut ctl = self.ctl();
            if ctl.syncs.running == Some(id) {
                ctl.syncs.running = None;
            }
            let added = apply_fresh(&mut ctl.saved, id, fresh);
            ctl.saved_version += 1;
            let queued = keep_new_offline(&ctl.saved, id, &added, &mut self.engine.state());
            if self.signal.stopped() {
                // Nothing follows any more to save it.
                ctl.saved.save_if_dirty();
            }
            queued
        };
        if queued > 0 {
            self.engine.signal.poke();
        }
        self.signal.poke();
    }
}

// ---- The API ----------------------------------------------------------------------------

/// Plays `list` from `index`, leaving out tracks gone from the platform. A removed track
/// that's tapped plays on its own (it may still have a downloaded copy).
fn play_saved(inner: &Inner, list: Vec<SavedTrack>, index: usize, shuffle: bool) {
    if let Some(t) = list.get(index).filter(|t| t.removed_upstream) {
        let track = t.track.clone();
        return inner.queue(|c, p, h| c.play_tracks(p, h, vec![track], 0, None));
    }
    let start = list[..index.min(list.len())].iter().filter(|t| !t.removed_upstream).count();
    let tracks: Vec<Track> = list.into_iter().filter(|t| !t.removed_upstream).map(|t| t.track).collect();
    if tracks.is_empty() {
        return;
    }
    inner.queue(|c, p, h| c.play_from(p, h, tracks, start, shuffle, None));
}

#[uniffi::export]
impl Music {
    // ---- Reading ----

    /// Bumped whenever Saved or a refresh changes; read the rest again only then.
    pub fn saved_version(&self) -> u64 {
        self.inner.ctl().saved_version
    }

    pub fn saved_overview(&self) -> SavedOverviewRecord {
        let ctl = self.inner.ctl();
        overview(&ctl.saved, &ctl.syncs, ctl.saved_version)
    }

    /// A platform's collections and single songs.
    pub fn saved_platform(&self, platform: Platform) -> SavedPlatformPageRecord {
        let p = CorePlatform::from(platform);
        let ctl = self.inner.ctl();
        let st = self.inner.engine.state();
        let (collections, songs) = ctl.saved.for_platform(p);
        let collections: Vec<_> = collections.into_iter().map(|c| collection_summary(c, &ctl.syncs)).collect();
        SavedPlatformPageRecord {
            platform,
            label: p.label().to_owned(),
            refreshing: collections.iter().filter(|c| c.refresh != RefreshState::Idle).count() as u32,
            collections,
            songs: songs.into_iter().map(|t| track_record(t, &st.history)).collect(),
            all_song_count: ctl.saved.all_songs(p).len() as u32,
        }
    }

    pub fn saved_collection(&self, id: u64) -> Option<CollectionRecord> {
        let ctl = self.inner.ctl();
        let c = ctl.saved.get(id)?;
        let st = self.inner.engine.state();
        Some(CollectionRecord {
            summary: collection_summary(c, &ctl.syncs),
            duration_secs: c.playable().iter().filter_map(|t| t.duration).sum(),
            tracks: c.tracks.iter().map(|t| saved_track_record(t, &st.history)).collect(),
        })
    }

    /// Every song saved from a platform, once each, most recently added first. Positions are
    /// what `play_saved_songs` takes.
    pub fn saved_all_songs(&self, platform: Platform) -> Vec<SavedTrackRecord> {
        let all = self.inner.ctl().saved.all_songs(platform.into());
        let st = self.inner.engine.state();
        all.iter().map(|t| saved_track_record(t, &st.history)).collect()
    }

    /// Whether a link is saved anywhere, as a song or in a collection (any URL shape of it).
    pub fn is_saved(&self, link: String) -> bool {
        self.inner.ctl().saved.contains(&platform::canonical(&link))
    }

    // ---- Saving ----

    /// Saves a link: a playlist, set or album becomes a collection (or refreshes the one saved
    /// from it), anything else a single song. Blocks while yt-dlp looks it up, unless the
    /// Download page just did.
    pub fn save_link(&self, url: String) -> Result<SaveOutcomeRecord, RedlineError> {
        let url = url.trim().to_owned();
        let info = self.inner.engine.look_up_titled(&url)?;
        let (out, platform, queued) = {
            let mut ctl = self.inner.ctl();
            let out = ctl.saved.save_link(&info, &url).map_err(|e| RedlineError::Invalid(format!("{e:#}")))?;
            ctl.saved_version += 1;
            let (id, queued) = match &out {
                SaveOutcome::Collection { id, .. } => (Some(*id), 0),
                SaveOutcome::Refreshed { id, sync, .. } => {
                    (Some(*id), keep_new_offline(&ctl.saved, *id, &sync.added, &mut self.inner.engine.state()))
                }
                _ => (None, 0),
            };
            let platform = id.and_then(|id| ctl.saved.get(id)).map_or_else(|| CorePlatform::of(&url), |c| c.platform());
            (out, platform, queued)
        };
        if queued > 0 {
            self.inner.engine.signal.poke();
        }
        self.inner.signal.poke();
        let (kind, collection_id) = match &out {
            SaveOutcome::Collection { id, .. } => (SaveKind::Collection, Some(*id)),
            SaveOutcome::Refreshed { id, .. } => (SaveKind::Refreshed, Some(*id)),
            SaveOutcome::Song { .. } => (SaveKind::Song, None),
            SaveOutcome::AlreadySaved => (SaveKind::AlreadySaved, None),
        };
        Ok(SaveOutcomeRecord { message: out.message(), kind, collection_id, platform: platform.into() })
    }

    /// Saves a link track as a single song. Returns what to say ("Saved X", "Already saved").
    pub fn save_track(&self, track: TrackRecord) -> String {
        let track = track.to_core();
        if !track.is_link() {
            return "Only links can be saved.".into();
        }
        let title = track.title.clone();
        if self.inner.saved(|s| s.add_song(track)) { format!("Saved {title}") } else { "Already saved".into() }
    }

    /// The player's bookmark: saves the track, or takes it out of the single songs. A track
    /// that's only in a collection stays (the collection follows the platform). Returns what
    /// to say.
    pub fn toggle_saved(&self, track: TrackRecord) -> String {
        let key = track.to_core().key();
        let title = track.title.clone();
        let in_collection = self.inner.saved(|s| {
            if s.remove_song(&key) {
                return Ok(());
            }
            Err(s.collection_with(&key).map(|c| c.name.clone()))
        });
        match in_collection {
            Ok(()) => format!("Removed {title} from Saved"),
            Err(Some(name)) => format!("Saved in {name}"),
            Err(None) => self.save_track(track),
        }
    }

    /// Takes a single song out of Saved. Returns whether it was there.
    pub fn remove_saved_song(&self, link: String) -> bool {
        let key = platform::canonical(&link);
        self.inner.saved(|s| s.remove_song(&key))
    }

    // ---- Export and import ----

    /// All of Saved as a Redline export (JSON), to write to the file the user picks.
    pub fn export_saved(&self) -> String {
        self.inner.ctl().saved.export_json()
    }

    /// Reads an import file's text. A Redline export is merged in straight away (collections
    /// already saved gain only their missing songs; new kept-offline ones queue downloads). A
    /// list of links, one per line, comes back as the links still to save.
    pub fn import_saved(&self, text: String) -> Result<SavedImportRecord, RedlineError> {
        let import = saved::parse_import(&text).map_err(|e| RedlineError::Invalid(format!("{e:#}")))?;
        let export = match import {
            Import::Links(links) => {
                let total = links.len();
                let links = self.inner.ctl().saved.unsaved_links(links);
                let skipped = (total - links.len()) as u32;
                let message = if links.is_empty() {
                    format!("All {total} link{} in the file are saved already", if total == 1 { "" } else { "s" })
                } else {
                    String::new()
                };
                return Ok(SavedImportRecord { message, links, skipped });
            }
            Import::Saved(export) => export,
        };
        let (report, queued) = {
            let mut ctl = self.inner.ctl();
            let report = ctl.saved.merge(export);
            ctl.saved_version += 1;
            let mut st = self.inner.engine.state();
            let mut queued = 0;
            if st.tools.ready_for_download() {
                for id in &report.new_offline {
                    if let Some(c) = ctl.saved.get(*id) {
                        let planned = saved::offline_jobs(c, &st.history, &st.settings);
                        queued += queue_downloads(&mut st, planned);
                    }
                }
            }
            (report, queued)
        };
        if queued > 0 {
            self.inner.engine.signal.poke();
        }
        self.inner.signal.poke();
        let more = if queued > 0 { format!("; downloading {queued} for offline") } else { String::new() };
        Ok(SavedImportRecord { message: format!("{}{more}", report.message()), links: Vec::new(), skipped: 0 })
    }

    // ---- Collections ----

    /// Refreshes a collection from its platform in the background, after any already in line.
    pub fn refresh_collection(&self, id: u64) -> Result<(), RedlineError> {
        self.refresh(vec![id])
    }

    /// "Refresh all": every collection from the platform, one after another.
    pub fn refresh_platform(&self, platform: Platform) -> Result<(), RedlineError> {
        let ids = {
            let ctl = self.inner.ctl();
            ctl.saved.for_platform(platform.into()).0.iter().map(|c| c.id).collect()
        };
        self.refresh(ids)
    }

    /// A blank name is ignored.
    pub fn rename_collection(&self, id: u64, name: String) {
        self.inner.saved(|s| s.rename(id, &name));
    }

    pub fn remove_collection(&self, id: u64) {
        self.inner.saved(|s| s.remove(id));
    }

    /// Forgets the songs the platform removed.
    pub fn clear_removed(&self, id: u64) {
        self.inner.saved(|s| s.clear_removed(id));
    }

    /// Keep offline downloads every song not downloaded yet, now and the new ones after each
    /// refresh. Turning it on returns how many downloads were queued (0: all downloaded or on
    /// their way).
    pub fn set_keep_offline(&self, id: u64, on: bool) -> Result<u32, RedlineError> {
        let queued = {
            let mut ctl = self.inner.ctl();
            let mut st = self.inner.engine.state();
            if on && !st.tools.ready_for_download() {
                return Err(RedlineError::ToolsMissing("Downloading needs yt-dlp and ffmpeg.".into()));
            }
            ctl.saved.set_keep_offline(id, on);
            ctl.saved_version += 1;
            match ctl.saved.get(id).filter(|_| on) {
                Some(c) => {
                    let planned = saved::offline_jobs(c, &st.history, &st.settings);
                    queue_downloads(&mut st, planned)
                }
                None => 0,
            }
        };
        if queued > 0 {
            self.inner.engine.signal.poke();
        }
        self.inner.signal.poke();
        Ok(queued as u32)
    }

    /// Copies the collection's songs (not the removed ones) to the end of a playlist, or of a
    /// new one named after it. Returns the playlist's id.
    pub fn copy_collection_to_playlist(&self, id: u64, playlist: Option<u64>) -> Option<u64> {
        let (name, songs) = {
            let ctl = self.inner.ctl();
            let c = ctl.saved.get(id)?;
            (c.name.clone(), c.playable())
        };
        Some(self.inner.library(|l| {
            let target = playlist.filter(|p| l.get(*p).is_some()).unwrap_or_else(|| l.create(&name));
            l.add(target, songs);
            target
        }))
    }

    // ---- Dead links ----

    /// Swaps a replacement into every saved track with this link (in collections and the
    /// single songs), keeping its title and place. Returns whether there was one.
    pub fn replace_saved_track(&self, location: String, candidate: ReplacementRecord) -> bool {
        let key = platform::canonical(&location);
        let c = candidate.to_core();
        let done = self.inner.saved(|s| s.replace_link(&key, &c));
        if done {
            linkcheck::shared().record(&c.url, LinkStatus::Alive);
        }
        done
    }

    /// "Fix unavailable" for a collection: like `fix_unavailable_playlist`. Blocks for a few
    /// seconds per dead track.
    pub fn fix_unavailable_collection(&self, id: u64) -> Result<FixReportRecord, RedlineError> {
        let tracks: Vec<Track> = {
            let ctl = self.inner.ctl();
            ctl.saved.get(id).map(|c| c.tracks.iter().map(|t| t.track.clone()).collect()).unwrap_or_default()
        };
        let items = self.find_fixes(&tracks)?;
        Ok(self.fix_report(items, |_, was, c| self.inner.saved(|s| s.replace_link(&was.key(), c))))
    }

    // ---- Playing ----

    /// Replaces Now playing with a collection's songs still on the platform, from the track
    /// at `index` (or a random one, shuffled). A removed track plays on its own.
    pub fn play_collection(&self, id: u64, index: u32, shuffle: bool) {
        let Some(list) = self.inner.ctl().saved.get(id).map(|c| c.tracks.clone()) else { return };
        play_saved(&self.inner, list, index as usize, shuffle);
    }

    /// Like `play_collection`, for a platform's "All songs".
    pub fn play_saved_songs(&self, platform: Platform, index: u32, shuffle: bool) {
        let list = self.inner.ctl().saved.all_songs(platform.into());
        play_saved(&self.inner, list, index as usize, shuffle);
    }
}

impl Music {
    fn refresh(&self, ids: Vec<CollectionId>) -> Result<(), RedlineError> {
        if self.inner.engine.state().tools.ytdlp.is_none() {
            return Err(RedlineError::ToolsMissing("Refreshing needs yt-dlp.".into()));
        }
        self.inner.queue_syncs(ids, false);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{one_engine_at_a_time, test_dirs, wait_for};
    use crate::music::tests::music_with_tools;
    use crate::records::TrackKind;
    use redline_core::tools::ToolPaths;
    use redline_core::tools::ytdlp::{MediaInfo, PlaylistItem};

    fn playlist_info(ids: &[&str]) -> MediaInfo {
        MediaInfo {
            title: "Mix".into(),
            is_playlist: true,
            items: ids
                .iter()
                .map(|id| PlaylistItem {
                    url: format!("https://www.youtube.com/watch?v={id}"),
                    title: id.to_uppercase(),
                    duration: Some(60.0),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn yt(id: &str) -> Track {
        Track::new(Source::Link(format!("https://youtu.be/{id}")), id.to_uppercase())
    }

    #[test]
    fn fresh_listings_are_taken_in() {
        let mut s = Saved::default();
        s.save_link(&playlist_info(&["a", "b"]), "https://www.youtube.com/playlist?list=PL1").unwrap();
        let added = apply_fresh(&mut s, 1, Ok(vec![yt("a"), yt("c")]));
        assert_eq!(added.len(), 1);
        let c = s.get(1).unwrap();
        assert_eq!((c.tracks.len(), c.removed_count(), c.sync_error.as_deref()), (3, 1, None));
        assert!(apply_fresh(&mut s, 1, Ok(vec![])).is_empty());
        assert!(s.get(1).unwrap().sync_error.is_some(), "an empty listing is an error");
        apply_fresh(&mut s, 1, Err("Private video".into()));
        assert_eq!(s.get(1).unwrap().sync_error.as_deref(), Some("Private video"));
        assert!(apply_fresh(&mut s, 9, Ok(vec![yt("a")])).is_empty(), "gone");

        let syncs = Syncs { waiting: [(1, false)].into(), running: Some(2), worker: true };
        assert_eq!((syncs.state(1), syncs.state(2), syncs.state(3)), (RefreshState::Waiting, RefreshState::Running, RefreshState::Idle));
        let o = overview(&s, &syncs, 7);
        assert_eq!((o.collection_count, o.song_count, o.version), (1, 3, 7));
        assert_eq!(o.platforms[0].platform, Platform::YouTube);
        assert_eq!(o.platforms[0].refreshing, 1);
        assert_eq!(platform_label(Platform::SoundCloud), "SoundCloud");
    }

    #[test]
    fn saved_through_the_api() {
        let _turn = one_engine_at_a_time();
        let dir = test_dirs();
        // yt-dlp "is there" but can't run: refreshes start and fail without the network.
        let missing = dir.join("no-such-tool");
        let tools = ToolPaths { ytdlp: Some(missing.clone()), ffmpeg: Some(missing.clone()), ffprobe: Some(missing), ..Default::default() };
        let (engine, music, _) = music_with_tools(tools, 8.0);
        let url = "https://www.youtube.com/playlist?list=PLsaved";
        music.inner.engine.state().remember(url.into(), playlist_info(&["a", "b", "c"]));

        let v0 = music.saved_version();
        let out = music.save_link(format!("  {url} ")).unwrap();
        assert_eq!((out.kind, out.platform, out.message.as_str()), (SaveKind::Collection, Platform::YouTube, "Saved 3 songs from Mix"));
        let id = out.collection_id.unwrap();
        assert!(music.saved_version() > v0);
        assert_eq!(music.save_link(url.into()).unwrap().kind, SaveKind::Refreshed);
        assert!(music.is_saved("https://youtu.be/b?si=x".into()));
        assert!(!music.is_saved("https://youtu.be/zz".into()));

        let o = music.saved_overview();
        assert_eq!((o.platforms.len(), o.collection_count, o.song_count), (1, 1, 3));
        let page = music.saved_platform(Platform::YouTube);
        assert_eq!((page.collections.len(), page.songs.len(), page.all_song_count), (1, 0, 3));
        assert_eq!(page.collections[0].song_count, 3);

        // Single songs.
        let song = TrackRecord {
            kind: TrackKind::Link,
            location: "https://soundcloud.com/x/song".into(),
            title: "Song".into(),
            artist: None,
            duration_secs: None,
            thumbnail: None,
            added_at: 0,
            platform: None,
            offline: false,
            link_status: crate::records::LinkStatusRecord::Unknown,
            replaced_from: None,
        };
        assert_eq!(music.save_track(song.clone()), "Saved Song");
        assert_eq!(music.save_track(song.clone()), "Already saved");
        assert_eq!(music.saved_platform(Platform::SoundCloud).songs.len(), 1);
        assert_eq!(music.toggle_saved(song.clone()), "Removed Song from Saved");
        let in_mix = TrackRecord { location: "https://www.youtube.com/watch?v=a".into(), ..song.clone() };
        assert_eq!(music.toggle_saved(in_mix), "Saved in Mix");
        assert_eq!(music.toggle_saved(song.clone()), "Saved Song");
        assert!(music.remove_saved_song("https://m.soundcloud.com/x/song/".into()));
        assert!(music.saved_platform(Platform::SoundCloud).songs.is_empty());

        // A refresh runs in the background; this one fails, and says why.
        music.refresh_collection(id).unwrap();
        wait_for("the failed refresh", || {
            music.saved_collection(id).is_some_and(|c| c.summary.sync_error.is_some() && c.summary.refresh == RefreshState::Idle)
        });
        // A listing that works: a new song, and one gone (kept, marked).
        music.inner.finish_sync(id, Ok(vec![yt("a"), yt("c"), yt("d")]));
        let c = music.saved_collection(id).unwrap();
        assert_eq!((c.summary.song_count, c.summary.removed_count, c.summary.sync_error), (3, 1, None));
        assert_eq!(c.tracks.iter().map(|t| (t.track.title.as_str(), t.removed)).collect::<Vec<_>>(),
            [("A", false), ("B", true), ("C", false), ("D", false)]);
        assert_eq!(c.duration_secs, 120.0, "only the known lengths of what plays");

        // Playing leaves the removed song out, unless it's the one tapped.
        music.play_collection(id, 2, false);
        let s = music.snapshot();
        assert_eq!((s.queue_length, s.queue_index), (3, Some(1)));
        music.play_collection(id, 1, false);
        assert_eq!(music.snapshot().queue_length, 1);
        music.play_saved_songs(Platform::YouTube, 0, true);
        assert_eq!(music.snapshot().queue_length, 3);
        music.set_shuffle(false);
        music.stop();

        // Keep offline queues what isn't downloaded, and the new songs after a refresh.
        assert_eq!(music.set_keep_offline(id, true).unwrap(), 3);
        music.inner.finish_sync(id, Ok(vec![yt("a"), yt("c"), yt("d"), yt("e")]));
        assert!(engine.queue_snapshot().iter().any(|j| j.title == "E"), "the new song is downloading");
        assert!(music.saved_collection(id).unwrap().summary.keep_offline);

        music.rename_collection(id, "  Road trip ".into());
        music.clear_removed(id);
        let c = music.saved_collection(id).unwrap();
        assert_eq!((c.summary.name.as_str(), c.summary.removed_count, c.tracks.len()), ("Road trip", 0, 4));
        let pid = music.copy_collection_to_playlist(id, None).unwrap();
        assert_eq!(music.playlist(pid).unwrap().tracks.len(), 4);
        music.delete_playlist(pid);

        let saved = dir.join("data").join("saved.json");
        wait_for("Saved to be saved", || std::fs::read_to_string(&saved).is_ok_and(|s| s.contains("Road trip")));
        music.remove_collection(id);
        assert!(music.saved_overview().platforms.is_empty());
        assert!(music.saved_collection(id).is_none());
        engine.cancel_all();
        engine.clear_finished();
        drop(music);
        drop(engine);
        assert!(!std::fs::read_to_string(&saved).unwrap().contains("Road trip"), "the removal was saved");
    }
}
