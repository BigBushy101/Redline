//! Everything Redline does apart from drawing it: the external tools, the job queue, history,
//! settings and OS integration. No UI dependency, so it can be driven and tested headless.

pub mod convert_plan;
pub mod download_plan;
pub mod format;
pub mod history;
pub mod jobs;
pub mod library;
#[cfg(windows)]
pub mod media_keys;
pub mod platform;
pub mod player;
pub mod presets;
pub mod queue_watch;
pub mod settings;
pub mod system;
pub mod tools;

use std::sync::Arc;

/// Asks the UI to redraw. Background work calls it after changing state the UI shows; the app
/// sets it once to its repaint request.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A [`Wake`] that does nothing, for headless use and tests.
pub fn no_wake() -> Wake {
    Arc::new(|| {})
}
