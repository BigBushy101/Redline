//! Finished jobs, kept across sessions so they can be found, opened or run again.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::jobs::{Job, JobSpec, JobStatus};
use crate::settings;

const MAX_ENTRIES: usize = 2000;

#[derive(Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub title: String,
    pub subtitle: String,
    pub spec: JobSpec,
    pub output: Option<PathBuf>,
    /// Unix seconds.
    pub finished_at: i64,
}

impl HistoryEntry {
    pub fn url(&self) -> Option<&str> {
        match &self.spec {
            JobSpec::Download(r) => Some(&r.url),
            _ => None,
        }
    }

    /// Text the history search matches against.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.title.to_lowercase().contains(&q)
            || self.subtitle.to_lowercase().contains(&q)
            || self.url().is_some_and(|u| u.to_lowercase().contains(&q))
            || match &self.spec {
                JobSpec::Convert(r) => r.input.to_string_lossy().to_lowercase().contains(&q),
                JobSpec::Concat(r) => r
                    .inputs
                    .iter()
                    .any(|i| i.to_string_lossy().to_lowercase().contains(&q)),
                JobSpec::Download(_) => false,
            }
    }
}

#[derive(Default)]
pub struct History {
    /// Newest first.
    pub entries: Vec<HistoryEntry>,
    dirty: bool,
}

fn history_path() -> PathBuf {
    settings::data_dir().join("history.json")
}

impl History {
    pub fn load() -> Self {
        Self::load_from(&history_path())
    }

    fn load_from(path: &Path) -> Self {
        let entries = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            entries,
            dirty: false,
        }
    }

    pub fn save_if_dirty(&mut self) {
        if std::mem::take(&mut self.dirty) {
            self.save_to(&history_path());
        }
    }

    fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(&self.entries) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Records a job that finished successfully; other outcomes are ignored.
    pub fn record(&mut self, job: &Job) {
        let JobStatus::Done(output) = &job.status else {
            return;
        };
        self.entries.insert(
            0,
            HistoryEntry {
                title: job.title.clone(),
                subtitle: job.subtitle.clone(),
                spec: job.spec.clone(),
                output: output.clone(),
                finished_at: chrono::Utc::now().timestamp(),
            },
        );
        self.entries.truncate(MAX_ENTRIES);
        self.dirty = true;
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
            self.dirty = true;
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.dirty = true;
    }

    /// The most recent download of `url`, if any.
    pub fn find_url(&self, url: &str) -> Option<&HistoryEntry> {
        let url = url.trim();
        self.entries.iter().find(|e| e.url() == Some(url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ytdlp::DownloadRequest;

    #[test]
    fn records_saves_and_finds() {
        let mut jobs = crate::jobs::JobManager::new();
        jobs.push(
            "Song".into(),
            "Audio · MP3".into(),
            JobSpec::Download(DownloadRequest {
                url: "https://example.com/a".into(),
                ..Default::default()
            }),
        );
        let mut h = History::default();
        h.record(&jobs.jobs[0]); // still queued: ignored
        assert!(h.entries.is_empty());

        jobs.jobs[0].status = JobStatus::Done(Some(PathBuf::from("a.mp3")));
        h.record(&jobs.jobs[0]);
        assert_eq!(h.entries.len(), 1);
        assert!(h.find_url(" https://example.com/a ").is_some());
        assert!(h.entries[0].matches("song"));
        assert!(h.entries[0].matches("EXAMPLE.com"));
        assert!(!h.entries[0].matches("nope"));

        let path = std::env::temp_dir().join(format!("redline-history-{}.json", std::process::id()));
        h.save_to(&path);
        let back = History::load_from(&path);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].output, Some(PathBuf::from("a.mp3")));
        let _ = std::fs::remove_file(path);
    }
}
