//! Saved music, grouped by the platform it's from: playlists, sets and albums kept in step
//! with the original (one way, pulling from the platform), and single links saved on their
//! own. Kept in `saved.json`, apart from the user's own playlists.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::download_plan::PlannedDownload;
use crate::history::History;
use crate::jobs::{JobManager, JobSpec};
use crate::library::{self, Track};
use crate::platform::{self, Platform};
use crate::player::Source;
use crate::settings::{self, Settings};
use crate::tools::ToolPaths;
use crate::tools::ytdlp::{self, MediaInfo, NetworkOptions};

pub type CollectionId = u64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTrack {
    pub track: Track,
    /// Gone from the platform's list at the last sync. Kept rather than deleted: videos
    /// get taken down, and there may be an offline copy worth keeping.
    #[serde(default)]
    pub removed_upstream: bool,
}

/// A platform playlist, set or album, saved whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: CollectionId,
    /// From the platform; the user can rename it.
    pub name: String,
    /// The platform playlist / set / album link.
    pub source_url: String,
    /// In the platform's order.
    #[serde(default)]
    pub tracks: Vec<SavedTrack>,
    /// Unix seconds.
    #[serde(default)]
    pub synced_at: Option<i64>,
    #[serde(default)]
    pub keep_offline: bool,
    #[serde(default)]
    pub sync_error: Option<String>,
    /// yt-dlp's idea of the site, only kept when the host isn't one we recognize (a
    /// Bandcamp artist on their own domain).
    #[serde(default)]
    pub extractor_platform: Option<Platform>,
}

impl Collection {
    pub fn platform(&self) -> Platform {
        match Platform::of(&self.source_url) {
            Platform::Other => self.extractor_platform.unwrap_or(Platform::Other),
            p => p,
        }
    }

    /// Links of the tracks that already have a name besides their link.
    pub fn titled_links(&self) -> HashSet<String> {
        self.tracks
            .iter()
            .filter_map(|t| match &t.track.source {
                Source::Link(u) if *u != t.track.title => Some(u.clone()),
                _ => None,
            })
            .collect()
    }

    /// What plays when the collection is started: everything still on the platform.
    pub fn playable(&self) -> Vec<Track> {
        self.tracks
            .iter()
            .filter(|t| !t.removed_upstream)
            .map(|t| t.track.clone())
            .collect()
    }

    pub fn removed_count(&self) -> usize {
        self.tracks.iter().filter(|t| t.removed_upstream).count()
    }
}

/// What a sync would change, and the list to keep afterwards.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncResult {
    /// New on the platform.
    pub added: Vec<Track>,
    /// Newly gone from the platform (already-flagged tracks aren't counted again).
    pub removed: Vec<Track>,
    /// Flagged as removed before and back now.
    pub restored: usize,
    /// The new track list, in the platform's order.
    pub order: Vec<SavedTrack>,
}

impl SyncResult {
    pub fn is_unchanged(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.restored == 0
    }
}

/// What [`Saved::save_link`] did, for the toast.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveOutcome {
    Collection { id: CollectionId, name: String, songs: usize },
    /// The playlist was already saved; it was synced instead.
    Refreshed { id: CollectionId, name: String, sync: SyncResult },
    Song { title: String },
    AlreadySaved,
}

impl SaveOutcome {
    pub fn message(&self) -> String {
        let s = |n: usize| if n == 1 { "" } else { "s" };
        match self {
            SaveOutcome::Collection { name, songs, .. } => format!("Saved {songs} song{} from {name}", s(*songs)),
            SaveOutcome::Refreshed { name, sync, .. } if sync.is_unchanged() => {
                format!("Already saved; {name} is up to date")
            }
            SaveOutcome::Refreshed { name, sync, .. } => format!(
                "Already saved; refreshed {name} ({} new, {} removed)",
                sync.added.len(),
                sync.removed.len()
            ),
            SaveOutcome::Song { title } => format!("Saved {title}"),
            SaveOutcome::AlreadySaved => "Already saved".to_owned(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct SavedFile {
    #[serde(default)]
    collections: Vec<Collection>,
    #[serde(default)]
    songs: Vec<Track>,
}

#[derive(Serialize)]
struct SavedFileRef<'a> {
    collections: &'a [Collection],
    songs: &'a [Track],
}

#[derive(Default)]
pub struct Saved {
    pub collections: Vec<Collection>,
    /// Single links saved on their own.
    pub songs: Vec<Track>,
    dirty: bool,
    /// Every saved track's key, built on first use. Rows ask "is this saved?" every frame.
    keys: OnceCell<HashSet<String>>,
}

fn saved_path() -> PathBuf {
    settings::data_dir().join("saved.json")
}

impl Saved {
    pub fn load() -> Self {
        Self::load_from(&saved_path())
    }

    fn load_from(path: &Path) -> Self {
        let file: SavedFile = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            collections: file.collections,
            songs: file.songs,
            dirty: false,
            keys: OnceCell::new(),
        }
    }

    pub fn save_if_dirty(&mut self) {
        if std::mem::take(&mut self.dirty) {
            self.save_to(&saved_path());
        }
    }

    fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = SavedFileRef {
            collections: &self.collections,
            songs: &self.songs,
        };
        if let Ok(json) = serde_json::to_string(&file) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Marks the store as changed: needs a save, and the key index is out of date.
    fn touch(&mut self) {
        self.dirty = true;
        self.keys.take();
    }

    pub fn get(&self, id: CollectionId) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == id)
    }

    /// The collection, for changing; marks the store as needing a save.
    pub fn get_mut(&mut self, id: CollectionId) -> Option<&mut Collection> {
        let i = self.collections.iter().position(|c| c.id == id)?;
        self.touch();
        Some(&mut self.collections[i])
    }

    /// Platforms with at least one saved collection or song, and how many, in sidebar order.
    pub fn platforms(&self) -> Vec<(Platform, usize)> {
        Platform::ALL
            .into_iter()
            .map(|p| {
                let (c, s) = self.for_platform(p);
                (p, c.len() + s.len())
            })
            .filter(|(_, n)| *n > 0)
            .collect()
    }

    /// A platform's collections and single songs.
    pub fn for_platform(&self, p: Platform) -> (Vec<&Collection>, Vec<&Track>) {
        (
            self.collections.iter().filter(|c| c.platform() == p).collect(),
            self.songs.iter().filter(|t| t.platform() == Some(p)).collect(),
        )
    }

    /// Every track saved for the platform, once each (by [`Track::key`]), most recently
    /// added first. Flagged as removed only if every copy is.
    pub fn all_songs(&self, p: Platform) -> Vec<SavedTrack> {
        let (collections, songs) = self.for_platform(p);
        let mut out: Vec<SavedTrack> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let loose = songs.into_iter().map(|t| SavedTrack {
            track: t.clone(),
            removed_upstream: false,
        });
        for t in collections.iter().flat_map(|c| c.tracks.iter().cloned()).chain(loose) {
            match seen.get(&t.track.key()) {
                Some(&i) => {
                    let kept = &mut out[i];
                    kept.removed_upstream &= t.removed_upstream;
                    if t.track.added_at > kept.track.added_at {
                        kept.track.added_at = t.track.added_at;
                    }
                }
                None => {
                    seen.insert(t.track.key(), out.len());
                    out.push(t);
                }
            }
        }
        out.sort_by_key(|t| std::cmp::Reverse(t.track.added_at));
        out
    }

    /// Whether a track with this key is saved anywhere, as a song or in a collection.
    pub fn contains(&self, key: &str) -> bool {
        self.keys
            .get_or_init(|| {
                let in_collections = self.collections.iter().flat_map(|c| c.tracks.iter().map(|t| t.track.key()));
                self.songs.iter().map(Track::key).chain(in_collections).collect()
            })
            .contains(key)
    }

    /// The saved collection for a playlist link, matched by canonical link.
    pub fn find_collection(&self, url: &str) -> Option<&Collection> {
        let key = platform::canonical(url);
        self.collections.iter().find(|c| platform::canonical(&c.source_url) == key)
    }

    /// Saves a looked-up link: a playlist becomes a collection (or refreshes the one already
    /// saved from it), anything else a single song.
    pub fn save_link(&mut self, info: &MediaInfo, url: &str) -> Result<SaveOutcome> {
        let url = url.trim();
        let tracks = library::tracks_from_info(url, info);
        if !info.is_playlist {
            let Some(track) = tracks.into_iter().next() else { bail!("Nothing playable at this link.") };
            if self.songs.iter().any(|t| t.key() == track.key()) {
                return Ok(SaveOutcome::AlreadySaved);
            }
            let title = track.title.clone();
            self.songs.insert(0, track);
            self.touch();
            return Ok(SaveOutcome::Song { title });
        }
        if let Some(id) = self.find_collection(url).map(|c| c.id) {
            let c = self.get(id).expect("just found");
            let sync = sync_plan(c, &tracks)?;
            let name = c.name.clone();
            self.apply_sync(id, sync.clone());
            return Ok(SaveOutcome::Refreshed { id, name, sync });
        }
        if tracks.is_empty() {
            bail!("This playlist has nothing playable in it.");
        }
        // A playlist can list a video twice; sync_plan keeps the first, so saving does too,
        // or the first refresh would "lose" a song.
        let mut keys = HashSet::new();
        let tracks: Vec<Track> = tracks.into_iter().filter(|t| keys.insert(t.key())).collect();
        let id = self.collections.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        let extractor_platform = match Platform::of(url) {
            Platform::Other => info.extractor.as_deref().map(Platform::from_extractor),
            _ => None,
        };
        let songs = tracks.len();
        self.collections.push(Collection {
            id,
            name: info.title.clone(),
            source_url: url.to_owned(),
            tracks: tracks
                .into_iter()
                .map(|track| SavedTrack {
                    track,
                    removed_upstream: false,
                })
                .collect(),
            synced_at: Some(chrono::Utc::now().timestamp()),
            keep_offline: false,
            sync_error: None,
            extractor_platform,
        });
        self.touch();
        Ok(SaveOutcome::Collection {
            id,
            name: info.title.clone(),
            songs,
        })
    }

    /// Takes in a sync: the new list, the time, and no error.
    pub fn apply_sync(&mut self, id: CollectionId, sync: SyncResult) {
        if let Some(c) = self.get_mut(id) {
            c.tracks = sync.order;
            c.synced_at = Some(chrono::Utc::now().timestamp());
            c.sync_error = None;
        }
    }

    pub fn set_sync_error(&mut self, id: CollectionId, error: String) {
        if let Some(c) = self.get_mut(id) {
            c.sync_error = Some(error);
        }
    }

    pub fn rename(&mut self, id: CollectionId, name: &str) {
        if let (Some(c), n) = (self.get_mut(id), name.trim())
            && !n.is_empty()
        {
            c.name = n.to_owned();
        }
    }

    pub fn remove(&mut self, id: CollectionId) {
        self.collections.retain(|c| c.id != id);
        self.touch();
    }

    /// Forgets the tracks flagged as removed from the platform.
    pub fn clear_removed(&mut self, id: CollectionId) {
        if let Some(c) = self.get_mut(id) {
            c.tracks.retain(|t| !t.removed_upstream);
        }
    }

    pub fn set_keep_offline(&mut self, id: CollectionId, on: bool) {
        if let Some(c) = self.get_mut(id) {
            c.keep_offline = on;
        }
    }

    /// Collections not synced in the last `max_age` seconds (or never), oldest first, for
    /// the refresh on launch.
    pub fn stale(&self, now: i64, max_age: i64) -> Vec<CollectionId> {
        let mut old: Vec<&Collection> = self
            .collections
            .iter()
            .filter(|c| c.synced_at.is_none_or(|t| now - t >= max_age))
            .collect();
        old.sort_by_key(|c| c.synced_at.unwrap_or(i64::MIN));
        old.into_iter().map(|c| c.id).collect()
    }

    /// The collection holding a track with this key, if any; for "it's in X" messages.
    pub fn collection_with(&self, key: &str) -> Option<&Collection> {
        self.collections.iter().find(|c| c.tracks.iter().any(|t| t.track.key() == key))
    }

    /// Adds a single song unless it's already saved as one. Returns whether it was added.
    pub fn add_song(&mut self, track: Track) -> bool {
        if !track.is_link() || self.songs.iter().any(|t| t.key() == track.key()) {
            return false;
        }
        self.songs.insert(0, track);
        self.touch();
        true
    }

    /// Takes a single song out by key. Returns whether there was one.
    pub fn remove_song(&mut self, key: &str) -> bool {
        let before = self.songs.len();
        self.songs.retain(|t| t.key() != key);
        let removed = self.songs.len() != before;
        if removed {
            self.touch();
        }
        removed
    }

    /// The whole of Saved as an export file ([`SavedExport`]), for moving it to another
    /// install or keeping a copy.
    pub fn export_json(&self) -> String {
        let export = SavedExport {
            format: EXPORT_FORMAT.to_owned(),
            version: EXPORT_VERSION,
            exported_at: chrono::Utc::now().timestamp(),
            collections: self
                .collections
                .iter()
                .map(|c| ExportCollection {
                    name: c.name.clone(),
                    source_url: c.source_url.clone(),
                    keep_offline: c.keep_offline,
                    synced_at: c.synced_at,
                    extractor_platform: c.extractor_platform,
                    tracks: c.tracks.clone(),
                })
                .collect(),
            songs: self.songs.clone(),
        };
        serde_json::to_string_pretty(&export).expect("Saved serializes")
    }

    /// Takes in an export. A collection already saved from the same link keeps its name and
    /// settings and gains only the songs it doesn't have; single songs already saved are
    /// skipped. Importing the same file twice changes nothing.
    pub fn merge(&mut self, export: SavedExport) -> MergeReport {
        let mut report = MergeReport::default();
        for c in export.collections {
            if let Some(id) = self.find_collection(&c.source_url).map(|c| c.id) {
                let existing = self.collections.iter_mut().find(|e| e.id == id).expect("just found");
                let mut have: HashSet<String> = existing.tracks.iter().map(|t| t.track.key()).collect();
                let new: Vec<SavedTrack> = c.tracks.into_iter().filter(|t| have.insert(t.track.key())).collect();
                if !new.is_empty() {
                    report.collections_merged += 1;
                    report.tracks_added += new.len();
                    existing.tracks.extend(new);
                }
                continue;
            }
            let id = self.collections.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            let mut keys = HashSet::new();
            let tracks: Vec<SavedTrack> = c.tracks.into_iter().filter(|t| keys.insert(t.track.key())).collect();
            report.collections_added += 1;
            report.tracks_added += tracks.len();
            if c.keep_offline {
                report.new_offline.push(id);
            }
            self.collections.push(Collection {
                id,
                name: c.name,
                source_url: c.source_url,
                tracks,
                synced_at: c.synced_at,
                keep_offline: c.keep_offline,
                sync_error: None,
                extractor_platform: c.extractor_platform,
            });
        }
        // Oldest first, so each lands on top in the file's order.
        for song in export.songs.into_iter().rev() {
            report.songs_added += self.add_song(song) as usize;
        }
        if report.collections_added + report.tracks_added > 0 {
            self.touch();
        }
        report
    }

    /// The links not saved yet, as a song or as a collection, so an import of a list of
    /// links only looks up what's new.
    pub fn unsaved_links(&self, links: Vec<String>) -> Vec<String> {
        links
            .into_iter()
            .filter(|u| !self.contains(&platform::canonical(u)) && self.find_collection(u).is_none())
            .collect()
    }
}

const EXPORT_FORMAT: &str = "redline-saved";
const EXPORT_VERSION: u32 = 1;

/// The export file: Saved's collections (their links, names, settings and songs) and single
/// songs. Ids and sync errors are left out; they belong to one install.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedExport {
    /// Always `redline-saved`; tells the file apart from other JSON.
    pub format: String,
    pub version: u32,
    /// Unix seconds.
    #[serde(default)]
    pub exported_at: i64,
    #[serde(default)]
    pub collections: Vec<ExportCollection>,
    #[serde(default)]
    pub songs: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportCollection {
    pub name: String,
    pub source_url: String,
    #[serde(default)]
    pub keep_offline: bool,
    #[serde(default)]
    pub synced_at: Option<i64>,
    #[serde(default)]
    pub extractor_platform: Option<Platform>,
    #[serde(default)]
    pub tracks: Vec<SavedTrack>,
}

/// What an import file holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Import {
    /// A Redline export, to [`Saved::merge`].
    Saved(SavedExport),
    /// A plain list of links, one per line, each to look up and save with [`Saved::save_link`].
    Links(Vec<String>),
}

/// Reads an import file: a Redline export, or text with one link per line (blank lines and
/// `#` comments are skipped, repeats dropped).
pub fn parse_import(text: &str) -> Result<Import> {
    let text = text.trim_start_matches('\u{feff}');
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
        && value.get("format").and_then(|f| f.as_str()) == Some(EXPORT_FORMAT)
    {
        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        if version > EXPORT_VERSION as u64 {
            bail!("This file was made by a newer Redline (format version {version}). Update Redline to import it.");
        }
        return Ok(Import::Saved(serde_json::from_value(value)?));
    }
    let mut seen = HashSet::new();
    let links: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("http://") || l.starts_with("https://"))
        .filter(|l| seen.insert(platform::canonical(l)))
        .map(str::to_owned)
        .collect();
    if links.is_empty() {
        bail!("No Redline export or links in this file.");
    }
    Ok(Import::Links(links))
}

/// What [`Saved::merge`] did, for the toast.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeReport {
    pub collections_added: usize,
    /// Already saved, and given songs they didn't have.
    pub collections_merged: usize,
    /// Songs added to collections, new or merged.
    pub tracks_added: usize,
    /// Single songs added.
    pub songs_added: usize,
    /// New collections set to keep offline, whose downloads the app should queue.
    pub new_offline: Vec<CollectionId>,
}

impl MergeReport {
    pub fn message(&self) -> String {
        let n = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
        let mut parts = Vec::new();
        if self.collections_added > 0 {
            parts.push(n(self.collections_added, "collection"));
        }
        if self.songs_added > 0 {
            parts.push(n(self.songs_added, "song"));
        }
        if self.collections_merged > 0 {
            parts.push(format!("new songs in {}", n(self.collections_merged, "saved collection")));
        }
        if parts.is_empty() {
            "Nothing new to import; it's all saved already".to_owned()
        } else {
            format!("Imported {}", parts.join(", "))
        }
    }
}

/// Lines up a collection with a fresh listing from its platform.
///
/// The result follows the platform's order. Tracks already saved are kept (matched by key,
/// so `added_at` stays) with their details refreshed; new ones go in at the platform's
/// position; ones missing upstream stay where they were relative to their neighbours,
/// flagged, and lose the flag if they come back.
///
/// An empty listing is an error: a playlist that comes back empty has far more likely
/// failed to load than been emptied.
pub fn sync_plan(collection: &Collection, fresh: &[Track]) -> Result<SyncResult> {
    if fresh.is_empty() {
        bail!("The platform returned no tracks for this playlist; it wasn't changed.");
    }
    let mut fresh_keys = HashSet::new();
    let fresh: Vec<&Track> = fresh.iter().filter(|t| fresh_keys.insert(t.key())).collect();

    // Tracks missing upstream, grouped under the nearest earlier track still there.
    let mut missing_after: HashMap<Option<String>, Vec<SavedTrack>> = HashMap::new();
    let mut old: HashMap<String, &SavedTrack> = HashMap::new();
    let mut anchor: Option<String> = None;
    let mut removed = Vec::new();
    let mut seen = HashSet::new();
    for t in &collection.tracks {
        let key = t.track.key();
        if !seen.insert(key.clone()) {
            continue;
        }
        if fresh_keys.contains(&key) {
            old.insert(key.clone(), t);
            anchor = Some(key);
        } else {
            if !t.removed_upstream {
                removed.push(t.track.clone());
            }
            missing_after.entry(anchor.clone()).or_default().push(SavedTrack {
                track: t.track.clone(),
                removed_upstream: true,
            });
        }
    }

    let mut order = missing_after.remove(&None).unwrap_or_default();
    let mut added = Vec::new();
    let mut restored = 0;
    for f in fresh {
        let key = f.key();
        let track = match old.get(&key) {
            Some(prev) => {
                restored += prev.removed_upstream as usize;
                let mut t = prev.track.clone();
                // Flat listings (SoundCloud's, for one) can carry no title, which yt-dlp's
                // entry then fills with the link or a name guessed from it; keep the name we
                // already have, unless that is only the link.
                let guessed = matches!(&f.source, Source::Link(u)
                    if *u == f.title || ytdlp::title_from_link(u).as_ref() == Some(&f.title));
                if !guessed || matches!(&t.source, Source::Link(u) if *u == t.title) {
                    t.title = f.title.clone();
                }
                t.artist = f.artist.clone().or(t.artist);
                t.duration = f.duration.or(t.duration);
                t.thumbnail = f.thumbnail.clone().or(t.thumbnail);
                t
            }
            None => {
                added.push(f.clone());
                f.clone()
            }
        };
        order.push(SavedTrack {
            track,
            removed_upstream: false,
        });
        order.extend(missing_after.remove(&Some(key)).unwrap_or_default());
    }
    Ok(SyncResult {
        added,
        removed,
        restored,
        order,
    })
}

/// The collection's link listed fresh from its platform. Private playlists and likes work
/// through the cookie settings in `net`; errors come back readable. Untitled tracks are looked
/// up unless `titled` (from [`Collection::titled_links`]) already names them.
pub fn sync(tools: &ToolPaths, net: &NetworkOptions, url: &str, titled: &HashSet<String>) -> Result<Vec<Track>> {
    let info = ytdlp::fetch_titled(tools, net, url, &|u| titled.contains(u))?;
    Ok(library::tracks_from_info(url, &info))
}

/// Downloads that would make the collection playable offline: its tracks still on the
/// platform with no finished download in History. Links already in the download archive
/// are left for yt-dlp to skip.
pub fn offline_jobs(collection: &Collection, history: &History, settings: &Settings) -> Vec<PlannedDownload> {
    collection
        .tracks
        .iter()
        .filter(|t| !t.removed_upstream)
        .filter(|t| matches!(&t.track.source, Source::Link(u) if history.find_url(u).is_none()))
        .filter_map(|t| library::offline_download(&t.track, settings))
        .collect()
}

/// Leaves out downloads of links already waiting or running in the queue, so turning
/// keep-offline off and on (or a refresh during downloads) doesn't queue them twice.
pub fn not_queued(planned: Vec<PlannedDownload>, jobs: &JobManager) -> Vec<PlannedDownload> {
    let url = |spec: &JobSpec| match spec {
        JobSpec::Download(r) => Some(platform::canonical(&r.url)),
        _ => None,
    };
    let queued: HashSet<String> = jobs
        .jobs
        .iter()
        .filter(|j| j.status.is_active())
        .filter_map(|j| url(&j.spec))
        .collect();
    planned
        .into_iter()
        .filter(|p| url(&p.spec).is_none_or(|u| !queued.contains(&u)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::JobStatus;
    use crate::tools::ytdlp::{DownloadRequest, PlaylistItem};

    fn yt(id: &str) -> Track {
        let mut t = Track::new(Source::Link(format!("https://www.youtube.com/watch?v={id}")), id.to_uppercase());
        t.added_at = 100;
        t
    }

    fn collection(tracks: &[(&str, bool)]) -> Collection {
        Collection {
            id: 1,
            name: "Mix".into(),
            source_url: "https://www.youtube.com/playlist?list=PL1".into(),
            tracks: tracks
                .iter()
                .map(|(id, removed)| SavedTrack {
                    track: yt(id),
                    removed_upstream: *removed,
                })
                .collect(),
            synced_at: None,
            keep_offline: false,
            sync_error: None,
            extractor_platform: None,
        }
    }

    fn keys(order: &[SavedTrack]) -> Vec<String> {
        order
            .iter()
            .map(|t| format!("{}{}", t.track.title, if t.removed_upstream { "-" } else { "" }))
            .collect()
    }

    /// Fresh tracks as a platform lists them: other URL shapes, new `added_at`.
    fn fresh(ids: &[&str]) -> Vec<Track> {
        ids.iter()
            .map(|id| {
                let mut t = Track::new(Source::Link(format!("https://youtu.be/{id}?si=x")), id.to_uppercase());
                t.added_at = 999;
                t
            })
            .collect()
    }

    #[test]
    fn sync_adds_new_tracks_at_the_platform_position() {
        let c = collection(&[("a", false), ("b", false)]);
        let r = sync_plan(&c, &fresh(&["a", "n", "b"])).unwrap();
        assert_eq!(keys(&r.order), ["A", "N", "B"]);
        assert_eq!(r.added.len(), 1);
        assert!(r.removed.is_empty());
        assert_eq!(r.order[0].track.added_at, 100, "kept tracks keep added_at");
        assert_eq!(r.order[1].track.added_at, 999);
        assert!(!r.is_unchanged());

        // A listing without titles doesn't wipe the ones already known.
        let mut untitled = fresh(&["a"]);
        untitled[0].title = "https://youtu.be/a?si=x".into();
        let r = sync_plan(&c, &untitled).unwrap();
        assert_eq!(r.order[0].track.title, "A");
    }

    #[test]
    fn sync_prefers_known_titles_over_guessed_ones() {
        let link = "https://soundcloud.com/forss/funk-for-nerds";
        let sc = |title: &str| Track::new(Source::Link(link.into()), title.to_string());
        let mut c = collection(&[]);
        c.tracks.push(SavedTrack {
            track: sc("Funk for Nerds"),
            removed_upstream: false,
        });
        // A name guessed from the link doesn't replace the real one...
        let r = sync_plan(&c, &[sc("Funk For Nerds")]).unwrap();
        assert_eq!(r.order[0].track.title, "Funk for Nerds");
        // ...but does replace a bare link saved before titles were guessed.
        c.tracks[0].track.title = link.into();
        let r = sync_plan(&c, &[sc("Funk For Nerds")]).unwrap();
        assert_eq!(r.order[0].track.title, "Funk For Nerds");
    }

    #[test]
    fn sync_keeps_removed_tracks_flagged_in_place() {
        let c = collection(&[("a", false), ("b", false), ("c", false), ("d", false)]);
        let r = sync_plan(&c, &fresh(&["a", "c"])).unwrap();
        assert_eq!(keys(&r.order), ["A", "B-", "C", "D-"]);
        assert_eq!(r.removed.len(), 2);

        // Missing from the start stays at the start.
        let r = sync_plan(&c, &fresh(&["b", "c", "d"])).unwrap();
        assert_eq!(keys(&r.order), ["A-", "B", "C", "D"]);

        // Already-flagged tracks aren't reported again.
        let c = collection(&[("a", false), ("b", true)]);
        let r = sync_plan(&c, &fresh(&["a"])).unwrap();
        assert!(r.is_unchanged());
        assert_eq!(keys(&r.order), ["A", "B-"]);
    }

    #[test]
    fn sync_restores_a_track_that_comes_back() {
        let c = collection(&[("a", false), ("b", true), ("c", false)]);
        let r = sync_plan(&c, &fresh(&["a", "b", "c"])).unwrap();
        assert_eq!(keys(&r.order), ["A", "B", "C"]);
        assert_eq!(r.restored, 1);
        assert!(r.added.is_empty());
        assert_eq!(r.order[1].track.added_at, 100);
    }

    #[test]
    fn sync_follows_a_reorder() {
        let c = collection(&[("a", false), ("b", false), ("x", false), ("c", false)]);
        let r = sync_plan(&c, &fresh(&["c", "b", "a"])).unwrap();
        // x was after b in the old order, so it stays after b.
        assert_eq!(keys(&r.order), ["C", "B", "X-", "A"]);
        assert!(r.order.iter().filter(|t| !t.removed_upstream).all(|t| t.track.added_at == 100));
    }

    #[test]
    fn sync_refuses_an_empty_listing() {
        let c = collection(&[("a", false)]);
        assert!(sync_plan(&c, &[]).is_err());
    }

    #[test]
    fn sync_drops_duplicates_in_the_listing() {
        let c = collection(&[]);
        let r = sync_plan(&c, &fresh(&["a", "a", "b"])).unwrap();
        assert_eq!(keys(&r.order), ["A", "B"]);
    }

    fn playlist_info(title: &str, ids: &[&str]) -> MediaInfo {
        MediaInfo {
            title: title.into(),
            is_playlist: true,
            items: ids
                .iter()
                .map(|id| PlaylistItem {
                    url: format!("https://www.youtube.com/watch?v={id}"),
                    title: id.to_uppercase(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn saving_drops_repeats_like_refresh_does() {
        let mut s = Saved::default();
        let url = "https://www.youtube.com/playlist?list=PL1";
        let out = s.save_link(&playlist_info("Mix", &["a", "b", "a", "c"]), url).unwrap();
        assert_eq!(out.message(), "Saved 3 songs from Mix");
        let ids: Vec<String> = s.collections[0].tracks.iter().map(|t| t.track.title.clone()).collect();
        assert_eq!(ids, ["A", "B", "C"]);
        let sync = sync_plan(&s.collections[0], &library::tracks_from_info(url, &playlist_info("Mix", &["a", "b", "a", "c"]))).unwrap();
        assert!(sync.added.is_empty() && sync.removed.is_empty(), "{sync:?}");
    }

    #[test]
    fn saves_playlists_and_songs_and_round_trips() {
        let mut s = Saved::default();
        let url = "https://www.youtube.com/playlist?list=PL1";
        let out = s.save_link(&playlist_info("Mix", &["a", "b"]), url).unwrap();
        assert_eq!(out.message(), "Saved 2 songs from Mix");
        let out = s.save_link(&playlist_info("Mix", &["a", "b", "c"]), &format!("{url}&si=zz")).unwrap();
        let SaveOutcome::Refreshed { sync, .. } = &out else { panic!("{out:?}") };
        assert_eq!(sync.added.len(), 1);
        assert_eq!(s.collections.len(), 1);

        let single = MediaInfo {
            title: "Song".into(),
            ..Default::default()
        };
        assert!(matches!(
            s.save_link(&single, "https://soundcloud.com/x/song").unwrap(),
            SaveOutcome::Song { .. }
        ));
        assert_eq!(
            s.save_link(&single, "https://m.soundcloud.com/x/song/?si=1").unwrap(),
            SaveOutcome::AlreadySaved
        );
        assert!(s.contains(&platform::canonical("https://youtu.be/b")));
        s.collections[0].tracks[1].removed_upstream = true;
        s.collections[0].keep_offline = true;
        assert!(s.dirty);

        let path = std::env::temp_dir().join(format!("redline-saved-{}.json", std::process::id()));
        s.save_to(&path);
        let back = Saved::load_from(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.collections, s.collections);
        assert_eq!(back.songs, s.songs);
        assert!(Saved::load_from(&path).collections.is_empty(), "no file: empty");
    }

    #[test]
    fn platforms_hide_empty_ones() {
        let mut s = Saved::default();
        assert!(s.platforms().is_empty());
        s.save_link(&playlist_info("Mix", &["a"]), "https://www.youtube.com/playlist?list=PL1").unwrap();
        s.add_song(Track::new(Source::Link("https://artist.bandcamp.com/track/x".into()), "x".into()));
        s.add_song(Track::new(Source::Link("https://youtu.be/q".into()), "q".into()));
        assert!(!s.add_song(Track::new(Source::Local("a.mp3".into()), "a".into())), "local files aren't saved");
        assert_eq!(s.platforms(), [(Platform::YouTube, 2), (Platform::Bandcamp, 1)]);
        let (c, songs) = s.for_platform(Platform::YouTube);
        assert_eq!((c.len(), songs.len()), (1, 1));
        assert!(s.contains(&platform::canonical("https://youtu.be/q")));
        assert!(s.remove_song(&platform::canonical("https://youtu.be/q")));
        assert!(!s.contains(&platform::canonical("https://youtu.be/q")), "the key index follows changes");
        s.remove(1);
        assert_eq!(s.platforms(), [(Platform::Bandcamp, 1)]);
    }

    #[test]
    fn unknown_hosts_fall_back_to_the_extractor() {
        let mut s = Saved::default();
        let mut info = playlist_info("Album", &["a"]);
        info.extractor = Some("BandcampAlbum".into());
        s.save_link(&info, "https://music.someartist.com/album/x").unwrap();
        assert_eq!(s.collections[0].platform(), Platform::Bandcamp);
    }

    #[test]
    fn all_songs_removes_duplicates_newest_first() {
        let mut s = Saved::default();
        let mut a = collection(&[("a", false), ("b", true)]);
        a.tracks[0].track.added_at = 50;
        let mut b = collection(&[("b", false), ("c", false)]);
        b.id = 2;
        b.tracks[1].track.added_at = 300;
        s.collections = vec![a, b];
        let mut loose = Track::new(Source::Link("https://youtu.be/a".into()), "A".into());
        loose.added_at = 200;
        s.songs.push(loose);
        s.songs.push(Track::new(Source::Link("https://soundcloud.com/x/y".into()), "Y".into()));

        let all = s.all_songs(Platform::YouTube);
        assert_eq!(keys(&all), ["C", "A", "B"], "deduplicated, newest first");
        assert_eq!(all[1].track.added_at, 200, "the newest add of a duplicate counts");
        assert!(!all[2].removed_upstream, "still present in one collection");
    }

    #[test]
    fn offline_jobs_skip_downloaded_and_removed_tracks() {
        let c = collection(&[("a", false), ("b", false), ("r", true)]);
        let mut history = History::default();
        let mut jobs = JobManager::new();
        jobs.push(
            "A".into(),
            String::new(),
            JobSpec::Download(DownloadRequest {
                url: "https://youtu.be/a".into(),
                ..Default::default()
            }),
        );
        jobs.jobs[0].status = JobStatus::Done(Some("a.mp3".into()));
        history.record(&jobs.jobs[0]);
        let planned = offline_jobs(&c, &history, &Settings::default());
        assert_eq!(planned.iter().map(|j| j.title.as_str()).collect::<Vec<_>>(), ["B"]);
    }

    #[test]
    fn stale_collections_oldest_first() {
        let mut s = Saved::default();
        for (id, at) in [(1, Some(1000)), (2, None), (3, Some(90_000)), (4, Some(50_000))] {
            let mut c = collection(&[]);
            c.id = id;
            c.synced_at = at;
            s.collections.push(c);
        }
        assert_eq!(s.stale(100_000, 12 * 3600), [2, 1, 4]);
        assert!(s.stale(1000, 12 * 3600) == [2]);
    }

    #[test]
    fn already_queued_downloads_are_left_out() {
        let c = collection(&[("a", false), ("b", false)]);
        let settings = Settings::default();
        let mut jobs = JobManager::new();
        let a = library::offline_download(&c.tracks[0].track, &settings).unwrap();
        jobs.push(a.title, a.subtitle, a.spec);
        let planned = not_queued(offline_jobs(&c, &History::default(), &settings), &jobs);
        assert_eq!(planned.iter().map(|j| j.title.as_str()).collect::<Vec<_>>(), ["B"]);
        jobs.jobs[0].status = JobStatus::Failed("x".into());
        assert_eq!(not_queued(offline_jobs(&c, &History::default(), &settings), &jobs).len(), 2, "a failed one can go again");
        assert_eq!(s_with(&c).collection_with(&c.tracks[1].track.key()).map(|c| c.id), Some(1));
    }

    fn s_with(c: &Collection) -> Saved {
        Saved {
            collections: vec![c.clone()],
            ..Default::default()
        }
    }

    fn export_of(s: &Saved) -> SavedExport {
        match parse_import(&s.export_json()).unwrap() {
            Import::Saved(e) => e,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn export_round_trips_into_an_empty_library() {
        let mut s = Saved::default();
        s.collections.push(collection(&[("a", false), ("b", true)]));
        s.collections[0].keep_offline = true;
        s.collections[0].sync_error = Some("rate limited".into());
        s.add_song(Track::new(Source::Link("https://soundcloud.com/x/y".into()), "Y".into()));
        s.add_song(Track::new(Source::Link("https://soundcloud.com/x/z".into()), "Z".into()));

        let mut back = Saved::default();
        let report = back.merge(export_of(&s));
        assert_eq!(report.collections_added, 1);
        assert_eq!(report.songs_added, 2);
        assert_eq!(report.new_offline, [1]);
        assert_eq!(report.message(), "Imported 1 collection, 2 songs");
        let c = &back.collections[0];
        assert_eq!((c.name.as_str(), c.keep_offline, c.sync_error.as_deref()), ("Mix", true, None));
        assert_eq!(c.tracks, s.collections[0].tracks, "songs and removed flags come along");
        assert_eq!(back.songs, s.songs, "in the same order");
        assert!(back.dirty);
    }

    #[test]
    fn import_merges_without_duplicates() {
        let mut theirs = Saved::default();
        theirs.collections.push(collection(&[("a", false), ("b", false), ("c", false)]));
        let mut other = collection(&[("x", false)]);
        other.id = 7;
        other.source_url = "https://www.youtube.com/playlist?list=PL2".into();
        theirs.collections.push(other);
        theirs.add_song(Track::new(Source::Link("https://youtu.be/q".into()), "Q".into()));
        theirs.add_song(Track::new(Source::Link("https://youtu.be/r".into()), "R".into()));

        let mut mine = Saved::default();
        let mut c = collection(&[("a", false)]);
        c.name = "My name for it".into();
        c.id = 3;
        c.source_url = "https://youtube.com/playlist?list=PL1&si=abc".into();
        mine.collections.push(c);
        mine.add_song(Track::new(Source::Link("https://www.youtube.com/watch?v=q".into()), "Q".into()));

        let report = mine.merge(export_of(&theirs));
        assert_eq!(
            (report.collections_added, report.collections_merged, report.tracks_added, report.songs_added),
            (1, 1, 3, 1)
        );
        assert_eq!(mine.collections.len(), 2);
        let merged = mine.get(3).unwrap();
        assert_eq!(merged.name, "My name for it", "the local name stays");
        assert_eq!(keys(&merged.tracks), ["A", "B", "C"]);
        assert_eq!(mine.collections[1].id, 4, "a fresh id");
        assert_eq!(mine.songs.len(), 2);

        // The same file again: nothing new.
        let again = mine.merge(export_of(&theirs));
        assert_eq!(again, MergeReport::default());
        assert_eq!(again.message(), "Nothing new to import; it's all saved already");
    }

    #[test]
    fn imports_a_plain_list_of_links() {
        let text = "\u{feff}# my links\nhttps://youtu.be/a\n\n  https://www.youtube.com/watch?v=a  \nnot a link\nhttps://soundcloud.com/x/y\r\nhttps://www.youtube.com/playlist?list=PL1\n";
        let Import::Links(links) = parse_import(text).unwrap() else { panic!() };
        assert_eq!(links, ["https://youtu.be/a", "https://soundcloud.com/x/y", "https://www.youtube.com/playlist?list=PL1"]);

        let mut s = Saved::default();
        s.collections.push(collection(&[]));
        s.add_song(Track::new(Source::Link("https://soundcloud.com/x/y".into()), "Y".into()));
        assert_eq!(s.unsaved_links(links), ["https://youtu.be/a"], "saved songs and collections are skipped");

        assert!(parse_import("just words\n").is_err());
        assert!(parse_import("{\"other\": 1}").is_err());
    }

    #[test]
    fn refuses_exports_from_a_newer_version() {
        let err = parse_import(r#"{"format":"redline-saved","version":2,"collections":[]}"#).unwrap_err();
        assert!(err.to_string().contains("newer Redline"), "{err}");
        assert!(matches!(parse_import(r#"{"format":"redline-saved","version":1}"#), Ok(Import::Saved(_))));
    }

    /// Save a small public YouTube playlist, then sync it twice: the second sync changes
    /// nothing.
    #[test]
    #[ignore]
    fn saves_and_syncs_a_youtube_playlist() {
        let tools = crate::tools::e2e::ensure_tools();
        let net = NetworkOptions::default();
        let url = "https://www.youtube.com/playlist?list=PLBB231211A4F62143";
        let info = ytdlp::fetch_info(&tools, &net, url).unwrap();
        let mut s = Saved::default();
        let SaveOutcome::Collection { id, songs, .. } = s.save_link(&info, url).unwrap() else { panic!() };
        assert!(songs > 1, "{songs}");
        assert_eq!(s.get(id).unwrap().platform(), Platform::YouTube);
        for round in 0..2 {
            let fresh = sync(&tools, &net, url, &s.get(id).unwrap().titled_links()).unwrap();
            let r = sync_plan(s.get(id).unwrap(), &fresh).unwrap();
            assert!(r.is_unchanged(), "round {round}: {} added, {} removed", r.added.len(), r.removed.len());
            s.apply_sync(id, r);
        }
        assert_eq!(s.get(id).unwrap().tracks.len(), songs);
    }
}
