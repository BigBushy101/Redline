//! Playlists mixing links and local files, and the "Now playing" queue built from them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::download_plan::{LinkChoices, PlannedDownload};
use crate::history::History;
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
        }
    }

    pub fn is_link(&self) -> bool {
        matches!(self.source, Source::Link(_))
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
}

fn library_path() -> PathBuf {
    settings::data_dir().join("playlists.json")
}

impl Library {
    pub fn load() -> Self {
        Self::load_from(&library_path())
    }

    fn load_from(path: &Path) -> Self {
        let playlists = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            playlists,
            dirty: false,
        }
    }

    pub fn save_if_dirty(&mut self) {
        if std::mem::take(&mut self.dirty) {
            self.save_to(&library_path());
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
    media_files(paths)
        .into_iter()
        .map(|file| {
            let probe = tools.ffprobe.as_deref().and_then(|p| decode::probe(p, &file)).unwrap_or_default();
            let source = Source::Local(file);
            let mut t = Track::new(source.clone(), probe.title.unwrap_or_else(|| source.label()));
            t.artist = probe.artist;
            t.duration = probe.duration;
            t
        })
        .collect()
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

/// What to actually play for a track: a link that's been downloaded plays from the file.
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

        // Older files without the optional fields still load.
        let old: Vec<Playlist> =
            serde_json::from_str(r#"[{"id":3,"name":"x","tracks":[{"source":{"Link":"https://y"},"title":"y"}]}]"#)
                .unwrap();
        assert_eq!(old[0].tracks[0].artist, None);
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
                    thumbnail: None,
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
}
