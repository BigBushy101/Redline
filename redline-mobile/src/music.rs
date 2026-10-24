//! The music player and playlists for Kotlin: redline-core's player engine, the playlists file
//! and the Now playing queue, driven the way the desktop's frame loop drives them. A follower
//! thread does that loop's player work (catching the queue up when a track ends, handing over
//! settings, saving playlists) whenever the player or a call wakes it, then nudges Kotlin,
//! which pulls a snapshot.
//!
//! It shares the engine's state: History (offline copies play from their file), the player's
//! and network settings (saved by the engine's thread) and the job queue (offline downloads).
//! Saved music ([`crate::saved`]) lives here too, since it plays through the same queue.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redline_core::Wake;
use redline_core::history::History;
use redline_core::library::{self, Library, NowPlaying, PlaylistId, Track};
use redline_core::linkcheck::{self, Candidate, FixItem, LinkStatus, PlayWatch};
use redline_core::player::{Command, Player, PlayerSettings, PlayerState, Source, dsp};
use redline_core::saved::Saved;
use redline_core::tools::ToolPaths;
use redline_core::tools::ytdlp::NetworkOptions;

use crate::engine::{Shared, Signal, State, follow};
use crate::error::RedlineError;
use crate::records::*;
use crate::saved::Syncs;

/// Kotlin's side of the player's nudge.
#[uniffi::export(with_foreign)]
pub trait MusicListener: Send + Sync {
    /// The player moved (position, status, the next track), or the queue, playlists or player
    /// settings changed; pull what you show. Like [`crate::EngineListener::on_change`]: on
    /// Redline's own thread, at most ten times a second (that often while playing), so hop to
    /// the main thread and don't block.
    fn on_change(&self);
}

/// Previous goes back to the start of the track once it's this many seconds in.
const RESTART_AFTER: f64 = 3.0;

/// The desktop app's player fields: the playlists, Saved, the queue, and what the player was
/// told.
#[derive(Default)]
pub(crate) struct Controller {
    library: Library,
    pub(crate) saved: Saved,
    /// Bumped whenever Saved or its refreshes change, so Kotlin re-reads only then.
    pub(crate) saved_version: u64,
    /// Saved collections waiting to be refreshed, and the one being refreshed.
    pub(crate) syncs: Syncs,
    now: NowPlaying,
    /// Bumped whenever the queue's tracks, order or current track change.
    queue_version: u64,
    /// Auto-advances of the player the queue has caught up with.
    seen_advances: u64,
    /// The next track (by its own source) last handed to the player.
    sent_next: Option<Option<Source>>,
    /// The load the speed was last reset for.
    seen_load: u64,
    /// Volume, speed and EQ the player was last given.
    player_sent: Option<PlayerSettings>,
    player_network: Option<NetworkOptions>,
    player_tools: Option<ToolPaths>,
    /// Notices links failing to play, and records why.
    play_watch: PlayWatch,
    /// The last link that failed to play, for Kotlin to say so with Find replacement.
    failure: Option<PlayFailureRecord>,
}

impl Controller {
    fn queue_changed(&mut self) {
        self.queue_version += 1;
    }

    /// Hands changed volume, speed, EQ, network settings and tools to the player. Returns
    /// whether it changed the settings (the speed going back to 1x for a new track).
    fn sync_player(&mut self, player: &Player, ps: &PlayerState, st: &mut State) -> bool {
        let mut changed = false;
        if ps.load_id != self.seen_load {
            self.seen_load = ps.load_id;
            if st.settings.player.reset_speed && st.settings.player.speed != 1.0 {
                st.settings.player.speed = 1.0;
                changed = true;
            }
        }
        let want = &st.settings.player;
        let sent = self.player_sent.as_ref();
        if sent.is_none_or(|s| s.volume != want.volume) {
            player.send(Command::SetVolume(want.volume));
        }
        if sent.is_none_or(|s| s.speed != want.speed) {
            player.send(Command::SetSpeed(want.speed));
        }
        if sent.is_none_or(|s| s.eq != want.eq) {
            player.send(Command::SetEq(want.eq));
        }
        self.player_sent = Some(want.clone());
        let mut tools_changed = false;
        if self.player_network.as_ref() != Some(&st.settings.network) {
            self.player_network = Some(st.settings.network.clone());
            player.send(Command::SetNetwork(st.settings.network.clone()));
            tools_changed = true;
        }
        if self.player_tools.as_ref() != Some(&st.tools) {
            self.player_tools = Some(st.tools.clone());
            player.send(Command::SetTools(st.tools.clone()));
            tools_changed = true;
        }
        if tools_changed {
            // The link checks run with the same tools and network settings.
            linkcheck::shared().set_tools(st.tools.clone(), st.settings.network.clone());
        }
        changed
    }

    /// Keeps the player told what follows the current track, and catches the queue up when
    /// the player has moved on by itself.
    fn follow_queue(&mut self, player: &Player, ps: &PlayerState, history: &History) {
        let advances = ps.advances;
        if advances != self.seen_advances {
            for _ in self.seen_advances..advances {
                self.now.after_end();
            }
            self.seen_advances = advances;
            self.sent_next = None;
            self.queue_changed();
        }
        // Compared by the track's own source, so History is only searched on a change.
        let next = self.now.peek_after_end().map(|t| t.source.clone());
        if self.sent_next.as_ref() != Some(&next) {
            let playable = self.now.peek_after_end().map(|t| library::playable_source(t, history));
            player.send(Command::SetNext(playable));
            self.sent_next = Some(next);
        }
    }

    /// When a link fails to play (the desktop's `watch_failures`): the link checks note why,
    /// so its rows show it; the queue moves on if it won't play however often it's tried; and
    /// the failure is kept for Kotlin to show with Find replacement.
    fn watch_failures(&mut self, player: &Player, ps: &PlayerState, history: &History) {
        let lc = linkcheck::shared();
        let Some(failed) = self.play_watch.check(lc, ps, &self.now) else { return };
        let skipped = failed.skip && lc.skip_failed(&mut self.now);
        if skipped {
            self.load_current(player, history);
        }
        let id = self.failure.as_ref().map_or(1, |f| f.id + 1);
        self.failure = Some(play_failure_record(id, &failed, skipped, &self.library, &self.saved, history));
    }

    /// Loads the queue's current track, from its downloaded copy when there is one.
    fn load_current(&mut self, player: &Player, history: &History) {
        if let Some(t) = self.now.current() {
            player.send(Command::Load(library::playable_source(t, history), 0.0));
            // A load clears the player's next track; hand it over again.
            self.sent_next = None;
        }
        self.queue_changed();
    }

    /// Replaces Now playing with `tracks` and starts at `start`. Dead links are left out (a
    /// dead `start` moves on to the next live track), unless they're all dead.
    pub(crate) fn play_tracks(&mut self, p: &Player, h: &History, tracks: Vec<Track>, start: usize, playlist: Option<PlaylistId>) {
        let (tracks, start) = linkcheck::shared().drop_dead(tracks, start);
        self.now.start(tracks, start, playlist);
        self.load_current(p, h);
    }

    pub(crate) fn play_from(&mut self, p: &Player, h: &History, tracks: Vec<Track>, start: usize, shuffle: bool, playlist: Option<PlaylistId>) {
        let start = if shuffle && !tracks.is_empty() {
            // Shuffle from a random first track, not always the top one.
            self.now.shuffle = true;
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
            nanos as usize % tracks.len()
        } else {
            start
        };
        self.play_tracks(p, h, tracks, start, playlist);
    }

    /// Adds tracks to the end of Now playing, starting it if nothing's queued. Dead links are
    /// left out, as when playing.
    fn queue_tracks(&mut self, p: &Player, h: &History, tracks: Vec<Track>) {
        let tracks = linkcheck::shared().drop_dead(tracks, 0).0;
        if self.now.is_empty() {
            self.play_tracks(p, h, tracks, 0, None);
        } else {
            self.now.append(tracks);
            self.queue_changed();
        }
    }

    /// Puts tracks right after the current one, starting Now playing if nothing's queued.
    /// Dead links are left out, as when playing.
    fn play_next(&mut self, p: &Player, h: &History, tracks: Vec<Track>) {
        let tracks = linkcheck::shared().drop_dead(tracks, 0).0;
        if self.now.is_empty() {
            self.play_tracks(p, h, tracks, 0, None);
        } else {
            self.now.insert_next(tracks);
            // What follows the current track changed: hand the player its new next one.
            self.sent_next = None;
            self.queue_changed();
        }
    }
}

pub(crate) struct Inner {
    pub(crate) engine: Arc<Shared>,
    pub(crate) player: Player,
    ctl: Mutex<Controller>,
    /// Wakes the follower: the player when its state changes, calls after changing things.
    pub(crate) signal: Arc<Signal>,
    listener: Mutex<Arc<dyn MusicListener>>,
}

impl Inner {
    pub(crate) fn ctl(&self) -> MutexGuard<'_, Controller> {
        self.ctl.lock().unwrap()
    }

    /// One turn of the desktop's frame loop, the player's part. Locks the controller before
    /// the engine's state, as every call here does.
    fn step(&self) -> (Option<Duration>, bool) {
        let ps = self.player.state();
        let mut ctl = self.ctl();
        let settings_changed = {
            let mut st = self.engine.state();
            let changed = ctl.sync_player(&self.player, &ps, &mut st);
            ctl.follow_queue(&self.player, &ps, &st.history);
            ctl.watch_failures(&self.player, &ps, &st.history);
            changed
        };
        ctl.library.save_if_dirty();
        ctl.saved.save_if_dirty();
        drop(ctl);
        if settings_changed {
            // The engine's thread saves settings.
            self.engine.signal.poke();
        }
        // The player wakes us when anything moves.
        (None, false)
    }

    fn notify(&self) {
        let listener = self.listener.lock().unwrap().clone();
        listener.on_change();
    }

    /// Changes the queue with the player and History at hand, then lets the follower act on it.
    pub(crate) fn queue<R>(&self, f: impl FnOnce(&mut Controller, &Player, &History) -> R) -> R {
        let r = {
            let mut ctl = self.ctl();
            let st = self.engine.state();
            f(&mut ctl, &self.player, &st.history)
        };
        self.signal.poke();
        r
    }

    /// Changes the player's settings; the follower hands them over and the engine saves them.
    fn settings(&self, f: impl FnOnce(&mut PlayerSettings)) {
        f(&mut self.engine.state().settings.player);
        self.engine.signal.poke();
        self.signal.poke();
    }

    /// Changes the playlists; the follower saves them.
    pub(crate) fn library<R>(&self, f: impl FnOnce(&mut Library) -> R) -> R {
        let r = f(&mut self.ctl().library);
        self.signal.poke();
        r
    }

    fn send(&self, cmd: Command) {
        self.player.send(cmd);
        self.signal.poke();
    }
}

/// The music player, Now playing and the playlists. Get it from [`crate::Engine::music`].
#[derive(uniffi::Object)]
pub struct Music {
    pub(crate) inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Music {
    /// Starts the follower with the player `make_player` builds; it gets the [`Wake`] that
    /// wakes the follower.
    pub(crate) fn start(
        engine: Arc<Shared>,
        listener: Arc<dyn MusicListener>,
        make_player: impl FnOnce(Wake) -> Player,
    ) -> Arc<Self> {
        let signal = Arc::new(Signal::default());
        let poke = signal.clone();
        let player = make_player(Arc::new(move || poke.poke()));
        let inner = Arc::new(Inner {
            engine,
            player,
            ctl: Mutex::new(Controller { library: Library::load(), saved: Saved::load(), ..Default::default() }),
            signal,
            listener: Mutex::new(listener),
        });
        // Link check results nudge Kotlin, which reads the statuses again. Weak: the checks
        // outlive this player.
        let poke = Arc::downgrade(&inner.signal);
        linkcheck::shared().set_wake(Arc::new(move || {
            if let Some(s) = poke.upgrade() {
                s.poke();
            }
        }));
        let runner = inner.clone();
        let thread = thread::Builder::new()
            .name("redline-music".into())
            // Eager: a track's end has to be followed by the next one being handed over at
            // once, not when the next notification is due.
            .spawn(move || follow(&runner.signal, true, || runner.step(), || runner.notify()))
            .expect("can't start the music thread");
        inner.refresh_on_launch();
        Arc::new(Self { inner, thread: Mutex::new(Some(thread)) })
    }

    pub(crate) fn set_listener(&self, listener: Arc<dyn MusicListener>) {
        *self.inner.listener.lock().unwrap() = listener;
        self.inner.signal.poke();
    }

    /// Stops playback, the follower and Saved's refreshes, and saves the playlists and Saved.
    /// Calls still work afterwards, but nothing follows them.
    pub(crate) fn shutdown(&self) {
        self.inner.signal.stop();
        if let Some(t) = self.thread.lock().unwrap().take()
            // Kotlin could drop the last reference from inside on_change.
            && t.thread().id() != thread::current().id()
        {
            let _ = t.join();
        }
        self.inner.player.send(Command::Stop);
        let mut ctl = self.inner.ctl();
        ctl.library.save_if_dirty();
        ctl.saved.save_if_dirty();
    }

    pub(crate) fn records(&self, tracks: &[Track]) -> Vec<TrackRecord> {
        let st = self.inner.engine.state();
        tracks.iter().map(|t| track_record(t, &st.history)).collect()
    }

    fn history_entry<R>(&self, index: u32, f: impl FnOnce(&redline_core::history::HistoryEntry) -> R) -> Option<R> {
        self.inner.engine.state().history.entries.get(index as usize).map(f)
    }

    /// yt-dlp and the network settings, for a replacement search.
    fn search_tools(&self) -> Result<(ToolPaths, NetworkOptions), RedlineError> {
        let st = self.inner.engine.state();
        if st.tools.ytdlp.is_none() {
            return Err(RedlineError::ToolsMissing("Finding replacements needs yt-dlp.".into()));
        }
        Ok((st.tools.clone(), st.settings.network.clone()))
    }

    /// The dead links among `tracks` (with their positions), searched for replacements one
    /// after another. Blocks for a few seconds per track; takes no lock while it does.
    pub(crate) fn find_fixes(&self, tracks: &[Track]) -> Result<Vec<FixItem>, RedlineError> {
        let statuses = linkcheck::shared().statuses(tracks);
        let dead: Vec<(usize, Track)> = tracks
            .iter()
            .zip(statuses)
            .enumerate()
            .filter(|(_, (_, s))| s.is_dead())
            .map(|(i, (t, _))| (i, t.clone()))
            .collect();
        if dead.is_empty() {
            return Ok(Vec::new());
        }
        let (tools, net) = self.search_tools()?;
        Ok(linkcheck::fix_unavailable(&tools, &net, dead))
    }

    /// Swaps in each item's clear match with `apply` (given the position, the track as it was
    /// found and the pick; false if it's no longer there), and lists the rest for review.
    pub(crate) fn fix_report(
        &self,
        items: Vec<FixItem>,
        mut apply: impl FnMut(usize, &Track, &Candidate) -> bool,
    ) -> FixReportRecord {
        let mut fixed = 0;
        let mut review = Vec::new();
        for item in items {
            if let Some(c) = item.clear_match()
                && apply(item.index, &item.track, c)
            {
                linkcheck::shared().record(&c.url, LinkStatus::Alive);
                fixed += 1;
                continue;
            }
            review.push((item.index, item.track, item.ranked));
        }
        let st = self.inner.engine.state();
        FixReportRecord {
            fixed,
            review: review
                .into_iter()
                .map(|(index, track, ranked)| ReviewRecord {
                    index: index as u32,
                    track: track_record(&track, &st.history),
                    candidates: ranked.iter().map(replacement_record).collect(),
                })
                .collect(),
        }
    }
}

impl Drop for Music {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[uniffi::export]
impl Music {
    // ---- What's playing ----

    /// The player bar's state.
    pub fn snapshot(&self) -> PlayerSnapshotRecord {
        let ps = self.inner.player.state();
        let ctl = self.inner.ctl();
        PlayerSnapshotRecord { failure: ctl.failure.clone(), ..player_snapshot(&ps, &ctl.now, ctl.queue_version) }
    }

    /// Now playing, in play order. Changes when `snapshot().queue_version` does.
    pub fn queue(&self) -> Vec<QueueTrackRecord> {
        let ctl = self.inner.ctl();
        let st = self.inner.engine.state();
        queue_records(&ctl.now, &st.history)
    }

    // ---- Transport ----

    pub fn play(&self) {
        self.inner.send(Command::Play);
    }

    pub fn pause(&self) {
        self.inner.send(Command::Pause);
    }

    pub fn toggle(&self) {
        self.inner.send(Command::Toggle);
    }

    pub fn seek(&self, secs: f64) {
        self.inner.send(Command::Seek(secs.max(0.0)));
    }

    /// Puts the A-B loop's A marker where the track is now.
    pub fn set_loop_a(&self) {
        let pos = self.inner.player.state().position;
        self.inner.send(Command::SetLoopA(pos));
    }

    /// Puts the B marker where the track is now; from then on reaching it goes back to A (the
    /// start, if A isn't set). Ignored if it would be before A.
    pub fn set_loop_b(&self) {
        let pos = self.inner.player.state().position;
        self.inner.send(Command::SetLoopB(pos));
    }

    /// Ends the A-B loop. Changing tracks ends it too.
    pub fn clear_loop(&self) {
        self.inner.send(Command::ClearLoop);
    }

    pub fn next(&self) {
        self.inner.queue(|c, p, h| {
            if c.now.next().is_some() {
                c.load_current(p, h);
            }
        });
    }

    /// Back to the start of the track, or the one before if it only just started.
    pub fn prev(&self) {
        let position = self.inner.player.state().position;
        self.inner.queue(|c, p, h| {
            if position > RESTART_AFTER || !c.now.has_prev() {
                p.send(Command::Seek(0.0));
            } else if c.now.prev().is_some() {
                c.load_current(p, h);
            }
        });
    }

    /// Stops and empties Now playing.
    pub fn stop(&self) {
        self.inner.queue(|c, p, _| {
            c.now.clear();
            c.queue_changed();
            p.send(Command::Stop);
        });
    }

    /// Plays the queue's track `index` ([`QueueTrackRecord::index`]).
    pub fn jump(&self, index: u32) {
        self.inner.queue(|c, p, h| {
            if c.now.jump(index as usize).is_some() {
                c.load_current(p, h);
            }
        });
    }

    /// Shuffling keeps the current track and plays the rest in a random order.
    pub fn set_shuffle(&self, on: bool) {
        self.inner.queue(|c, _, _| {
            if c.now.shuffle != on {
                c.now.set_shuffle(on);
                c.queue_changed();
            }
        });
    }

    pub fn set_repeat(&self, repeat: RepeatMode) {
        self.inner.queue(|c, _, _| c.now.repeat = repeat.into());
    }

    /// Takes a track out of Now playing. Not the current one (stop or skip it instead), as on
    /// the desktop.
    pub fn remove_from_queue(&self, index: u32) {
        self.inner.queue(|c, _, _| {
            let index = index as usize;
            if index < c.now.tracks.len() && c.now.current_index() != Some(index) {
                c.now.remove(index);
                c.queue_changed();
            }
        });
    }

    // ---- Player settings ----

    pub fn player_settings(&self) -> PlayerSettingsRecord {
        player_settings_record(&self.inner.engine.state().settings.player)
    }

    /// 0 to 1.
    pub fn set_volume(&self, volume: f32) {
        let v = if volume.is_finite() { volume.clamp(0.0, 1.0) } else { 1.0 };
        self.inner.settings(|p| p.volume = v);
    }

    /// 0.5 to 2 ([`PlayerChoicesRecord`]); pitch follows.
    pub fn set_speed(&self, speed: f32) {
        let s = if speed.is_finite() { speed.clamp(dsp::MIN_SPEED, dsp::MAX_SPEED) } else { 1.0 };
        self.inner.settings(|p| p.speed = s);
    }

    /// Back to 1x whenever a new track loads.
    pub fn set_reset_speed(&self, on: bool) {
        self.inner.settings(|p| p.reset_speed = on);
    }

    /// Gains outside the ranges in [`PlayerChoicesRecord`] are clamped.
    pub fn set_eq(&self, eq: EqRecord) {
        let eq = dsp::EqSettings::from(&eq);
        self.inner.settings(|p| p.eq = eq);
    }

    // ---- Playing things ----

    /// Replaces Now playing with a playlist from track `start`, or from a random one shuffled.
    pub fn play_playlist(&self, id: u64, start: u32, shuffle: bool) {
        let Some(tracks) = self.inner.ctl().library.get(id).map(|p| p.tracks.clone()) else { return };
        self.inner.queue(|c, p, h| c.play_from(p, h, tracks, start as usize, shuffle, Some(id)));
    }

    /// Replaces Now playing with `tracks` (not a playlist's), from track `start`.
    pub fn play_tracks(&self, tracks: Vec<TrackRecord>, start: u32) {
        let tracks = tracks.iter().map(TrackRecord::to_core).collect();
        self.inner.queue(|c, p, h| c.play_tracks(p, h, tracks, start as usize, None));
    }

    /// Adds tracks to the end of Now playing, starting it if it's empty.
    pub fn queue_tracks(&self, tracks: Vec<TrackRecord>) {
        let tracks = tracks.iter().map(TrackRecord::to_core).collect();
        self.inner.queue(|c, p, h| c.queue_tracks(p, h, tracks));
    }

    /// Puts tracks right after the current one ("Play next"), starting Now playing if it's
    /// empty.
    pub fn play_next(&self, tracks: Vec<TrackRecord>) {
        let tracks = tracks.iter().map(TrackRecord::to_core).collect();
        self.inner.queue(|c, p, h| c.play_next(p, h, tracks));
    }

    /// Plays one file on its own.
    pub fn play_file(&self, path: String) {
        let source = Source::Local(PathBuf::from(path));
        let title = source.label();
        self.inner.queue(|c, p, h| c.play_tracks(p, h, vec![Track::new(source, title)], 0, None));
    }

    /// Plays a History row: its file while that's there, otherwise its link streamed.
    pub fn play_history(&self, index: u32) -> Result<(), RedlineError> {
        let track = self
            .history_entry(index, |e| {
                let file = e.output.clone().filter(|p| playable_file(p));
                match (file, history_track(e)) {
                    (Some(f), Some(t)) => Some(Track::new(Source::Local(f), t.title)),
                    (_, t) => t,
                }
            })
            .ok_or_else(|| RedlineError::Invalid("That history entry is gone.".into()))?
            .ok_or_else(|| RedlineError::Invalid("Its file was moved or deleted.".into()))?;
        self.inner.queue(|c, p, h| c.play_tracks(p, h, vec![track], 0, None));
        Ok(())
    }

    // ---- Playlists ----

    pub fn playlists(&self) -> Vec<PlaylistSummaryRecord> {
        self.inner.ctl().library.playlists.iter().map(playlist_summary).collect()
    }

    pub fn playlist(&self, id: u64) -> Option<PlaylistRecord> {
        let ctl = self.inner.ctl();
        let p = ctl.library.get(id)?;
        Some(playlist_record(p, &self.inner.engine.state().history))
    }

    /// A blank name becomes "Playlist N". Returns the new playlist's id.
    pub fn create_playlist(&self, name: String) -> u64 {
        self.inner.library(|l| l.create(&name))
    }

    /// A blank name is ignored.
    pub fn rename_playlist(&self, id: u64, name: String) {
        self.inner.library(|l| l.rename(id, &name));
    }

    pub fn delete_playlist(&self, id: u64) {
        self.inner.library(|l| l.delete(id));
    }

    /// Adds tracks to the end of a playlist.
    pub fn add_tracks(&self, id: u64, tracks: Vec<TrackRecord>) {
        self.inner.library(|l| l.add(id, tracks.iter().map(TrackRecord::to_core)));
    }

    pub fn remove_track(&self, id: u64, index: u32) {
        self.inner.library(|l| l.remove_track(id, index as usize));
    }

    /// Moves the track at `from` to `to`, both positions in the current order.
    pub fn move_track(&self, id: u64, from: u32, to: u32) {
        self.inner.library(|l| l.move_track(id, from as usize, to as usize));
    }

    /// Swaps a replacement into the playlist's track at `index`, keeping its title, artist
    /// and place. Returns whether there was such a track.
    pub fn replace_playlist_track(&self, id: u64, index: u32, candidate: ReplacementRecord) -> bool {
        let c = candidate.to_core();
        let done = self.inner.library(|l| l.replace_link(id, index as usize, &c));
        if done {
            linkcheck::shared().record(&c.url, LinkStatus::Alive);
        }
        done
    }

    /// Swaps a replacement in for a song that failed to play ([`PlayFailureRecord`]): in
    /// `playlist_id`'s playlist, found by its link, or else in every saved copy. Returns
    /// whether the song was still there.
    pub fn replace_failed_track(&self, track: TrackRecord, playlist_id: Option<u64>, candidate: ReplacementRecord) -> bool {
        let Some(id) = playlist_id else { return self.replace_saved_track(track.location, candidate) };
        let source = track.to_core().source;
        let index = self.inner.ctl().library.get(id).and_then(|p| p.tracks.iter().position(|t| t.source == source));
        index.is_some_and(|i| self.replace_playlist_track(id, i as u32, candidate))
    }

    /// "Fix unavailable": searches for each dead link in the playlist, swaps in the clear
    /// matches and returns the rest to review. Blocks for a few seconds per dead track.
    pub fn fix_unavailable_playlist(&self, id: u64) -> Result<FixReportRecord, RedlineError> {
        let tracks = self.inner.ctl().library.get(id).map(|p| p.tracks.clone()).unwrap_or_default();
        let items = self.find_fixes(&tracks)?;
        Ok(self.fix_report(items, |index, was, c| {
            self.inner.library(|l| {
                // Only if the playlist wasn't changed under the search.
                let same = l.get(id).and_then(|p| p.tracks.get(index)).is_some_and(|t| t.source == was.source);
                same && l.replace_link(id, index, c)
            })
        }))
    }

    // ---- Dead links ----

    /// Checks, in the background, the links among `tracks` with no recent result (an open
    /// playlist's or collection's). Results nudge as they come in; see `link_check_version`.
    pub fn check_links(&self, tracks: Vec<TrackRecord>) {
        let tracks: Vec<Track> = tracks.iter().map(TrackRecord::to_core).collect();
        linkcheck::shared().request(&tracks);
    }

    /// Changes whenever a link check finds something; read the open page again then.
    pub fn link_check_version(&self) -> u64 {
        linkcheck::shared().version()
    }

    /// Other uploads of a track's song on YouTube, best first. Blocks while yt-dlp searches.
    pub fn find_replacements(&self, track: TrackRecord) -> Result<Vec<ReplacementRecord>, RedlineError> {
        let (tools, net) = self.search_tools()?;
        let ranked = linkcheck::search(&tools, &net, &track.to_core()).map_err(RedlineError::failed)?;
        Ok(ranked.iter().map(replacement_record).collect())
    }

    // ---- Copied files ----

    /// Deletes the files right in `dir` (the app's own folder that picked files are copied
    /// into for playlists) that nothing plays any more: not in a playlist, Now playing,
    /// History or `keep` (copies under way). Folders in it and anything outside it are left
    /// alone. Returns how many files it deleted.
    pub fn sweep_imported(&self, dir: String, keep: Vec<String>) -> u32 {
        let dir = PathBuf::from(dir);
        if !dir.is_absolute() {
            return 0;
        }
        let mut in_use: HashSet<PathBuf> = keep.into_iter().map(PathBuf::from).collect();
        {
            let ctl = self.inner.ctl();
            if ctl.library.is_unreadable() {
                // The playlists may well point at every one of them.
                return 0;
            }
            let st = self.inner.engine.state();
            let tracks = ctl.library.playlists.iter().flat_map(|p| &p.tracks).chain(&ctl.now.tracks);
            for t in tracks {
                if let Source::Local(path) = &t.source {
                    in_use.insert(path.clone());
                }
            }
            in_use.extend(st.history.entries.iter().filter_map(|e| e.output.clone()));
        }
        unused_files(&dir, &in_use)
            .into_iter()
            .filter(|f| std::fs::remove_file(f).is_ok())
            .count() as u32
    }

    // ---- Making tracks ----

    /// The tracks a link stands for: every entry of a playlist, or the one video. Blocks while
    /// yt-dlp looks it up, unless the Download page just did.
    pub fn tracks_for_link(&self, url: String) -> Result<Vec<TrackRecord>, RedlineError> {
        let info = self.inner.engine.look_up(&url, true)?;
        Ok(self.records(&library::tracks_from_info(&url, &info)))
    }

    /// Tracks for audio and video files and folders (searched all the way down), titled from
    /// their tags. Blocks while ffprobe reads each file.
    pub fn scan_files(&self, paths: Vec<String>) -> Vec<TrackRecord> {
        let tools = self.inner.engine.state().tools.clone();
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        self.records(&library::scan(&tools, &paths))
    }

    /// The file a track plays from, for sending it to Convert: a file track, or a link that's
    /// been downloaded. `None` for a stream.
    pub fn local_file(&self, track: TrackRecord) -> Option<String> {
        let st = self.inner.engine.state();
        library::local_file(&track.to_core(), &st.history).map(|p| p.to_string_lossy().into_owned())
    }

    /// A History row as a track to add to a playlist: a download's link (which plays its
    /// file while that's there), or a conversion's output. `None` if there's nothing to play.
    pub fn track_for_history(&self, index: u32) -> Option<TrackRecord> {
        let st = self.inner.engine.state();
        let track = history_track(st.history.entries.get(index as usize)?)?;
        Some(track_record(&track, &st.history))
    }

    /// Queues a download of a link track as audio (the Download page's audio options), so it
    /// plays offline once done. Returns the job's id.
    pub fn download_for_offline(&self, track: TrackRecord) -> Result<u64, RedlineError> {
        let id = {
            let mut st = self.inner.engine.state();
            if !st.tools.ready_for_download() {
                return Err(RedlineError::ToolsMissing("Downloading needs yt-dlp and ffmpeg.".into()));
            }
            let plan = library::offline_download(&track.to_core(), &st.settings)
                .ok_or_else(|| RedlineError::Invalid("Only links can be downloaded.".into()))?;
            st.jobs.push(plan.title, plan.subtitle, plan.spec);
            st.jobs.jobs.last().map_or(0, |j| j.id)
        };
        self.inner.engine.signal.poke();
        Ok(id)
    }
}

/// The files right in `dir` that aren't in `in_use`, compared by their real paths (a link to
/// the same file, like /sdcard for /storage/emulated/0, still counts as in use).
fn unused_files(dir: &Path, in_use: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let in_use: HashSet<PathBuf> = in_use.iter().map(|p| real(p)).collect();
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        // Not following links: only a regular file of the folder's own.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| !in_use.contains(p) && !in_use.contains(&real(p)))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::Ordering;

    use redline_core::player::decode::{Input, Pcm};
    use redline_core::player::output::Capture;
    use redline_core::player::{Env, Media, Resolved, TrackInfo};

    use crate::engine::Engine;
    use crate::engine::tests::{Counter, one_engine_at_a_time, test_dirs, wait_for};

    /// A quiet tone instead of ffmpeg: a second long, or twenty for files named `long`.
    struct Tone;

    fn seconds(source: &Source) -> f64 {
        if source.label() == "long" { 20.0 } else { 1.0 }
    }

    impl Media for Tone {
        fn resolve(&self, _: &Env, source: &Source) -> anyhow::Result<Resolved> {
            Ok(Resolved {
                input: Input { location: seconds(source).to_string(), headers: None },
                info: TrackInfo {
                    title: format!("Tone {}", source.label()),
                    duration: Some(seconds(source)),
                    ..Default::default()
                },
            })
        }

        fn decode(&self, _: &Env, input: &Input, start: f64, rate: u32) -> anyhow::Result<Pcm> {
            let secs: f64 = input.location.parse()?;
            let frames = ((secs - start).max(0.0) * rate as f64) as usize;
            let bytes: Vec<u8> = std::iter::repeat_n(0.1f32.to_le_bytes(), frames * 2).flatten().collect();
            Ok(Pcm { reader: Box::new(Cursor::new(bytes)), kill: Box::new(|| {}), finish: Box::new(|| Ok(())) })
        }
    }

    fn file(name: &str) -> TrackRecord {
        TrackRecord {
            kind: TrackKind::File,
            location: format!("{name}.mp3"),
            title: name.to_uppercase(),
            artist: None,
            duration_secs: None,
            thumbnail: None,
            added_at: 0,
            platform: None,
            offline: false,
            link_status: LinkStatusRecord::Unknown,
            replaced_from: None,
        }
    }

    fn link(n: u32) -> TrackRecord {
        TrackRecord {
            kind: TrackKind::Link,
            location: format!("https://www.youtube.com/watch?v={n}"),
            title: format!("Song {n}"),
            duration_secs: Some(60.0),
            thumbnail: Some(format!("https://img/{n}.jpg")),
            platform: Some(crate::saved::Platform::YouTube),
            ..file("x")
        }
    }

    /// An engine with no tools, and its player on the fake decoder at `speedup`x real time.
    fn music(speedup: f32) -> (Arc<Engine>, Arc<Music>, Arc<Counter>) {
        music_with_tools(ToolPaths::default(), speedup)
    }

    /// [`music`] with an engine that has `tools` (the player still uses the fake decoder).
    pub(crate) fn music_with_tools(tools: ToolPaths, speedup: f32) -> (Arc<Engine>, Arc<Music>, Arc<Counter>) {
        let dir = test_dirs();
        let engine = Engine::start(tools, Arc::new(Counter::default()), Some(dir.join("converted")));
        let listener = Arc::new(Counter::default());
        let capture = Capture::new(8000, speedup);
        let music = engine.music_with(listener.clone(), |wake| {
            Player::spawn(ToolPaths::default(), Arc::new(Tone), capture.opener(), wake)
        });
        (engine, music, listener)
    }

    fn until(music: &Music, what: &str, cond: impl Fn(&PlayerSnapshotRecord) -> bool) -> PlayerSnapshotRecord {
        let mut last = None;
        wait_for(what, || {
            let s = music.snapshot();
            let ok = cond(&s);
            last = Some(s);
            ok
        });
        last.unwrap()
    }

    #[test]
    fn the_queue_follows_the_player() {
        let _turn = one_engine_at_a_time();
        let (engine, music, listener) = music(8.0);
        wait_for("the first nudge", || listener.0.load(Ordering::SeqCst) > 0);
        assert!(Arc::ptr_eq(&music, &engine.music_with(listener.clone(), |_| unreachable!())), "one per engine");
        music.set_repeat(RepeatMode::Off);
        music.set_shuffle(false);

        // Each track ends and the player moves on to the next by itself.
        let v0 = music.snapshot().queue_version;
        music.play_tracks(vec![file("a"), file("b"), file("c")], 0);
        let s = until(&music, "the queue to play out", |s| s.status == PlayerStatus::Ended);
        assert_eq!((s.queue_index, s.queue_position, s.queue_length), (Some(2), Some(2), 3));
        assert_eq!(s.title.as_deref(), Some("Tone c"));
        assert_eq!(s.load_id, 3, "three loads, one per track");
        assert!(s.queue_version >= v0 + 3, "a load and two advances");
        assert!(!s.has_next && s.has_prev && !s.is_link);
        let queue = music.queue();
        assert_eq!(queue.iter().map(|q| q.track.title.as_str()).collect::<Vec<_>>(), ["A", "B", "C"]);
        assert!(queue[2].is_current && !queue[0].is_current);

        // Repeat one plays the same track again; Next still moves on.
        music.set_repeat(RepeatMode::One);
        music.jump(0);
        let s = until(&music, "a replay", |s| s.load_id >= 6);
        assert_eq!(s.queue_index, Some(0), "still the first track");
        music.next();
        let s = until(&music, "next", |s| s.queue_index == Some(1));
        assert_eq!(s.repeat, RepeatMode::One);

        // Repeat all wraps around from the last track.
        music.set_repeat(RepeatMode::All);
        music.jump(2);
        until(&music, "the wrap", |s| s.queue_index == Some(0) && s.title.as_deref() == Some("Tone a"));

        // Previous: early in a track it goes back one; further in, or at the top, to the start.
        music.set_repeat(RepeatMode::Off);
        music.play_tracks(vec![file("a"), file("long")], 1);
        until(&music, "the long track", |s| s.status == PlayerStatus::Playing && s.position_secs > 0.0);
        music.prev();
        until(&music, "the one before", |s| s.queue_index == Some(0));
        music.play_tracks(vec![file("long"), file("a")], 0);
        let s = until(&music, "4 s in", |s| s.status == PlayerStatus::Playing && s.position_secs > 4.0);
        music.prev();
        let s = until(&music, "back to the start", |n| n.load_id == s.load_id && n.position_secs < 3.0);
        assert_eq!(s.queue_index, Some(0), "same track");
        music.prev();
        thread::sleep(Duration::from_millis(50));
        let n = music.snapshot();
        assert_eq!((n.queue_index, n.load_id), (Some(0), s.load_id), "nothing before the first: restart");

        // A new track takes the speed back to 1x when asked.
        music.set_speed(1.5);
        music.set_reset_speed(true);
        assert_eq!(music.player_settings().speed, 1.5);
        music.jump(0);
        wait_for("the speed reset", || music.player_settings().speed == 1.0);
        music.set_reset_speed(false);

        // Removing tracks keeps the current one; Stop empties the queue.
        music.queue_tracks(vec![file("d")]);
        assert_eq!(music.snapshot().queue_length, 3, "queued after the rest");
        music.remove_from_queue(0);
        assert_eq!(music.snapshot().queue_length, 3, "not the current one");
        music.remove_from_queue(1);
        let s = music.snapshot();
        assert_eq!((s.queue_length, s.queue_index), (2, Some(0)));
        assert_eq!(music.queue().iter().map(|q| q.track.title.as_str()).collect::<Vec<_>>(), ["LONG", "D"]);
        music.stop();
        let s = until(&music, "stopped", |s| s.status == PlayerStatus::Idle);
        assert_eq!((s.queue_length, s.queue_index, s.title), (0, None, None));
        music.queue_tracks(vec![file("e")]);
        until(&music, "an empty queue to start", |s| s.title.as_deref() == Some("Tone e"));

        drop(engine);
        until(&music, "the engine's end to stop the player", |s| s.status == PlayerStatus::Idle);
    }

    #[test]
    fn play_next_through_the_api() {
        let _turn = one_engine_at_a_time();
        let (engine, music, _) = music(1.0);
        music.set_shuffle(false);
        music.play_next(vec![file("long")]);
        let s = until(&music, "an empty queue to start", |s| s.title.as_deref() == Some("Tone long"));
        let v = s.queue_version;
        music.queue_tracks(vec![file("a"), file("b")]);
        music.play_next(vec![file("c"), file("d")]);
        let s = music.snapshot();
        assert!(s.queue_version > v);
        assert_eq!((s.queue_index, s.queue_position, s.queue_length), (Some(0), Some(0), 5));
        assert_eq!(music.queue().iter().map(|q| q.track.title.as_str()).collect::<Vec<_>>(), ["LONG", "C", "D", "A", "B"]);
        // The player was handed the new next track: skipping lands on it.
        music.next();
        until(&music, "the track played next", |s| s.title.as_deref() == Some("Tone c"));
        music.stop();
        drop(music);
        drop(engine);
    }

    #[test]
    fn replacing_dead_links() {
        let _turn = one_engine_at_a_time();
        let (engine, music, _) = music(8.0);
        let checks = linkcheck::shared();
        let dead = link(901);
        checks.record(&dead.location, LinkStatus::Dead);
        let id = music.create_playlist("Fixes".into());
        music.add_tracks(id, vec![link(900), dead.clone(), link(902)]);
        assert_eq!(music.playlist(id).unwrap().tracks[1].link_status, LinkStatusRecord::Dead);

        // Playing from the dead one starts on the next live one, and leaves it out.
        music.play_playlist(id, 1, false);
        let s = music.snapshot();
        assert_eq!((s.queue_index, s.queue_length), (Some(1), 2));
        assert_eq!(music.queue().iter().map(|q| q.track.title.as_str()).collect::<Vec<_>>(), ["Song 900", "Song 902"]);
        music.stop();

        // Nothing to fix without yt-dlp, and nothing dead in a playlist without dead links.
        assert!(matches!(music.fix_unavailable_playlist(id), Err(RedlineError::ToolsMissing(_))));
        assert!(matches!(music.find_replacements(dead.clone()), Err(RedlineError::ToolsMissing(_))));
        let clean = music.create_playlist("Clean".into());
        music.add_tracks(clean, vec![link(900)]);
        assert_eq!(music.fix_unavailable_playlist(clean).unwrap(), FixReportRecord { fixed: 0, review: vec![] });

        let pick = ReplacementRecord {
            url: "https://www.youtube.com/watch?v=new901".into(),
            title: "Song 901 (Official Video)".into(),
            uploader: Some("Band".into()),
            duration_secs: Some(61.0),
            thumbnail: None,
            delta_secs: Some(1.0),
            clear_match: true,
        };
        assert!(music.replace_playlist_track(id, 1, pick.clone()));
        assert!(!music.replace_playlist_track(id, 7, pick.clone()));
        let t = &music.playlist(id).unwrap().tracks[1];
        assert_eq!((t.title.as_str(), t.location.as_str()), ("Song 901", pick.url.as_str()), "title and place kept");
        assert_eq!(t.replaced_from.as_deref(), Some(dead.location.as_str()));
        assert_eq!((t.duration_secs, t.thumbnail.as_deref()), (Some(61.0), Some("https://img/901.jpg")));
        assert_eq!(t.link_status, LinkStatusRecord::Alive, "the new link is known to play");
        assert_eq!(t.clone().to_core().replaced_from, t.replaced_from, "round trip");

        music.delete_playlist(id);
        music.delete_playlist(clean);
        drop(music);
        drop(engine);
    }

    /// Links ending in `gone` fail to look up the way YouTube's blocked uploads do; the rest
    /// play as [`Tone`].
    struct SomeGone;

    impl Media for SomeGone {
        fn resolve(&self, env: &Env, source: &Source) -> anyhow::Result<Resolved> {
            match source {
                Source::Link(u) if u.ends_with("gone") => Err(anyhow::anyhow!("Video unavailable. This video is not available")),
                _ => Tone.resolve(env, source),
            }
        }

        fn decode(&self, env: &Env, input: &Input, start: f64, rate: u32) -> anyhow::Result<Pcm> {
            Tone.decode(env, input, start, rate)
        }
    }

    #[test]
    fn links_that_fail_to_play_are_marked_and_skipped() {
        let _turn = one_engine_at_a_time();
        let dir = test_dirs();
        let engine = Engine::start(ToolPaths::default(), Arc::new(Counter::default()), Some(dir.join("converted")));
        let capture = Capture::new(8000, 8.0);
        let music = engine.music_with(Arc::new(Counter::default()), |wake| {
            Player::spawn(ToolPaths::default(), Arc::new(SomeGone), capture.opener(), wake)
        });
        music.set_shuffle(false);
        music.set_repeat(RepeatMode::Off);
        let gone = TrackRecord { location: "https://www.youtube.com/watch?v=960gone".into(), ..link(960) };
        let id = music.create_playlist("Gaps".into());
        music.add_tracks(id, vec![link(961), gone.clone(), link(962)]);
        assert_eq!(music.playlist(id).unwrap().tracks[1].link_status, LinkStatusRecord::Unknown);

        // Played: it fails, its row says so, and the queue moves on by itself.
        music.play_playlist(id, 1, false);
        let s = until(&music, "the next track", |s| s.queue_index == Some(2));
        let f = s.failure.expect("the failure, to say so");
        assert_eq!((f.track.location.as_str(), f.skipped, f.playlist_id, f.saved), (gone.location.as_str(), true, Some(id), false));
        assert!(f.error.contains("This video is not available"), "{}", f.error);
        assert_eq!(music.playlist(id).unwrap().tracks[1].link_status, LinkStatusRecord::Dead);

        // Find replacement puts the pick in its place.
        let pick = ReplacementRecord {
            url: "https://www.youtube.com/watch?v=960new".into(),
            title: "Song 960".into(),
            uploader: None,
            duration_secs: Some(60.0),
            thumbnail: None,
            delta_secs: Some(0.0),
            clear_match: true,
        };
        assert!(music.replace_failed_track(f.track.clone(), f.playlist_id, pick.clone()));
        let t = &music.playlist(id).unwrap().tracks[1];
        assert_eq!((t.location.as_str(), t.link_status), (pick.url.as_str(), LinkStatusRecord::Alive));
        assert!(!music.replace_failed_track(f.track, f.playlist_id, pick), "it isn't there any more");

        music.stop();
        music.delete_playlist(id);
        drop(music);
        drop(engine);
    }

    #[test]
    fn sweeping_copied_files() {
        let _turn = one_engine_at_a_time();
        let (engine, music, _) = music(1.0);
        let dir = test_dirs().join("imported");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        for f in ["listed.mp3", "queued.mp3", "copying.mp3", "orphan.mp3", "folder/inner.mp3"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let track = |name: &str| TrackRecord { location: path(name), ..file(name) };

        let id = music.create_playlist("Phone".into());
        music.add_tracks(id, vec![track("listed.mp3"), track("orphan.mp3")]);
        music.play_tracks(vec![track("queued.mp3")], 0);
        let keep = vec![path("copying.mp3")];
        assert_eq!(music.sweep_imported(path(""), keep.clone()), 0, "everything's used");

        music.remove_track(id, 1);
        assert_eq!(music.sweep_imported(path(""), keep.clone()), 1);
        assert!(!dir.join("orphan.mp3").exists() && dir.join("listed.mp3").exists());
        music.delete_playlist(id);
        music.stop();
        assert_eq!(music.sweep_imported(path(""), keep), 2, "the playlist's and the queue's");
        assert!(dir.join("copying.mp3").exists() && dir.join("folder/inner.mp3").exists());
        assert_eq!(music.sweep_imported("relative".into(), vec![]), 0);
        drop(music);
        drop(engine);
    }

    #[test]
    fn playlists_through_the_api() {
        let _turn = one_engine_at_a_time();
        let (engine, music, _) = music(8.0);

        let id = music.create_playlist("  Mix ".into());
        let blank = music.create_playlist(String::new());
        assert_eq!(music.playlist(blank).unwrap().name, format!("Playlist {blank}"));
        music.add_tracks(id, vec![link(1), link(2), file("a")]);
        music.move_track(id, 2, 0);
        music.remove_track(id, 1);
        music.rename_playlist(id, "Road trip".into());
        music.rename_playlist(id, "   ".into());
        let p = music.playlist(id).unwrap();
        assert_eq!(p.name, "Road trip");
        assert_eq!(p.tracks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["A", "Song 2"]);
        assert_eq!(p.tracks[1], link(2), "round trip");
        assert_eq!(p.duration_secs, 60.0);
        let summary = music.playlists().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!((summary.track_count, summary.thumbnail.as_deref()), (2, Some("https://img/2.jpg")));
        assert!(music.playlist(9999).is_none());

        let saved = test_dirs().join("data").join("playlists.json");
        wait_for("the playlists to be saved", || {
            std::fs::read_to_string(&saved).is_ok_and(|s| s.contains("Road trip"))
        });

        music.play_playlist(id, 1, false);
        let s = music.snapshot();
        assert_eq!((s.playlist_id, s.queue_index, s.queue_length), (Some(id), Some(1), 2));
        music.play_playlist(id, 0, true);
        assert!(music.snapshot().shuffle);
        music.set_shuffle(false);
        music.stop();

        // Making tracks.
        let dir = test_dirs().join("scan");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("song.mp3"), b"").unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        let found = music.scan_files(vec![dir.to_string_lossy().into_owned()]);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].kind, found[0].title.as_str(), found[0].offline), (TrackKind::File, "song", false));
        assert!(matches!(music.tracks_for_link("  ".into()), Err(RedlineError::Invalid(_))));
        assert!(matches!(music.tracks_for_link("https://x/y".into()), Err(RedlineError::ToolsMissing(_))));
        assert!(matches!(music.download_for_offline(link(1)), Err(RedlineError::ToolsMissing(_))));
        assert!(music.track_for_history(99).is_none());
        assert!(matches!(music.play_history(99), Err(RedlineError::Invalid(_))));

        // Player settings are clamped and kept in the engine's settings.
        music.set_volume(3.0);
        music.set_speed(0.1);
        music.set_eq(EqRecord { enabled: false, preamp_db: -40.0, bands_db: vec![20.0, -3.0] });
        let ps = music.player_settings();
        assert_eq!((ps.volume, ps.speed, ps.eq.enabled), (1.0, 0.5, false));
        assert_eq!(ps.eq.preamp_db, dsp::PREAMP_MIN_DB);
        assert_eq!(&ps.eq.bands_db[..3], [dsp::BAND_DB, -3.0, 0.0]);
        assert_eq!(ps.eq.bands_db.len(), 10);
        let bass = player_choices().eq_presets.into_iter().find(|p| p.name == "Bass boost").unwrap();
        music.set_eq(EqRecord { enabled: true, preamp_db: bass.preamp_db, bands_db: bass.bands_db });
        assert_eq!(music.player_settings().eq_preset.as_deref(), Some("Bass boost"));
        music.set_eq(EqRecord { enabled: true, preamp_db: 0.0, bands_db: vec![0.0; 10] });
        music.set_volume(0.8);
        music.set_speed(1.0);

        music.delete_playlist(id);
        music.delete_playlist(blank);
        assert!(music.playlists().iter().all(|p| p.id != id && p.id != blank));
        drop(music);
        drop(engine);
        let saved = std::fs::read_to_string(&saved).unwrap();
        assert!(!saved.contains("Road trip"), "the deletion was saved");
    }
}
