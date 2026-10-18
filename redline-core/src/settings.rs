use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::player::PlayerSettings;
use crate::presets::{self, ConvertPreset, DownloadPreset};
use crate::tools::convert::ConvertOptions;
use crate::tools::ytdlp::{DownloadOptions, NetworkOptions};

/// Redline's folders, for hosts where the platform lookup doesn't apply (Android has no
/// user dirs to look up; the app hands over its own storage). Desktop never sets them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDirs {
    /// Settings, history, the queue and the tools' bin folder; used as is, without a
    /// `Redline` subfolder.
    pub data: PathBuf,
    /// Where downloads go until the user picks another folder.
    pub downloads: PathBuf,
}

static APP_DIRS: OnceLock<AppDirs> = OnceLock::new();

/// Overrides [`data_dir`] and the default download folder for the rest of the process. Call it
/// before anything reads settings. Setting the same folders again is fine (Android re-runs init
/// when an activity is recreated in the same process); different ones fail rather than moving
/// Redline's files mid-run.
pub fn set_app_dirs(dirs: AppDirs) -> Result<()> {
    let set = APP_DIRS.get_or_init(|| dirs.clone());
    ensure!(*set == dirs, "Redline's folders are already set to {}", set.data.display());
    Ok(())
}

pub fn data_dir() -> PathBuf {
    resolve_data_dir(APP_DIRS.get())
}

fn resolve_data_dir(dirs: Option<&AppDirs>) -> PathBuf {
    dirs.map_or_else(platform_data_dir, |d| d.data.clone())
}

/// `%APPDATA%\Redline` on Windows (the platform's roaming data dir elsewhere).
#[cfg(not(target_os = "android"))]
fn platform_data_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("Redline")
}

/// Only reached if the app didn't call [`set_app_dirs`]; nothing here survives a reboot.
#[cfg(target_os = "android")]
fn platform_data_dir() -> PathBuf {
    std::env::temp_dir().join("Redline")
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

fn default_download_dir() -> PathBuf {
    resolve_download_dir(APP_DIRS.get())
}

fn resolve_download_dir(dirs: Option<&AppDirs>) -> PathBuf {
    dirs.map_or_else(platform_download_dir, |d| d.downloads.clone())
}

#[cfg(not(target_os = "android"))]
fn platform_download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Redline")
}

#[cfg(target_os = "android")]
fn platform_download_dir() -> PathBuf {
    platform_data_dir().join("Downloads")
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub download_dir: PathBuf,
    /// `None` writes converted files next to their source.
    pub convert_dir: Option<PathBuf>,
    pub ytdlp_path: Option<PathBuf>,
    pub ffmpeg_path: Option<PathBuf>,
    pub ffprobe_path: Option<PathBuf>,
    pub deno_path: Option<PathBuf>,
    pub concurrency: usize,
    /// Skip links that were downloaded before (yt-dlp's download archive).
    pub skip_downloaded: bool,
    pub network: NetworkOptions,
    pub download: DownloadOptions,
    pub convert: ConvertOptions,
    /// Converted file name template; see `convert::render_stem`.
    pub rename_template: String,
    /// Send originals to the Recycle Bin after a successful conversion.
    pub recycle_originals: bool,
    pub download_presets: Vec<DownloadPreset>,
    pub convert_presets: Vec<ConvertPreset>,
    /// Desktop notification when jobs finish while Redline isn't focused.
    pub notify: bool,
    pub player: PlayerSettings,
    /// Refresh saved collections not synced in the last 12 hours when Redline starts.
    pub refresh_saved_on_start: bool,
    /// Folders the Local library lists songs from.
    pub local_folders: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            convert_dir: None,
            ytdlp_path: None,
            ffmpeg_path: None,
            ffprobe_path: None,
            deno_path: None,
            concurrency: 2,
            skip_downloaded: true,
            network: NetworkOptions::default(),
            download: DownloadOptions::default(),
            convert: ConvertOptions::default(),
            rename_template: "{name}".into(),
            recycle_originals: false,
            download_presets: presets::default_download_presets(),
            convert_presets: presets::default_convert_presets(),
            notify: true,
            player: PlayerSettings::default(),
            refresh_saved_on_start: true,
            local_folders: crate::local::default_folders(),
        }
    }
}

/// yt-dlp download archive shared by every download.
pub fn archive_path() -> PathBuf {
    data_dir().join("archive.txt")
}

impl Settings {
    pub fn load() -> Self {
        fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The process-wide override is never set in tests (it would leak into every other test),
    // so these exercise the resolution it goes through.
    #[test]
    fn app_dirs_take_precedence() {
        let dirs = AppDirs {
            data: PathBuf::from("/data/user/0/app/files"),
            downloads: PathBuf::from("/storage/emulated/0/Download/Redline"),
        };
        assert_eq!(resolve_data_dir(Some(&dirs)), dirs.data);
        assert_eq!(resolve_download_dir(Some(&dirs)), dirs.downloads);
        assert_eq!(resolve_data_dir(None), platform_data_dir());
        assert_eq!(resolve_download_dir(None), platform_download_dir());
        assert!(platform_data_dir().ends_with("Redline"));
    }
}
