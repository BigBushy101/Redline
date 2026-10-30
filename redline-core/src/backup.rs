//! One file with everything worth keeping: Saved, playlists and settings.

use std::collections::HashSet;

use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::library::{Library, Track};
use crate::saved::{Saved, SavedExport};
use crate::settings::Settings;

const FORMAT: &str = "redline-backup";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct BackupFile {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    pub saved: SavedExport,
    #[serde(default)]
    pub playlists: Vec<BackupPlaylist>,
    pub settings: Settings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupPlaylist {
    pub name: String,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub collections_added: usize,
    pub collections_merged: usize,
    pub tracks_added: usize,
    pub songs_added: usize,
    pub playlists_added: usize,
    pub playlist_tracks_added: usize,
    pub settings_applied: bool,
}

impl Report {
    pub fn message(&self) -> String {
        let mut parts = Vec::new();
        if self.collections_added > 0 {
            parts.push(format!("{} collections", self.collections_added));
        }
        if self.collections_merged > 0 {
            parts.push(format!("{} collections updated", self.collections_merged));
        }
        if self.tracks_added > 0 {
            parts.push(format!("{} songs in collections", self.tracks_added));
        }
        if self.songs_added > 0 {
            parts.push(format!("{} songs", self.songs_added));
        }
        if self.playlists_added > 0 {
            parts.push(format!("{} playlists", self.playlists_added));
        }
        if self.playlist_tracks_added > 0 {
            parts.push(format!("{} playlist tracks", self.playlist_tracks_added));
        }
        if parts.is_empty() {
            if self.settings_applied {
                "Settings restored; the library was already up to date".to_owned()
            } else {
                "Nothing new to restore; it's all there already".to_owned()
            }
        } else {
            let mut message = format!("Restored {}", parts.join(", "));
            if self.settings_applied {
                message.push_str(" and settings");
            }
            message
        }
    }
}

pub fn export(saved: &Saved, library: &Library, settings: &Settings) -> Result<String> {
    let saved: SavedExport = serde_json::from_str(&saved.export_json())
        .map_err(|e| anyhow::anyhow!("couldn't read Saved: {e}"))?;
    let file = BackupFile {
        format: FORMAT.to_owned(),
        version: VERSION,
        exported_at: Utc::now().timestamp(),
        saved,
        playlists: library
            .playlists
            .iter()
            .map(|p| BackupPlaylist {
                name: p.name.clone(),
                tracks: p.tracks.clone(),
            })
            .collect(),
        settings: settings.clone(),
    };
    Ok(serde_json::to_string_pretty(&file)?)
}

pub fn import(text: &str, saved: &mut Saved, library: &mut Library, settings: &mut Settings) -> Result<Report> {
    let text = text.trim_start_matches(char::from_u32(0xFEFF).unwrap());
    let file: BackupFile = serde_json::from_str(text)
        .map_err(|e| anyhow::anyhow!("not a Redline backup: {e}"))?;
    if file.format != FORMAT {
        bail!("That file isn't a Redline backup.");
    }
    if file.version > VERSION {
        bail!("This backup was made by a newer Redline (format {}). Update Redline to restore it.", file.version);
    }

    let merge = saved.merge(file.saved);
    let mut report = Report {
        collections_added: merge.collections_added,
        collections_merged: merge.collections_merged,
        tracks_added: merge.tracks_added,
        songs_added: merge.songs_added,
        ..Default::default()
    };

    for backup in file.playlists {
        let existing = library.playlists.iter().find(|p| p.name == backup.name).map(|p| p.id);
        match existing {
            Some(id) => {
                let known: HashSet<String> = library
                    .playlists
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.tracks.iter().map(Track::key).collect())
                    .unwrap_or_default();
                let mut seen = known;
                let fresh: Vec<Track> = backup
                    .tracks
                    .into_iter()
                    .filter(|t| seen.insert(t.key()))
                    .collect();
                if !fresh.is_empty() {
                    report.playlist_tracks_added += fresh.len();
                    library.add(id, fresh);
                }
            }
            None => {
                let id = library.create(&backup.name);
                let n = backup.tracks.len();
                library.add(id, backup.tracks);
                report.playlists_added += 1;
                report.playlist_tracks_added += n;
            }
        }
    }

    let mut next = file.settings;
    next.download_dir = settings.download_dir.clone();
    next.convert_dir = settings.convert_dir.clone();
    next.ytdlp_path = settings.ytdlp_path.clone();
    next.ffmpeg_path = settings.ffmpeg_path.clone();
    next.ffprobe_path = settings.ffprobe_path.clone();
    next.deno_path = settings.deno_path.clone();
    next.local_folders = settings.local_folders.clone();
    next.local_files = settings.local_files.clone();
    *settings = next;
    report.settings_applied = true;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Track;
    use crate::player::Source;
    use crate::saved::{ExportCollection, SavedTrack};
    use std::path::PathBuf;

    fn track(name: &str) -> Track {
        Track::new(Source::Link(format!("https://example.com/{name}")), name.to_owned())
    }

    #[test]
    fn round_trips_the_library_and_keeps_machine_paths() {
        let mut saved = Saved::default();
        saved.merge(SavedExport {
            format: "redline-saved".into(),
            version: 1,
            exported_at: 0,
            collections: vec![ExportCollection {
                name: "Mix".into(),
                source_url: "https://example.com/playlist".into(),
                keep_offline: false,
                synced_at: None,
                extractor_platform: None,
                tracks: vec![SavedTrack { track: track("a"), removed_upstream: false }],
            }],
            songs: vec![track("b")],
        });
        let mut library = Library::default();
        let id = library.create("Road");
        library.add(id, [track("a")]);
        let mut settings = Settings::default();
        settings.download.max_height = 240;
        settings.download_dir = PathBuf::from("C:/machine/downloads");
        settings.local_folders = vec![PathBuf::from("C:/machine/music")];

        let text = export(&saved, &library, &settings).unwrap();
        let mut saved2 = Saved::default();
        let mut library2 = Library::default();
        let mut settings2 = Settings::default();
        settings2.download_dir = PathBuf::from("D:/current/downloads");
        settings2.local_folders = vec![PathBuf::from("D:/current/music")];
        let report = import(&text, &mut saved2, &mut library2, &mut settings2).unwrap();

        assert_eq!(report.collections_added, 1);
        assert_eq!(report.songs_added, 1);
        assert_eq!(report.playlists_added, 1);
        assert_eq!(library2.playlists[0].tracks.len(), 1);
        assert_eq!(settings2.download.max_height, 240);
        assert_eq!(settings2.download_dir, PathBuf::from("D:/current/downloads"));
        assert_eq!(settings2.local_folders, vec![PathBuf::from("D:/current/music")]);
    }

    #[test]
    fn merges_playlists_by_name_without_duplicating_tracks() {
        let mut saved = Saved::default();
        let mut library = Library::default();
        let id = library.create("Road");
        library.add(id, [track("a")]);
        let mut settings = Settings::default();
        let text = export(&saved, &library, &settings).unwrap();

        let mut saved2 = Saved::default();
        let mut library2 = Library::default();
        let id2 = library2.create("Road");
        library2.add(id2, [track("a")]);
        let report = import(&text, &mut saved2, &mut library2, &mut settings).unwrap();
        assert_eq!(report.playlists_added, 0);
        assert_eq!(report.playlist_tracks_added, 0);
        assert_eq!(library2.playlists[0].tracks.len(), 1);
    }
}
