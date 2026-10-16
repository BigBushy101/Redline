//! Music player. An engine thread owns the audio output and the decoder; the UI sends it
//! [`Command`]s and reads a [`PlayerState`] snapshot each frame.
//!
//! Decoding is ffmpeg writing raw stereo samples to a pipe. A reader thread copies them into a
//! lock-free ring buffer that the audio callback ([`render::Renderer`]) drains. Seeking starts
//! a fresh ffmpeg at the new offset with its own ring buffer.

pub mod decode;
pub mod output;
pub mod render;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::Wake;
use crate::tools::ToolPaths;
use decode::{Input, Pcm};
use output::{OpenOutput, Output};
use render::{RenderMsg, Renderer, Shared, Stream};

/// What to play.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    Local(PathBuf),
}

impl Source {
    /// A name to show before the real title is known.
    pub fn label(&self) -> String {
        match self {
            Source::Local(p) => p
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
        }
    }
}

/// What's known about the loaded track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackInfo {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    /// Image URL or file path.
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Loading,
    Playing,
    Paused,
    /// Played to the end; a playlist moves on from here.
    Ended,
    Error(String),
}

/// Snapshot of the player for the UI.
#[derive(Clone, Debug)]
pub struct PlayerState {
    pub status: Status,
    pub source: Option<Source>,
    pub track: Option<TrackInfo>,
    /// Seconds into the track.
    pub position: f64,
    pub duration: Option<f64>,
    /// Waiting on the decoder or the network; the output is silent.
    pub buffering: bool,
    pub volume: f32,
    /// Counts loads, so the UI can tell one `Ended` from the next.
    pub load_id: u64,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            source: None,
            track: None,
            position: 0.0,
            duration: None,
            buffering: false,
            volume: 1.0,
            load_id: 0,
        }
    }
}

impl PlayerState {
    /// A track is loaded (or loading), so the player bar shows.
    pub fn is_active(&self) -> bool {
        self.source.is_some()
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.status, Status::Playing | Status::Loading)
    }
}

#[derive(Debug)]
pub enum Command {
    /// Loads `source` and starts playing it `start_at` seconds in.
    Load(Source, f64),
    Play,
    Pause,
    Toggle,
    Seek(f64),
    SetVolume(f32),
    Stop,
    /// The tools moved, e.g. after the installer ran.
    SetTools(ToolPaths),
}

/// Where a source's audio comes from, once it's been looked at.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub input: Input,
    pub info: TrackInfo,
}

/// Turns sources into audio. The real one runs ffprobe and ffmpeg; tests use a fake.
pub trait Media: Send + Sync + 'static {
    /// Works out where to read `source` from, and what it is. May block for a while.
    fn resolve(&self, tools: &ToolPaths, source: &Source) -> Result<Resolved>;
    /// Starts decoding `input` from `start` seconds, as interleaved stereo `f32` at `rate`.
    fn decode(&self, tools: &ToolPaths, input: &Input, start: f64, rate: u32) -> Result<Pcm>;
}

/// The real thing: ffprobe for tags, ffmpeg for audio.
pub struct Ffmpeg;

impl Media for Ffmpeg {
    fn resolve(&self, tools: &ToolPaths, source: &Source) -> Result<Resolved> {
        match source {
            Source::Local(path) => {
                if !path.is_file() {
                    return Err(anyhow!("{} was moved or deleted", path.display()));
                }
                let probe = tools
                    .ffprobe
                    .as_deref()
                    .and_then(|p| decode::probe(p, path))
                    .unwrap_or_default();
                if tools.ffprobe.is_some() && !probe.has_audio && probe.duration.is_some() {
                    return Err(anyhow!("{} has no sound to play", source.label()));
                }
                Ok(Resolved {
                    input: Input {
                        location: path.display().to_string(),
                        headers: None,
                    },
                    info: TrackInfo {
                        title: probe.title.unwrap_or_else(|| source.label()),
                        artist: probe.artist,
                        album: probe.album,
                        duration: probe.duration,
                        thumbnail: None,
                    },
                })
            }
        }
    }

    fn decode(&self, tools: &ToolPaths, input: &Input, start: f64, rate: u32) -> Result<Pcm> {
        decode::ffmpeg(tools, input, start, rate)
    }
}

enum Msg {
    Cmd(Command),
    Resolved(u64, Result<Resolved, String>),
    DecodeEnded(u64, Result<(), String>),
    Shutdown,
}

/// Handle to the engine thread. Dropping it stops playback.
pub struct Player {
    tx: Sender<Msg>,
    state: Arc<Mutex<PlayerState>>,
}

impl Player {
    /// Starts the engine. Nothing opens the audio device until the first load.
    pub fn spawn(tools: ToolPaths, media: Arc<dyn Media>, open_output: OpenOutput, wake: Wake) -> Self {
        let (tx, rx) = channel();
        let state = Arc::new(Mutex::new(PlayerState::default()));
        let (engine_state, engine_tx) = (state.clone(), tx.clone());
        // Built on its own thread: audio streams can't always move between threads.
        thread::Builder::new()
            .name("player".into())
            .spawn(move || {
                Engine {
                    media,
                    tools,
                    open_output,
                    output: None,
                    render_tx: None,
                    rate: 0,
                    shared: Arc::new(Shared::default()),
                    state: engine_state,
                    wake,
                    tx: engine_tx,
                    load_id: 0,
                    current: None,
                    next_stream: 1,
                    want_play: false,
                    last_wake: Instant::now(),
                    lost_at: None,
                }
                .run(rx)
            })
            .expect("spawn player thread");
        Self { tx, state }
    }

    /// The real player: ffmpeg into the default sound device.
    pub fn system(tools: ToolPaths, wake: Wake) -> Self {
        Self::spawn(tools, Arc::new(Ffmpeg), output::system(), wake)
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(Msg::Cmd(cmd));
    }

    pub fn state(&self) -> PlayerState {
        self.state.lock().unwrap().clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

struct Current {
    source: Source,
    load_id: u64,
    resolved: Option<Resolved>,
    /// Where to start once resolved.
    start_at: f64,
    stream: Option<ActiveStream>,
}

struct ActiveStream {
    id: u64,
    /// Source seconds where this stream begins.
    start: f64,
    stop: Arc<AtomicBool>,
    kill: Arc<dyn Fn() + Send + Sync>,
}

struct Engine {
    media: Arc<dyn Media>,
    tools: ToolPaths,
    open_output: OpenOutput,
    output: Option<Box<dyn Output>>,
    render_tx: Option<rtrb::Producer<RenderMsg>>,
    rate: u32,
    shared: Arc<Shared>,
    state: Arc<Mutex<PlayerState>>,
    wake: Wake,
    tx: Sender<Msg>,
    load_id: u64,
    current: Option<Current>,
    next_stream: u64,
    /// The user wants sound (as opposed to paused).
    want_play: bool,
    last_wake: Instant,
    /// While the output device is gone: when to try opening it again.
    lost_at: Option<Instant>,
}

/// How often the UI is woken while the position moves.
const WAKE_EVERY: Duration = Duration::from_millis(100);

impl Engine {
    fn run(&mut self, rx: Receiver<Msg>) {
        loop {
            let tick = if self.current.is_some() {
                Duration::from_millis(25)
            } else {
                Duration::from_millis(500)
            };
            let changed = match rx.recv_timeout(tick) {
                Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(msg) => {
                    self.handle(msg);
                    true
                }
                Err(RecvTimeoutError::Timeout) => false,
            };
            let changed = self.poll() || changed;
            let moving = self.state.lock().unwrap().status == Status::Playing;
            if changed || moving && self.last_wake.elapsed() >= WAKE_EVERY {
                self.last_wake = Instant::now();
                (self.wake)();
            }
        }
        self.stop_stream();
    }

    fn set_state(&self, f: impl FnOnce(&mut PlayerState)) {
        f(&mut self.state.lock().unwrap());
    }

    fn status(&self) -> Status {
        self.state.lock().unwrap().status.clone()
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Cmd(cmd) => self.command(cmd),
            Msg::Resolved(load_id, result) => {
                let Some(cur) = self.current.as_mut().filter(|c| c.load_id == load_id) else {
                    return;
                };
                match result {
                    Ok(r) => {
                        let start = cur.start_at;
                        let info = r.info.clone();
                        cur.resolved = Some(r);
                        self.set_state(|s| {
                            s.duration = info.duration;
                            s.track = Some(info);
                        });
                        self.start_stream(start);
                    }
                    Err(e) => self.fail(e),
                }
            }
            Msg::DecodeEnded(id, result) => {
                let current = self
                    .current
                    .as_ref()
                    .and_then(|c| c.stream.as_ref())
                    .is_some_and(|s| s.id == id);
                if let (true, Err(e)) = (current, result) {
                    self.fail(e);
                }
            }
            Msg::Shutdown => {}
        }
    }

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::Load(source, start_at) => self.load(source, start_at.max(0.0)),
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::Toggle => {
                if self.want_play && self.status() != Status::Ended {
                    self.pause();
                } else {
                    self.play();
                }
            }
            Command::Seek(secs) => self.seek(secs),
            Command::SetVolume(v) => {
                let v = v.clamp(0.0, 1.0);
                self.shared.volume.set(v);
                self.set_state(|s| s.volume = v);
            }
            Command::Stop => {
                self.stop_stream();
                self.current = None;
                self.want_play = false;
                self.shared.playing.store(false, Ordering::Relaxed);
                self.set_state(|s| {
                    s.status = Status::Idle;
                    s.source = None;
                    s.track = None;
                    s.position = 0.0;
                    s.duration = None;
                    s.buffering = false;
                });
            }
            Command::SetTools(tools) => self.tools = tools,
        }
    }

    fn load(&mut self, source: Source, start_at: f64) {
        self.stop_stream();
        self.load_id += 1;
        let load_id = self.load_id;
        self.want_play = true;
        self.shared.playing.store(true, Ordering::Relaxed);
        self.set_state(|s| {
            s.status = Status::Loading;
            s.source = Some(source.clone());
            s.track = Some(TrackInfo {
                title: source.label(),
                ..Default::default()
            });
            s.position = start_at;
            s.duration = None;
            s.buffering = false;
            s.load_id = load_id;
        });
        self.current = Some(Current {
            source: source.clone(),
            load_id,
            resolved: None,
            start_at,
            stream: None,
        });
        let media = self.media.clone();
        let tools = self.tools.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = media.resolve(&tools, &source).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Resolved(load_id, result));
        });
    }

    fn play(&mut self) {
        match self.status() {
            Status::Idle => return,
            Status::Ended => {
                self.want_play = true;
                self.shared.playing.store(true, Ordering::Relaxed);
                self.seek(0.0);
                self.set_state(|s| s.status = Status::Playing);
                return;
            }
            Status::Error(_) => {
                // Try again from where it stopped.
                if let Some(cur) = &self.current {
                    let (source, at) = (cur.source.clone(), self.state.lock().unwrap().position);
                    self.load(source, at);
                }
                return;
            }
            Status::Paused => self.set_state(|s| s.status = Status::Playing),
            Status::Loading | Status::Playing => {}
        }
        self.want_play = true;
        self.shared.playing.store(true, Ordering::Relaxed);
    }

    fn pause(&mut self) {
        self.want_play = false;
        self.shared.playing.store(false, Ordering::Relaxed);
        if self.status() == Status::Playing {
            self.set_state(|s| s.status = Status::Paused);
        }
    }

    fn seek(&mut self, secs: f64) {
        let Some(cur) = self.current.as_mut() else { return };
        let duration = cur.resolved.as_ref().and_then(|r| r.info.duration);
        let secs = duration.map_or(secs, |d| secs.min(d)).max(0.0);
        if cur.resolved.is_none() {
            cur.start_at = secs;
            self.set_state(|s| s.position = secs);
            return;
        }
        self.set_state(|s| s.position = secs);
        if self.status() == Status::Ended {
            let status = if self.want_play { Status::Playing } else { Status::Paused };
            self.set_state(|s| s.status = status);
        }
        self.start_stream(secs);
    }

    /// Stops the current decoder and silences the output.
    fn stop_stream(&mut self) {
        if let Some(s) = self.current.as_mut().and_then(|c| c.stream.take()) {
            s.stop.store(true, Ordering::Relaxed);
            // Killing a process tree on Windows takes a moment; don't hold up the next stream.
            let kill = s.kill.clone();
            thread::spawn(move || kill());
        }
        if let Some(tx) = &mut self.render_tx {
            let _ = tx.push(RenderMsg::Clear);
        }
    }

    fn ensure_output(&mut self) -> Result<()> {
        if self.output.is_some() {
            return Ok(());
        }
        let mut out = (self.open_output)()?;
        let rate = out.sample_rate();
        let (tx, renderer) = Renderer::new(self.shared.clone(), rate);
        out.start(renderer)?;
        self.rate = rate;
        self.render_tx = Some(tx);
        self.output = Some(out);
        Ok(())
    }

    /// Starts decoding the current track from `pos` seconds.
    fn start_stream(&mut self, pos: f64) {
        self.stop_stream();
        if let Err(e) = self.ensure_output() {
            self.fail(format!("{e:#}"));
            return;
        }
        let Some(input) = self
            .current
            .as_ref()
            .and_then(|c| c.resolved.as_ref())
            .map(|r| r.input.clone())
        else {
            return;
        };
        let pcm = match self.media.decode(&self.tools, &input, pos, self.rate) {
            Ok(p) => p,
            Err(e) => {
                self.fail(format!("{e:#}"));
                return;
            }
        };
        let id = self.next_stream;
        self.next_stream += 1;
        let (producer, consumer) = render::stream_buffer(self.rate);
        let eof = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(tx) = &mut self.render_tx {
            let _ = tx.push(RenderMsg::Play(Stream {
                id,
                pcm: consumer,
                eof: eof.clone(),
            }));
        }
        let Pcm { reader, kill, finish } = pcm;
        let tx = self.tx.clone();
        let stop_reader = stop.clone();
        thread::spawn(move || {
            let pumped = decode::pump(reader, producer, &stop_reader, &eof);
            let result = finish()
                .and_then(|()| pumped.map_err(Into::into))
                .map_err(|e| format!("{e:#}"));
            // A stopped stream's errors (the kill) don't matter.
            if !stop_reader.load(Ordering::Relaxed) {
                let _ = tx.send(Msg::DecodeEnded(id, result));
            }
        });
        if let Some(cur) = self.current.as_mut() {
            cur.stream = Some(ActiveStream {
                id,
                start: pos,
                stop,
                kill: Arc::from(kill),
            });
        }
    }

    fn fail(&mut self, msg: String) {
        self.stop_stream();
        if let Some(cur) = self.current.as_mut() {
            cur.stream = None;
        }
        self.set_state(|s| {
            s.status = Status::Error(msg);
            s.buffering = false;
        });
    }

    /// Reads the audio callback's progress into the snapshot. Returns true if the status
    /// or buffering changed.
    fn poll(&mut self) -> bool {
        self.check_output();
        let Some(stream) = self.current.as_ref().and_then(|c| c.stream.as_ref()) else {
            return false;
        };
        let (id, start) = (stream.id, stream.start);
        let sh = &self.shared;
        let playing_this = sh.stream_id.load(Ordering::Relaxed) == id;
        let frames = if playing_this { sh.frames.load(Ordering::Relaxed) } else { 0 };
        let ready = sh.ready_id.load(Ordering::Relaxed) >= id;
        let drained = sh.drained_id.load(Ordering::Relaxed) >= id;
        let underrun = playing_this && sh.underrun.load(Ordering::Relaxed);
        let rate = self.rate.max(1) as f64;
        let lost = self.lost_at.is_some();
        let want_play = self.want_play;

        let mut st = self.state.lock().unwrap();
        let before = (st.status.clone(), st.buffering);
        let duration = st.duration;
        st.position = duration.map_or(start + frames as f64 / rate, |d| (start + frames as f64 / rate).min(d));
        if st.status == Status::Loading && ready {
            st.status = if want_play { Status::Playing } else { Status::Paused };
        }
        if drained && st.status == Status::Playing {
            st.status = Status::Ended;
            if let Some(d) = duration {
                st.position = d;
            }
        }
        st.buffering = lost || st.status == Status::Loading || st.status == Status::Playing && (underrun || !ready);
        let changed = before != (st.status.clone(), st.buffering);
        drop(st);
        if changed && self.status() == Status::Ended {
            self.shared.playing.store(false, Ordering::Relaxed);
        }
        changed
    }

    /// Reopens the default device after the current one goes away, and picks up where the
    /// track was.
    fn check_output(&mut self) {
        if self.output.as_ref().is_some_and(|o| o.lost()) {
            self.output = None;
            self.render_tx = None;
            self.lost_at = Some(Instant::now());
        }
        let Some(retry_at) = self.lost_at else { return };
        if Instant::now() < retry_at {
            return;
        }
        if self.ensure_output().is_err() {
            self.lost_at = Some(Instant::now() + Duration::from_secs(1));
            return;
        }
        self.lost_at = None;
        let pos = self.state.lock().unwrap().position;
        let has_stream = self.current.as_ref().is_some_and(|c| c.stream.is_some());
        if has_stream && self.status() != Status::Ended {
            self.start_stream(pos);
        }
    }
}

#[cfg(test)]
mod e2e;
#[cfg(test)]
mod tests;
