//! The Local library: the songs in the folders the user picked (their Music folder until they
//! pick others). Each file's tags are remembered by its modified time, so scanning again only
//! runs ffprobe on files that are new or changed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::library::{Track, forget_cover, media_files, track_for_file};
use crate::player::Source;
use crate::settings;
use crate::tools::ToolPaths;
use crate::tools::convert::{Category, classify};

/// How many files are probed at once on a scan.
const PROBES_AT_ONCE: usize = 4;

#[derive(Serialize, Deserialize)]
struct Cached {
    /// Modified time, Unix seconds.
    modified: i64,
    track: Track,
    /// Cached before covers were looked for; such entries are probed again once.
    #[serde(default)]
    covers: bool,
}

fn cache_path() -> PathBuf {
    settings::data_dir().join("local.json")
}

/// The user's Music folder, where Local looks until other folders are picked.
#[cfg(not(target_os = "android"))]
pub fn default_folders() -> Vec<PathBuf> {
    directories::UserDirs::new()
        .and_then(|u| u.audio_dir().map(Path::to_path_buf))
        .into_iter()
        .collect()
}

/// Android picks its folders itself.
#[cfg(target_os = "android")]
pub fn default_folders() -> Vec<PathBuf> {
    Vec::new()
}

/// Audio files under `folders`, searched all the way down, then the single `files` added on
/// their own. A folder inside another one picked, or an added file already in a folder,
/// doesn't list its songs twice.
pub fn audio_files(folders: &[PathBuf], files: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    media_files(folders)
        .into_iter()
        .chain(files.iter().filter(|p| p.is_file()).cloned())
        .filter(|p| is_audio(p))
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Whether Local takes this file: songs only, not videos or pictures.
pub fn is_audio(path: &Path) -> bool {
    classify(path) == Some(Category::Audio)
}

fn modified(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// The tracks in `folders` and the added `files`, titled from their tags. Slow the first time
/// for a big library; run it off the UI thread.
pub fn scan(tools: &ToolPaths, folders: &[PathBuf], files: &[PathBuf]) -> Vec<Track> {
    scan_with(tools, folders, files, &cache_path())
}

fn scan_with(tools: &ToolPaths, folders: &[PathBuf], added: &[PathBuf], cache: &Path) -> Vec<Track> {
    let mut known: HashMap<PathBuf, Cached> = std::fs::read_to_string(cache)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Cached>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| match &c.track.source {
            Source::Local(p) => Some((p.clone(), c)),
            Source::Link(_) => None,
        })
        .collect();

    let files: Vec<(PathBuf, i64)> = audio_files(folders, added)
        .into_iter()
        .map(|p| {
            let m = modified(&p);
            (p, m)
        })
        .collect();
    let mut found: Vec<Option<Cached>> = files
        .iter()
        .map(|(p, m)| known.remove(p).filter(|c| c.modified == *m && c.covers))
        .collect();

    // Probe the rest a few at a time.
    let missing: Vec<usize> = (0..files.len()).filter(|i| found[*i].is_none()).collect();
    let chunk = missing.len().div_ceil(PROBES_AT_ONCE).max(1);
    let probed: Vec<(usize, Cached)> = std::thread::scope(|s| {
        let workers: Vec<_> = missing
            .chunks(chunk)
            .map(|ids| {
                let files = &files;
                s.spawn(move || {
                    ids.iter()
                        .map(|i| {
                            let (path, modified) = &files[*i];
                            (*i, Cached { modified: *modified, track: track_for_file(tools, path), covers: true })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    });
    for (i, c) in probed {
        found[i] = Some(c);
    }

    let found: Vec<Cached> = found.into_iter().flatten().collect();
    // Songs that are gone take their extracted covers with them.
    for (path, c) in &known {
        if !path.exists() {
            forget_cover(&c.track);
        }
    }
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(&found) {
        let _ = std::fs::write(cache, json);
    }
    found.into_iter().map(|c| c.track).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_songs_once_and_remembers_them() {
        let dir = std::env::temp_dir().join(format!("redline-localscan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Album")).unwrap();
        for f in ["Album/1 intro.flac", "Album/cover.jpg", "b.mp3", "clip.mp4", "notes.txt"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let both = [dir.clone(), dir.join("Album")];
        let names = |tracks: &[Track]| tracks.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        assert_eq!(audio_files(&both, &[]).len(), 2, "no videos, pictures or doubles");

        let cache = dir.join("cache.json");
        let first = scan_with(&ToolPaths::default(), &both, &[], &cache);
        assert_eq!(names(&first), ["1 intro", "b"]);
        let mut renamed: Vec<Cached> = serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
        renamed[1].track.title = "From the cache".into();
        std::fs::write(&cache, serde_json::to_string(&renamed).unwrap()).unwrap();
        let again = scan_with(&ToolPaths::default(), &both, &[], &cache);
        assert_eq!(names(&again), ["1 intro", "From the cache"], "unchanged files aren't probed again");

        renamed[1].modified -= 10;
        std::fs::write(&cache, serde_json::to_string(&renamed).unwrap()).unwrap();
        let changed = scan_with(&ToolPaths::default(), &both, &[], &cache);
        assert_eq!(names(&changed), ["1 intro", "b"], "a changed file is");
        std::fs::remove_file(dir.join("b.mp3")).unwrap();
        assert_eq!(names(&scan_with(&ToolPaths::default(), &both, &[], &cache)), ["1 intro"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_added_songs_next_to_the_folders() {
        let dir = std::env::temp_dir().join(format!("redline-localadded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Music")).unwrap();
        std::fs::create_dir_all(dir.join("Elsewhere")).unwrap();
        for f in ["Music/a.mp3", "Elsewhere/b.ogg", "Elsewhere/clip.mp4"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let folders = [dir.join("Music")];
        let added = [
            dir.join("Elsewhere/b.ogg"),
            dir.join("Music/a.mp3"),
            dir.join("Elsewhere/clip.mp4"),
            dir.join("Elsewhere/gone.mp3"),
        ];
        let tracks = scan_with(&ToolPaths::default(), &folders, &added, &dir.join("cache.json"));
        let names: Vec<_> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(names, ["a", "b"], "added songs once, no videos, nothing missing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
