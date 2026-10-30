//! Backup and restore for Kotlin: one JSON file with Saved, playlists and settings.

use redline_core::backup as core_backup;

use crate::music::Music;
use crate::RedlineError;

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct BackupReportRecord {
    pub message: String,
    pub collections_added: u32,
    pub collections_merged: u32,
    pub tracks_added: u32,
    pub songs_added: u32,
    pub playlists_added: u32,
    pub playlist_tracks_added: u32,
    pub settings_applied: bool,
}

impl From<core_backup::Report> for BackupReportRecord {
    fn from(r: core_backup::Report) -> Self {
        Self {
            message: r.message(),
            collections_added: r.collections_added as u32,
            collections_merged: r.collections_merged as u32,
            tracks_added: r.tracks_added as u32,
            songs_added: r.songs_added as u32,
            playlists_added: r.playlists_added as u32,
            playlist_tracks_added: r.playlist_tracks_added as u32,
            settings_applied: r.settings_applied,
        }
    }
}

#[uniffi::export]
impl Music {
    /// Everything worth keeping, as one JSON string.
    pub fn export_backup(&self) -> Result<String, RedlineError> {
        let settings = self.inner.engine.state().settings.clone();
        let ctl = self.inner.ctl();
        core_backup::export(&ctl.saved, &ctl.library, &settings).map_err(RedlineError::failed)
    }

    /// Merges a backup into Saved, the playlists and the portable settings.
    pub fn import_backup(&self, text: String) -> Result<BackupReportRecord, RedlineError> {
        let mut settings = self.inner.engine.state().settings.clone();
        let report = {
            let mut ctl = self.inner.ctl();
            let ctl = &mut *ctl;
            let report = core_backup::import(&text, &mut ctl.saved, &mut ctl.library, &mut settings)
                .map_err(RedlineError::failed)?;
            ctl.saved_version += 1;
            ctl.saved.save_if_dirty();
            ctl.library.save_if_dirty();
            report
        };
        self.inner.engine.state().settings = settings;
        self.inner.engine.signal.poke();
        self.inner.signal.poke();
        Ok(report.into())
    }
}