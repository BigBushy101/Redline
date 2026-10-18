//! redline-core for the Android app. The Compose UI drives an [`Engine`] through UniFFI's
//! generated Kotlin (package `app.redline.core`): blocking calls for lookups and updates,
//! snapshots for the queue and history, and an [`EngineListener`] nudge when they change.
//!
//! This slice covers downloading, converting, the queue, and the player with its playlists
//! ([`Music`], from `Engine::music`), and Saved music ([`saved`], also on `Music`).

uniffi::setup_scaffolding!();

mod convert;
mod engine;
mod error;
mod format;
mod music;
mod records;
mod saved;

pub use convert::*;
pub use engine::{Engine, EngineListener};
pub use error::RedlineError;
pub use format::*;
pub use music::{Music, MusicListener};
pub use records::*;
pub use saved::*;
