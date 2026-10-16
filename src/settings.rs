use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::tools::convert::ConvertOptions;
use crate::tools::ytdlp::DownloadOptions;

/// `%APPDATA%\Redline` on Windows (the platform's roaming data dir elsewhere).
pub fn data_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("Redline")
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

fn default_download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Redline")
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
    pub download: DownloadOptions,
    pub convert: ConvertOptions,
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
            download: DownloadOptions::default(),
            convert: ConvertOptions::default(),
        }
    }
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
