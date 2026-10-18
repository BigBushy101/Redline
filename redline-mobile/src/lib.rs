//! redline-core for the Android app. The Compose UI drives an [`Engine`] through UniFFI's
//! generated Kotlin (package `app.redline.core`): blocking calls for lookups and updates,
//! snapshots for the queue and history, and an [`EngineListener`] nudge when they change.
//!
//! This slice covers downloading and the queue. Conversion, the library, the player and saved
//! music follow as more `Engine` methods and `*Record` types in the same style.

uniffi::setup_scaffolding!();

mod engine;
mod error;
mod format;
mod records;

pub use engine::{Engine, EngineListener};
pub use error::RedlineError;
pub use format::*;
pub use records::*;
