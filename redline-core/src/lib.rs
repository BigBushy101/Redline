//! Everything Redline does apart from drawing it: the external tools, the job queue, history,
//! settings and OS integration. No UI dependency, so it can be driven and tested headless.

#[cfg(any(target_os = "android", test))]
pub mod android;
pub mod backup;
pub mod convert_plan;
pub mod download_plan;
pub mod format;
pub mod history;
pub mod jobs;
pub mod library;
pub mod linkcheck;
pub mod local;
#[cfg(windows)]
pub mod media_keys;
pub mod platform;
pub mod player;
pub mod presets;
pub mod queue_watch;
pub mod saved;
pub mod settings;
pub mod system;
pub mod tools;
pub mod waveform;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Writes `bytes` beside `path` and then renames the copy over it, so a write cut short (the
/// app killed, the disk full) leaves the old file whole instead of an empty one.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// Moves a file that couldn't be read out of the way, to `<name>.<unix time>.bad`, so the next
/// save can't overwrite what's in it.
pub(crate) fn keep_aside(path: &Path) {
    let mut bad = path.as_os_str().to_owned();
    bad.push(format!(".{}.bad", chrono::Utc::now().timestamp()));
    let _ = std::fs::rename(path, PathBuf::from(bad));
}

/// Asks the UI to redraw. Background work calls it after changing state the UI shows; the app
/// sets it once to its repaint request.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Removes the copies [`super::keep_aside`] made of `path`, returning what they held.
    pub(crate) fn take_aside(path: &Path) -> Vec<Vec<u8>> {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut held = Vec::new();
        for entry in std::fs::read_dir(path.parent().unwrap()).unwrap().flatten() {
            let n = entry.file_name().to_string_lossy().into_owned();
            if n.starts_with(&format!("{name}.")) && n.ends_with(".bad") {
                held.push(std::fs::read(entry.path()).unwrap());
                std::fs::remove_file(entry.path()).unwrap();
            }
        }
        held
    }
}

/// A [`Wake`] that does nothing, for headless use and tests.
pub fn no_wake() -> Wake {
    Arc::new(|| {})
}
