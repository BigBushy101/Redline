//! The one error type Kotlin sees. Flat, so each variant arrives as an exception whose
//! `message` is ready to show.

use std::fmt;

#[derive(Debug, Clone, PartialEq, uniffi::Error)]
#[uniffi(flat_error)]
pub enum RedlineError {
    /// yt-dlp or ffmpeg isn't there.
    ToolsMissing(String),
    /// The request can't be done as given (no link, a bad trim, folders already set).
    Invalid(String),
    /// A tool or the filesystem failed; already worded for the user (with core's
    /// `friendly_error` hints where the desktop shows them).
    Failed(String),
}

impl fmt::Display for RedlineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Self::ToolsMissing(m) | Self::Invalid(m) | Self::Failed(m)) = self;
        f.write_str(m)
    }
}

impl std::error::Error for RedlineError {}

impl RedlineError {
    /// A tool or I/O failure, with its context chain as the desktop prints it.
    pub fn failed(e: anyhow::Error) -> Self {
        Self::Failed(format!("{e:#}"))
    }
}
