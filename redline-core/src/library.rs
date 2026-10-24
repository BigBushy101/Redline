//! Playlists mixing links and local files, and the "Now playing" queue built from them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::download_plan::{LinkChoices, PlannedDownload};
use crate::history::History;
use crate::linkcheck;
use crate::platform::{self, Platform};
use crate::player::{Source, decode};
use crate::settings;
use crate::tools::ToolPaths;
use crate::tools::convert::{Category, classify};
use crate::tools::ytdlp::{MediaInfo, Mode};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub source: Source,
    pub title: String,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    /// Image URL.
    #[serde(default)]
    pub thumbnail: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub added_at: i64,
    /// The link this track pointed at before a replacement was swapped in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_from: Option<String>,
}

impl Track {
    pub fn new(source: Source, title: String) -> Self {
        Self {
            source,
            title,
            artist: None,
            duration: None,
            thumbnail: None,
            added_at: chrono::Utc::now().timestamp(),
            replaced_from: None,
        }
    }

    /// Points a link track at another upload of the same song, keeping its title, artist and
    /// when it was added. The first link it ever had stays in `replaced_from`.
    pub fn swap_link(&mut self, c: &linkcheck::Candidate) {
        let Source::Link(old) = &self.source else { return };
        if self.replaced_from.is_none() {
            self.replaced_from = Some(old.clone());
        }
        self.source = Source::Link(c.url.clone());
        self.thumbnail = c.thumbnail.clone().or(self.thumbnail.take());
        self.duration = c.duration.or(self.duration);
    }

    pub fn is_link(&self) -> bool {
        matches!(self.source, Source::Link(_))
    }

    /// The site a link track is from; `None` for files on this PC.
    pub fn platform(&self) -> Option<Platform> {
        match &self.source {
            Source::Link(url) => Some(Platform::of(url)),
            Source::Local(_) => None,
        }
    }

    /// What makes two tracks the same song: the canonical link, or the file path.
    pub fn key(&self) -> String {
        match &self.source {
            Source::Link(url) => platform::canonical(url),
            Source::Local(p) => p.display().to_string(),
        }
    }

    /// Text the playlist search matches against.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.title.to_lowercase().contains(&q) || self.artist.as_ref().is_some_and(|a| a.to_lowercase().contains(&q))
    }
}

pub type PlaylistId = u64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

impl Playlist {
    pub fn duration(&self) -> f64 {
        self.tracks.iter().filter_map(|t| t.duration).sum()
    }
}

#[derive(Default)]
pub struct Library {
    pub playlists: Vec<Playlist>,
    dirty: bool,
    /// The playlists file is there but couldn't be read, so `playlists` may be missing what
    /// it holds; cleared once they're saved over it.
    unreadable: bool,
}

fn library_path() -> PathBuf {
    settings::data_dir().join("playlists.json")
}

impl Library {
    pub fn load() -> Self {
        Self::load_from(&library_path())
    }

    fn load_from(path: &Path) -> Self {
        let (playlists, unreadable) = match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).map_or((Vec::new(), true), |p| (p, false)),
            Err(e) => (Vec::new(), e.kind() != std::io::ErrorKind::NotFound),
        };
        Self {
            playlists,
            dirty: false,
            unreadable,
        }
    }

    /// Whether the saved playlists couldn't be read (see [`Self::unreadable`]): nothing should
    /// be thrown away for not being in a playlist.
    pub fn is_unreadable(&self) -> bool {
        self.unreadable
    }

    pub fn save_if_dirty(&mut self) {
        if std::mem::take(&mut self.dirty) {
            self.save_to(&library_path());
            self.unreadable = false;
        }
    }

    fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(&self.playlists) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn get(&self, id: PlaylistId) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    /// The playlist, for changing; marks the library as needing a save.
    pub fn get_mut(&mut self, id: PlaylistId) -> Option<&mut Playlist> {
        let p = self.playlists.iter_mut().find(|p| p.id == id)?;
        self.dirty = true;
        Some(p)
    }

    pub fn create(&mut self, name: &str) -> PlaylistId {
        let id = self.playlists.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        let name = match name.trim() {
            "" => format!("Playlist {id}"),
            n => n.to_owned(),
        };
        self.playlists.push(Playlist {
            id,
            name,
            tracks: Vec::new(),
        });
        self.dirty = true;
        id
    }

    pub fn rename(&mut self, id: PlaylistId, name: &str) {
        if let (Some(p), n) = (self.get_mut(id), name.trim())
            && !n.is_empty()
        {
            p.name = n.to_owned();
        }
    }

    pub fn delete(&mut self, id: PlaylistId) {
        self.playlists.retain(|p| p.id != id);
        self.dirty = true;
    }

    pub fn add(&mut self, id: PlaylistId, tracks: impl IntoIterator<Item = Track>) {
        if let Some(p) = self.get_mut(id) {
            p.tracks.extend(tracks);
        }
    }

    pub fn remove_track(&mut self, id: PlaylistId, index: usize) {
        if let Some(p) = self.get_mut(id)
            && index < p.tracks.len()
        {
            p.tracks.remove(index);
        }
    }

    /// Swaps a replacement link into the track at `index`; false if there's no such track.
    pub fn replace_link(&mut self, id: PlaylistId, index: usize, c: &linkcheck::Candidate) -> bool {
        let Some(t) = self.get_mut(id).and_then(|p| p.tracks.get_mut(index)) else { return false };
        t.swap_link(c);
        true
    }

    /// Moves a track from `from` to `to` (both indices in the current order).
    pub fn move_track(&mut self, id: PlaylistId, from: usize, to: usize) {
        if let Some(p) = self.get_mut(id)
            && from < p.tracks.len()
            && to < p.tracks.len()
        {
            let t = p.tracks.remove(from);
            p.tracks.insert(to, t);
        }
    }
}

/// Audio and video files under `paths` (folders are searched all the way down), in name
/// order per folder.
pub fn media_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        collect(p, &mut out);
    }
    out
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        entries.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        for e in entries {
            collect(&e, out);
        }
    } else if matches!(classify(path), Some(Category::Audio | Category::Video)) && path.is_file() {
        out.push(path.to_path_buf());
    }
}

/// Tracks for local files, with titles and artists from their tags where ffprobe finds
/// them. Slow for big folders; run it off the UI thread.
pub fn scan(tools: &ToolPaths, paths: &[PathBuf]) -> Vec<Track> {
    media_files(paths).iter().map(|file| track_for_file(tools, file)).collect()
}

/// A track for one local file: title and artist from its tags, and its cover art.
pub fn track_for_file(tools: &ToolPaths, file: &Path) -> Track {
    let probe = tools.ffprobe.as_deref().and_then(|p| decode::probe(p, file)).unwrap_or_default();
    let source = Source::Local(file.to_path_buf());
    let mut t = Track::new(source.clone(), probe.title.unwrap_or_else(|| source.label()));
    t.artist = probe.artist;
    t.duration = probe.duration;
    t.thumbnail = cover_for(tools, file, probe.has_cover);
    t
}

/// Where a file's extracted cover art is kept.
pub fn covers_dir() -> PathBuf {
    settings::data_dir().join("covers")
}

/// A `file://` image for a local song: a cover picture next to it, else its embedded art
/// pulled out with ffmpeg into [`covers_dir`]. Android reads the art itself when it shows it.
#[cfg(not(target_os = "android"))]
fn cover_for(tools: &ToolPaths, file: &Path, embedded: bool) -> Option<String> {
    let uri = |p: &Path| Some(format!("file://{}", p.display()));
    if let Some(dir) = file.parent() {
        for name in ["cover.jpg", "folder.jpg", "front.jpg", "cover.png", "folder.png"] {
            let p = dir.join(name);
            if p.is_file() {
                return uri(&p);
            }
        }
    }
    let ffmpeg = tools.ffmpeg.as_deref().filter(|_| embedded)?;
    let out = covers_dir().join(cover_name(file));
    std::fs::create_dir_all(covers_dir()).ok()?;
    let ok = crate::tools::command(ffmpeg)
        .args(["-y", "-v", "error", "-i"])
        .arg(file)
        .args(["-an", "-map", "0:v:0", "-frames:v", "1", "-vf", "scale='min(600,iw)':-2", "-q:v", "3"])
        .arg(&out)
        .output()
        .is_ok_and(|o| o.status.success());
    if ok && out.is_file() { uri(&out) } else { None }
}

#[cfg(target_os = "android")]
fn cover_for(_tools: &ToolPaths, _file: &Path, _embedded: bool) -> Option<String> {
    None
}

/// The extracted cover's file name, one per song path.
fn cover_name(file: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    format!("{:016x}.jpg", h.finish())
}

/// Removes the extracted cover of a song that's gone; cover pictures beside songs are left.
pub fn forget_cover(track: &Track) {
    if let Some(p) = track.thumbnail.as_deref().and_then(|t| t.strip_prefix("file://")) {
        let p = Path::new(p);
        if p.starts_with(covers_dir()) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The file a track plays from, for sending it to Convert: a local song, or a link that's
/// been downloaded.
pub fn local_file(track: &Track, history: &History) -> Option<PathBuf> {
    match playable_source(track, history) {
        Source::Local(p) if p.is_file() => Some(p),
        _ => None,
    }
}

/// Tracks for a looked-up link: every entry of a playlist, or the one video.
pub fn tracks_from_info(url: &str, info: &MediaInfo) -> Vec<Track> {
    if info.is_playlist {
        return info
            .items
            .iter()
            .map(|i| {
                let mut t = Track::new(Source::Link(i.url.clone()), i.title.clone());
                t.artist = i.uploader.clone();
                t.duration = i.duration;
                t.thumbnail = i.thumbnail.clone();
                t
            })
            .collect();
    }
    let mut t = Track::new(Source::Link(url.trim().to_owned()), info.track.clone().unwrap_or_else(|| info.title.clone()));
    t.artist = info.artist.clone().or_else(|| info.uploader.clone());
    t.duration = info.duration;
    t.thumbnail = info.thumbnail.clone();
    vec![t]
}

/// A download job that saves a link track as audio with the current download options, so
/// it can play offline. `None` for local tracks.
pub fn offline_download(track: &Track, settings: &settings::Settings) -> Option<PlannedDownload> {
    let Source::Link(url) = &track.source else { return None };
    let mut settings = settings.clone();
    settings.download.mode = Mode::Audio;
    let mut job = LinkChoices::default().plan(url, None, &settings);
    job.title = track.title.clone();
    Some(job)
}

/// What to actually play for a track: a link that's been downloaded plays from the file,
/// matched by canonical link so URL variants of the same song find it.
pub fn playable_source(track: &Track, history: &History) -> Source {
    if let Source::Link(url) = &track.source
        && let Some(file) = history
            .find_url(url)
            .and_then(|e| e.output.clone())
            .filter(|p| p.is_file() && matches!(classify(p), Some(Category::Audio | Category::Video)))
    {
        return Source::Local(file);
    }
    track.source.clone()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

/// The tracks being played, in play order, and where we are in them.
#[derive(Default)]
pub struct NowPlaying {
    pub tracks: Vec<Track>,
    /// Play order: indices into `tracks`. Shuffled when shuffle is on.
    order: Vec<usize>,
    /// Position in `order`.
    pos: usize,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// The playlist it was started from, if any.
    pub playlist: Option<PlaylistId>,
    seed: u64,
}

impl NowPlaying {
    /// Replaces the queue with `tracks`, starting at `start`.
    pub fn start(&mut self, tracks: Vec<Track>, start: usize, playlist: Option<PlaylistId>) {
        self.tracks = tracks;
        self.playlist = playlist;
        self.order = (0..self.tracks.len()).collect();
        self.pos = start.min(self.tracks.len().saturating_sub(1));
        if self.shuffle {
            self.reshuffle();
        }
    }

    /// Adds tracks after the rest of the queue.
    pub fn append(&mut self, tracks: impl IntoIterator<Item = Track>) {
        for t in tracks {
            self.order.push(self.tracks.len());
            self.tracks.push(t);
        }
    }

    /// Puts tracks right after the current one ("Play next"), shuffled or not. They also go
    /// after it in list order, so turning shuffle off keeps them next. An empty queue just
    /// gets them, from the first.
    pub fn insert_next(&mut self, tracks: impl IntoIterator<Item = Track>) {
        let at = self.current_index().map_or(self.tracks.len(), |c| c + 1);
        let before = self.tracks.len();
        self.tracks.splice(at..at, tracks);
        let n = self.tracks.len() - before;
        for i in &mut self.order {
            if *i >= at {
                *i += n;
            }
        }
        let pos = if self.order.is_empty() {
            self.pos = 0;
            0
        } else {
            self.pos + 1
        };
        self.order.splice(pos..pos, at..at + n);
    }

    /// Takes `tracks[index]` out of the queue, keeping the current track current.
    pub fn remove(&mut self, index: usize) {
        if index >= self.tracks.len() {
            return;
        }
        let cur = self.current_index();
        self.tracks.remove(index);
        self.order.retain(|i| *i != index);
        for i in &mut self.order {
            if *i > index {
                *i -= 1;
            }
        }
        let cur = cur.filter(|c| *c != index).map(|c| if c > index { c - 1 } else { c });
        self.pos = cur
            .and_then(|c| self.order.iter().position(|i| *i == c))
            .unwrap_or(self.pos.min(self.order.len().saturating_sub(1)));
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.order.clear();
        self.pos = 0;
        self.playlist = None;
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Index into `tracks` of what's playing.
    pub fn current_index(&self) -> Option<usize> {
        self.order.get(self.pos).copied()
    }

    pub fn current(&self) -> Option<&Track> {
        self.current_index().map(|i| &self.tracks[i])
    }

    /// Tracks in play order, with their index into `tracks`.
    pub fn in_order(&self) -> impl Iterator<Item = (usize, &Track)> {
        self.order.iter().map(|i| (*i, &self.tracks[*i]))
    }

    /// Jumps to `tracks[index]`.
    pub fn jump(&mut self, index: usize) -> Option<&Track> {
        self.pos = self.order.iter().position(|i| *i == index)?;
        self.current()
    }

    pub fn has_next(&self) -> bool {
        self.pos + 1 < self.order.len() || self.repeat == Repeat::All && !self.order.is_empty()
    }

    pub fn has_prev(&self) -> bool {
        self.pos > 0 || self.repeat == Repeat::All && !self.order.is_empty()
    }

    /// Skips forward (the Next button): repeat-one doesn't hold it back.
    pub fn next(&mut self) -> Option<&Track> {
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
        } else if self.repeat == Repeat::All && !self.order.is_empty() {
            self.pos = 0;
        } else {
            return None;
        }
        self.current()
    }

    pub fn prev(&mut self) -> Option<&Track> {
        if self.pos > 0 {
            self.pos -= 1;
        } else if self.repeat == Repeat::All && !self.order.is_empty() {
            self.pos = self.order.len() - 1;
        } else {
            return None;
        }
        self.current()
    }

    /// What [`after_end`](Self::after_end) would move to, without moving.
    pub fn peek_after_end(&self) -> Option<&Track> {
        let pos = match self.repeat {
            Repeat::One => self.pos,
            _ if self.pos + 1 < self.order.len() => self.pos + 1,
            Repeat::All => 0,
            Repeat::Off => return None,
        };
        self.order.get(pos).map(|i| &self.tracks[*i])
    }

    /// What to play once the current track ends on its own, if anything.
    pub fn after_end(&mut self) -> Option<&Track> {
        if self.repeat == Repeat::One {
            return self.current();
        }
        self.next()
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            self.reshuffle();
        } else {
            // Back to list order, still on the same track.
            let cur = self.current_index();
            self.order = (0..self.tracks.len()).collect();
            self.pos = cur.unwrap_or(0);
        }
    }

    /// Random order with the current track first, so shuffling doesn't skip it.
    fn reshuffle(&mut self) {
        let cur = self.current_index();
        if self.seed == 0 {
            self.seed = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(1) as u64 | 1;
        }
        let mut rest: Vec<usize> = (0..self.tracks.len()).filter(|i| Some(*i) != cur).collect();
        // Fisher-Yates with xorshift; good enough to mix a playlist.
        for i in (1..rest.len()).rev() {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            rest.swap(i, (self.seed % (i as u64 + 1)) as usize);
        }
        self.order = cur.into_iter().chain(rest).collect();
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobManager, JobSpec, JobStatus};
    use crate::tools::ytdlp::{DownloadRequest, PlaylistItem};

    fn link(n: u32) -> Track {
        Track::new(Source::Link(format!("https://x/{n}")), format!("T{n}"))
    }

    #[test]
    fn playlists_round_trip_and_edit() {
        let mut lib = Library::default();
        let id = lib.create("  Mix ");
        let blank = lib.create("");
        assert_eq!(lib.get(blank).unwrap().name, "Playlist 2");
        lib.add(id, [link(1), link(2), Track::new(Source::Local("C:/m/a.flac".into()), "A".into())]);
        lib.move_track(id, 2, 0);
        lib.remove_track(id, 1);
        lib.rename(id, "Road trip");
        lib.rename(id, "   ");
        assert!(lib.dirty);

        let path = std::env::temp_dir().join(format!("redline-playlists-{}.json", std::process::id()));
        lib.save_to(&path);
        let back = Library::load_from(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.playlists, lib.playlists);
        let p = back.get(id).unwrap();
        assert_eq!(p.name, "Road trip");
        let titles: Vec<_> = p.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["A", "T2"]);
        assert_eq!(p.tracks[0].source, Source::Local("C:/m/a.flac".into()));
        assert!(!back.is_unreadable());
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(Library::load_from(&path).is_unreadable(), "a broken file isn't an empty library");
        let _ = std::fs::remove_file(&path);
        assert!(!Library::load_from(&path).is_unreadable(), "no file yet is");

        // Older files without the optional fields still load.
        let old: Vec<Playlist> =
            serde_json::from_str(r#"[{"id":3,"name":"x","tracks":[{"source":{"Link":"https://y"},"title":"y"}]}]"#)
                .unwrap();
        assert_eq!(old[0].tracks[0].artist, None);
    }

    #[test]
    fn replacing_a_link_keeps_the_rest() {
        let mut lib = Library::default();
        let id = lib.create("Mix");
        let mut dead = link(2);
        dead.artist = Some("Band".into());
        dead.duration = Some(200.0);
        dead.thumbnail = Some("old.jpg".into());
        dead.added_at = 42;
        lib.add(id, [link(1), dead, link(3)]);
        lib.dirty = false;
        let c = linkcheck::Candidate {
            url: "https://x/new".into(),
            title: "T2 (Official Video)".into(),
            duration: Some(201.0),
            ..Default::default()
        };
        assert!(lib.replace_link(id, 1, &c));
        assert!(lib.dirty);
        assert!(!lib.replace_link(id, 9, &c));
        let t = &lib.get(id).unwrap().tracks[1];
        assert_eq!(t.source, Source::Link("https://x/new".into()));
        assert_eq!((t.title.as_str(), t.artist.as_deref(), t.added_at), ("T2", Some("Band"), 42));
        assert_eq!((t.duration, t.thumbnail.as_deref()), (Some(201.0), Some("old.jpg")));
        assert_eq!(t.replaced_from.as_deref(), Some("https://x/2"));

        // Replacing again keeps the first link.
        let again = linkcheck::Candidate { url: "https://x/newer".into(), ..Default::default() };
        lib.replace_link(id, 1, &again);
        let t = &lib.get(id).unwrap().tracks[1];
        assert_eq!(t.source, Source::Link("https://x/newer".into()));
        assert_eq!(t.replaced_from.as_deref(), Some("https://x/2"));
        assert_eq!(t.duration, Some(201.0));

        let mut file = Track::new(Source::Local("C:/m/a.flac".into()), "a".into());
        file.swap_link(&c);
        assert_eq!((&file.source, &file.replaced_from), (&Source::Local("C:/m/a.flac".into()), &None));

        let json = serde_json::to_string(&lib.get(id).unwrap().tracks).unwrap();
        assert_eq!(json.matches("replaced_from").count(), 1, "only on replaced tracks");
        let old: Track = serde_json::from_str(r#"{"source":{"Link":"https://y"},"title":"y"}"#).unwrap();
        assert_eq!(old.replaced_from, None);
    }

    #[test]
    fn tracks_know_their_platform_and_key() {
        let a = Track::new(Source::Link("https://youtu.be/abc?t=3".into()), "a".into());
        let b = Track::new(Source::Link("https://music.youtube.com/watch?v=abc".into()), "b".into());
        assert_eq!(a.platform(), Some(Platform::YouTube));
        assert_eq!(a.key(), b.key());
        let local = Track::new(Source::Local("C:/m/a.flac".into()), "a".into());
        assert_eq!(local.platform(), None);
        assert_eq!(local.key(), "C:/m/a.flac");
    }

    #[test]
    fn expands_playlists_into_tracks() {
        let info = MediaInfo {
            title: "Mix".into(),
            is_playlist: true,
            items: vec![
                PlaylistItem {
                    url: "https://youtu.be/a".into(),
                    title: "A".into(),
                    uploader: Some("Band".into()),
                    duration: Some(60.0),
                    ..Default::default()
                },
                PlaylistItem {
                    url: "https://youtu.be/b".into(),
                    title: "B".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let tracks = tracks_from_info("https://youtube.com/playlist?list=x", &info);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].source, Source::Link("https://youtu.be/a".into()));
        assert_eq!(tracks[0].artist.as_deref(), Some("Band"));

        let single = MediaInfo {
            title: "Video title".into(),
            track: Some("Song".into()),
            uploader: Some("Chan".into()),
            ..Default::default()
        };
        let t = tracks_from_info(" https://youtu.be/c ", &single);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].title, "Song");
        assert_eq!(t[0].source, Source::Link("https://youtu.be/c".into()));
    }

    #[test]
    fn prefers_a_downloaded_copy() {
        let dir = std::env::temp_dir().join(format!("redline-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("song.mp3");
        std::fs::write(&file, b"").unwrap();

        let track = link(1);
        let mut history = History::default();
        assert_eq!(playable_source(&track, &history), track.source, "not downloaded: stream");

        let mut jobs = JobManager::new();
        jobs.push(
            "T1".into(),
            String::new(),
            JobSpec::Download(DownloadRequest {
                url: "https://x/1".into(),
                ..Default::default()
            }),
        );
        jobs.jobs[0].status = JobStatus::Done(Some(file.clone()));
        history.record(&jobs.jobs[0]);
        assert_eq!(playable_source(&track, &history), Source::Local(file.clone()));
        let variant = Track::new(Source::Link("https://x/1/?utm=y".into()), "T1".into());
        assert_eq!(playable_source(&variant, &history), Source::Local(file.clone()), "URL variant");

        std::fs::remove_file(&file).unwrap();
        assert_eq!(playable_source(&track, &history), track.source, "file gone: stream again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn offline_copy_is_an_audio_download() {
        let settings = settings::Settings::default();
        let job = offline_download(&link(7), &settings).unwrap();
        assert_eq!(job.title, "T7");
        let JobSpec::Download(r) = job.spec else { panic!() };
        assert_eq!(r.url, "https://x/7");
        assert_eq!(r.options.mode, Mode::Audio);
        assert!(!r.options.playlist);
        assert!(offline_download(&Track::new(Source::Local("a.mp3".into()), "a".into()), &settings).is_none());
    }

    /// Opt-in: needs the real ffmpeg (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn pulls_out_embedded_cover_art() {
        let tools = crate::tools::e2e::ensure_tools();
        let dir = crate::tools::e2e::scratch("cover");
        let (art, song, bare) = (dir.join("art.png"), dir.join("song.mp3"), dir.join("bare.mp3"));
        let ffmpeg = tools.ffmpeg.clone().unwrap();
        let run = |args: &[&str]| {
            let ok = crate::tools::command(&ffmpeg).args(["-y", "-v", "error"]).args(args).status().unwrap();
            assert!(ok.success(), "{args:?}");
        };
        run(&["-f", "lavfi", "-i", "color=red:s=64x64", "-frames:v", "1", art.to_str().unwrap()]);
        run(&["-f", "lavfi", "-i", "sine=d=1", bare.to_str().unwrap()]);
        run(&[
            "-i", bare.to_str().unwrap(), "-i", art.to_str().unwrap(), "-map", "0", "-map", "1", "-c:a", "copy",
            "-disposition:v", "attached_pic", song.to_str().unwrap(),
        ]);
        std::fs::remove_file(&art).unwrap();

        let t = track_for_file(&tools, &song);
        let cover = t.thumbnail.as_deref().and_then(|u| u.strip_prefix("file://")).expect("a cover");
        assert!(Path::new(cover).starts_with(covers_dir()));
        let img = image::open(cover).unwrap();
        assert_eq!((img.width(), img.height()), (64, 64));
        assert_eq!(track_for_file(&tools, &bare).thumbnail, None, "no art, no cover");
        forget_cover(&t);
        assert!(!Path::new(cover).exists());

        std::fs::write(dir.join("folder.jpg"), b"").unwrap();
        let beside = track_for_file(&tools, &bare).thumbnail.unwrap();
        assert!(beside.ends_with("folder.jpg"), "{beside}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_media_files_in_folders() {
        let dir = std::env::temp_dir().join(format!("redline-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("b/deeper")).unwrap();
        for f in ["b/deeper/z.flac", "b/a.mp3", "b/cover.jpg", "c.mkv", "notes.txt"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let files = media_files(std::slice::from_ref(&dir));
        let names: Vec<_> = files
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(names, ["b/a.mp3", "b/deeper/z.flac", "c.mkv"]);
        let tracks = scan(&ToolPaths::default(), &files[..1]);
        assert_eq!(tracks[0].title, "a");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queue_moves_with_repeat_and_shuffle() {
        let mut q = NowPlaying::default();
        q.start((1..=3).map(link).collect(), 1, None);
        assert_eq!(q.current().unwrap().title, "T2");
        assert_eq!(q.next().unwrap().title, "T3");
        assert!(!q.has_next());
        assert_eq!(q.after_end(), None, "repeat off stops at the end");

        assert_eq!(q.peek_after_end(), None);
        q.repeat = Repeat::One;
        assert_eq!(q.peek_after_end().unwrap().title, "T3");
        assert_eq!(q.after_end().unwrap().title, "T3", "repeat one plays it again");
        assert_eq!(q.next(), None, "Next still moves on, and there's nothing after");
        q.repeat = Repeat::All;
        assert_eq!(q.peek_after_end().unwrap().title, "T1");
        assert_eq!(q.after_end().unwrap().title, "T1", "repeat all wraps");
        assert_eq!(q.prev().unwrap().title, "T3");

        q.set_shuffle(true);
        assert_eq!(q.current().unwrap().title, "T3", "shuffle keeps the current track");
        let mut seen: Vec<_> = q.in_order().map(|(_, t)| t.title.clone()).collect();
        seen.sort();
        assert_eq!(seen, ["T1", "T2", "T3"]);
        q.set_shuffle(false);
        assert_eq!(q.current().unwrap().title, "T3");
        assert_eq!(q.in_order().map(|(i, _)| i).collect::<Vec<_>>(), [0, 1, 2]);

        q.append([link(4)]);
        assert_eq!(q.jump(3).unwrap().title, "T4");
        q.remove(0);
        assert_eq!(q.current().unwrap().title, "T4", "removing another track keeps this one");
        assert_eq!(q.in_order().map(|(_, t)| t.title.as_str()).collect::<Vec<_>>(), ["T2", "T3", "T4"]);
        q.clear();
        assert!(q.is_empty() && q.current().is_none());
        assert_eq!(Repeat::Off.cycle().cycle().cycle(), Repeat::Off);
    }

    fn titles(q: &NowPlaying) -> Vec<&str> {
        q.in_order().map(|(_, t)| t.title.as_str()).collect()
    }

    #[test]
    fn play_next_goes_after_the_current_track() {
        let mut q = NowPlaying::default();
        q.insert_next([link(1), link(2)]);
        assert_eq!(q.current().unwrap().title, "T1", "an empty queue starts at the top");

        q.start((1..=4).map(link).collect(), 1, None);
        q.insert_next([link(8), link(9)]);
        assert_eq!(q.current().unwrap().title, "T2");
        assert_eq!(titles(&q), ["T1", "T2", "T8", "T9", "T3", "T4"]);
        assert_eq!(q.next().unwrap().title, "T8");
        assert_eq!(q.next().unwrap().title, "T9");
        assert_eq!(q.next().unwrap().title, "T3");

        // Removing and jumping still work by list index afterwards.
        q.remove(2);
        assert_eq!(q.current().unwrap().title, "T3");
        assert_eq!(q.jump(0).unwrap().title, "T1");
        q.insert_next([]);
        assert_eq!(titles(&q), ["T1", "T2", "T9", "T3", "T4"]);
    }

    #[test]
    fn play_next_while_shuffled() {
        let mut q = NowPlaying::default();
        q.set_shuffle(true);
        q.start((1..=6).map(link).collect(), 2, None);
        q.next();
        q.next();
        let cur = q.current().unwrap().title.clone();
        let before = titles(&q).into_iter().map(String::from).collect::<Vec<_>>();
        q.insert_next([link(8), link(9)]);
        assert_eq!(q.current().unwrap().title, cur);
        let mut want = before.clone();
        want.splice(3..3, ["T8".to_string(), "T9".to_string()]);
        assert_eq!(titles(&q), want, "in play order right after the current track");
        assert_eq!(q.next().unwrap().title, "T8");
        assert_eq!(q.next().unwrap().title, "T9");
        assert_eq!(q.next().unwrap().title, want[5]);

        // Every list index is in the order once, and shuffle off keeps them after `cur`.
        let mut order: Vec<_> = q.in_order().map(|(i, _)| i).collect();
        order.sort();
        assert_eq!(order, (0..8).collect::<Vec<_>>());
        q.jump(q.tracks.iter().position(|t| t.title == cur).unwrap());
        q.set_shuffle(false);
        let list = titles(&q);
        let at = list.iter().position(|t| *t == cur).unwrap();
        assert_eq!(list[at + 1..at + 3], ["T8", "T9"]);
    }
}
