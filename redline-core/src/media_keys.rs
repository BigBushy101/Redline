//! Windows media keys and the media overlay (SMTC), through souvlaki.
//!
//! Play and pause go straight to the player from the system's callback thread, so they work
//! whatever the window is doing. Keys that need the playlist come back to the app.

use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crate::Wake;
use crate::player::{Command, PlayerRemote, PlayerState, Status};

/// A media key the app has to handle itself.
#[derive(Debug, PartialEq)]
pub enum MediaKey {
    Next,
    Previous,
    /// Seek by this many seconds (negative is back).
    SeekBy(f64),
    /// Bring the window to the front.
    Raise,
}

/// What was last shown in the overlay, so it's only updated on change.
#[derive(PartialEq, Clone)]
struct Shown {
    load_id: u64,
    title: String,
    artist: String,
    cover: Option<String>,
    duration: Option<u64>,
}

pub struct MediaKeys {
    controls: souvlaki::MediaControls,
    keys: Receiver<MediaKey>,
    shown: Option<Shown>,
    playback: Option<(bool, bool)>,
    /// Position last told to the overlay, and when.
    told: (f64, Instant),
}

/// Seconds the overlay's seek buttons jump.
const SEEK_STEP: f64 = 10.0;

impl MediaKeys {
    /// Hooks up the media keys for the window `hwnd`.
    pub fn new(hwnd: *mut std::ffi::c_void, player: PlayerRemote, wake: Wake) -> anyhow::Result<Self> {
        use souvlaki::{MediaControlEvent as E, MediaControls, PlatformConfig, SeekDirection};

        let mut controls = MediaControls::new(PlatformConfig {
            display_name: "Redline",
            dbus_name: "redline",
            hwnd: Some(hwnd),
        })
        .map_err(|e| anyhow::anyhow!("media controls: {e:?}"))?;
        let (tx, keys) = channel();
        controls
            .attach(move |event| {
                let forward = |k: MediaKey| {
                    let _ = tx.send(k);
                    wake();
                };
                let secs = |d: SeekDirection, by: Duration| match d {
                    SeekDirection::Forward => by.as_secs_f64(),
                    SeekDirection::Backward => -by.as_secs_f64(),
                };
                match event {
                    E::Play => player.send(Command::Play),
                    E::Pause | E::Stop => player.send(Command::Pause),
                    E::Toggle => player.send(Command::Toggle),
                    E::SetPosition(p) => player.send(Command::Seek(p.0.as_secs_f64())),
                    E::Next => forward(MediaKey::Next),
                    E::Previous => forward(MediaKey::Previous),
                    E::Seek(d) => forward(MediaKey::SeekBy(secs(d, Duration::from_secs_f64(SEEK_STEP)))),
                    E::SeekBy(d, by) => forward(MediaKey::SeekBy(secs(d, by))),
                    E::Raise => forward(MediaKey::Raise),
                    _ => {}
                }
            })
            .map_err(|e| anyhow::anyhow!("media controls: {e:?}"))?;
        Ok(Self {
            controls,
            keys,
            shown: None,
            playback: None,
            told: (0.0, Instant::now()),
        })
    }

    /// Keys pressed since the last call.
    pub fn take_keys(&self) -> Vec<MediaKey> {
        self.keys.try_iter().collect()
    }

    /// Shows the player's track and state in the overlay, when they change.
    pub fn update(&mut self, st: &PlayerState) {
        let track = st.track.clone().unwrap_or_default();
        let shown = Shown {
            load_id: st.load_id,
            title: track.title,
            artist: track.artist.unwrap_or_default(),
            cover: track.thumbnail.filter(|t| t.starts_with("http")),
            duration: st.duration.map(|d| d as u64),
        };
        if st.is_active() && self.shown.as_ref() != Some(&shown) {
            self.set_metadata(&shown, st.duration);
            self.shown = Some(shown);
        }

        let playing = st.is_playing();
        let state = (st.is_active(), playing);
        // Tell the overlay where we are when that changes in a way it can't work out: a
        // pause, play, seek, or drift. Otherwise it keeps time by itself.
        let (pos, at) = self.told;
        let expected = if playing { pos + at.elapsed().as_secs_f64() } else { pos };
        let jumped = (st.position - expected).abs() > 3.0;
        if self.playback != Some(state) || jumped {
            self.set_playback(st);
            self.playback = Some(state);
            self.told = (st.position, Instant::now());
        }
    }

    fn set_metadata(&mut self, s: &Shown, duration: Option<f64>) {
        let _ = self.controls.set_metadata(souvlaki::MediaMetadata {
            title: Some(&s.title),
            // Empty rather than None, so the last track's artist doesn't linger.
            artist: Some(&s.artist),
            album: None,
            cover_url: s.cover.as_deref(),
            duration: duration.map(Duration::from_secs_f64),
        });
    }

    fn set_playback(&mut self, st: &PlayerState) {
        use souvlaki::{MediaPlayback, MediaPosition};
        let progress = Some(MediaPosition(Duration::from_secs_f64(st.position.max(0.0))));
        let playback = match &st.status {
            _ if !st.is_active() => MediaPlayback::Stopped,
            Status::Playing | Status::Loading => MediaPlayback::Playing { progress },
            _ => MediaPlayback::Paused { progress },
        };
        let _ = self.controls.set_playback(playback);
    }
}
