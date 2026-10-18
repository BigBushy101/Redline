//! The engine Kotlin holds: redline-core's queue, history and settings behind one lock, with a
//! thread that runs the queue the way the desktop's frame loop does and tells Kotlin when
//! something changed. Kotlin then pulls snapshots; nothing is pushed across but the nudge.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use redline_core::history::History;
use redline_core::jobs::{JobManager, JobSpec, JobStatus};
use redline_core::settings::Settings;
use redline_core::tools::ToolPaths;
use redline_core::tools::install;
use redline_core::tools::ytdlp::{self, MediaInfo};

use crate::error::RedlineError;
use crate::records::*;

/// Kotlin's side of the nudge.
#[uniffi::export(with_foreign)]
pub trait EngineListener: Send + Sync {
    /// The queue, history or finished jobs changed; pull what you show. Called on Redline's
    /// own thread, at most ten times a second, so hop to the main thread and don't block.
    fn on_change(&self);
}

/// Most `on_change` calls per second is 1 / this. Progress lines arrive far faster.
const THROTTLE: Duration = Duration::from_millis(100);
/// Longest the queue thread sleeps with nothing to do. Workers and calls wake it sooner.
const IDLE: Duration = Duration::from_secs(30);
/// Looked-up links kept for planning their download (formats, tags, length).
const LOOKUPS: usize = 8;
/// Finished jobs kept for `drain_finished` if Kotlin never drains.
const MAX_FINISHED: usize = 200;

/// The engine's copy of the desktop app's state for this slice.
struct State {
    settings: Settings,
    saved_settings: Settings,
    tools: ToolPaths,
    jobs: JobManager,
    history: History,
    /// Recent lookups, newest last, by trimmed link.
    lookups: VecDeque<(String, MediaInfo)>,
    /// Jobs that ended and haven't been drained.
    finished: Vec<FinishedRecord>,
}

impl State {
    fn lookup(&self, url: &str) -> Option<&MediaInfo> {
        self.lookups.iter().rev().find(|(u, _)| u == url).map(|(_, i)| i)
    }

    fn remember(&mut self, url: String, info: MediaInfo) {
        self.lookups.retain(|(u, _)| *u != url);
        if self.lookups.len() == LOOKUPS {
            self.lookups.pop_front();
        }
        self.lookups.push_back((url, info));
    }

    /// One turn of the desktop's `logic`: apply worker events, start jobs, record what ended
    /// and save. Returns when to come back for a scheduled job, and whether anything changed
    /// that no worker wake-up announced.
    fn step(&mut self) -> (Option<Duration>, bool) {
        let queued = |jobs: &JobManager| jobs.jobs.iter().filter(|j| j.status == JobStatus::Queued).count();
        let before = queued(&self.jobs);
        let next = self.jobs.pump(&self.tools, self.settings.concurrency);
        let mut changed = queued(&self.jobs) != before;
        for id in self.jobs.take_finished() {
            if let Some(job) = self.jobs.get(id) {
                self.history.record(job);
                self.finished.push(FinishedRecord { job: job_record(job) });
                changed = true;
            }
        }
        let over = self.finished.len().saturating_sub(MAX_FINISHED);
        self.finished.drain(..over);
        self.jobs.save_if_dirty();
        self.history.save_if_dirty();
        if self.settings != self.saved_settings {
            self.settings.save();
            self.saved_settings = self.settings.clone();
        }
        (next, changed)
    }
}

/// Wakes the queue thread: workers poke it on progress, API calls after changing the queue.
#[derive(Default)]
struct Signal {
    flags: Mutex<Flags>,
    cv: Condvar,
}

#[derive(Default, Clone, Copy)]
struct Flags {
    poked: bool,
    stop: bool,
}

impl Signal {
    fn poke(&self) {
        self.flags.lock().unwrap().poked = true;
        self.cv.notify_one();
    }

    fn stop(&self) {
        self.flags.lock().unwrap().stop = true;
        self.cv.notify_one();
    }

    /// Waits up to `timeout` (for a stop, or a poke if `pokes`), and takes the poke.
    fn wait(&self, timeout: Duration, pokes: bool) -> Flags {
        let flags = self.flags.lock().unwrap();
        let (mut flags, _) = self
            .cv
            .wait_timeout_while(flags, timeout, |f| !f.stop && !(pokes && f.poked))
            .unwrap();
        let seen = *flags;
        flags.poked = false;
        seen
    }
}

struct Shared {
    state: Mutex<State>,
    signal: Arc<Signal>,
    listener: Arc<dyn EngineListener>,
}

/// The queue thread: the desktop's frame loop without the frames. Coalesces wake-ups so
/// Kotlin hears at most one `on_change` per [`THROTTLE`].
fn run(shared: Arc<Shared>) {
    let mut pending = true; // Kotlin wants a first snapshot
    let mut last_notify: Option<Instant> = None;
    let mut wait = Duration::ZERO;
    let mut throttled = false;
    loop {
        // While throttled, pokes only mean "more changes"; the deadline still holds.
        let flags = shared.signal.wait(wait, !throttled);
        if flags.stop {
            break;
        }
        let (next, changed) = shared.state.lock().unwrap().step();
        pending |= flags.poked || changed;
        let since = last_notify.map_or(THROTTLE, |t| t.elapsed());
        if pending && since >= THROTTLE {
            pending = false;
            last_notify = Some(Instant::now());
            shared.listener.on_change();
        }
        wait = next.unwrap_or(IDLE).min(IDLE);
        throttled = pending;
        if pending {
            wait = wait.min(THROTTLE.saturating_sub(since));
        }
    }
}

/// Redline's working parts for the Android app. Make one per process (in the `Application`)
/// and keep it; a second one would load and run the same queue again.
#[derive(uniffi::Object)]
pub struct Engine {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// Points redline-core at the app's folders and tools.
#[cfg(target_os = "android")]
fn init(paths: &AndroidPathsRecord) -> anyhow::Result<ToolPaths> {
    redline_core::android::init(paths.to_core())
}

/// Desktop hosts (tests, trying the engine out) only get the folders; the tools are whatever
/// [`ToolPaths::locate`] finds there or on `PATH`.
#[cfg(not(target_os = "android"))]
fn init(paths: &AndroidPathsRecord) -> anyhow::Result<ToolPaths> {
    redline_core::settings::set_app_dirs(redline_core::settings::AppDirs {
        data: paths.data_dir.clone().into(),
        downloads: paths.download_dir.clone().into(),
    })?;
    Ok(ToolPaths::locate(&Settings::load()))
}

#[uniffi::export]
impl Engine {
    /// Sets up redline-core with the app's folders and tools, restores the queue (jobs that
    /// were running start over) and starts running it. Call after youtubedl-android's init.
    #[uniffi::constructor]
    pub fn new(paths: AndroidPathsRecord, listener: Arc<dyn EngineListener>) -> Result<Arc<Self>, RedlineError> {
        let tools = init(&paths).map_err(|e| RedlineError::Invalid(format!("{e:#}")))?;
        Ok(Self::start(tools, listener))
    }

    /// Which tools Redline found and their readiness. Blocks while `yt-dlp --version` runs.
    pub fn tools_status(&self) -> ToolsStatusRecord {
        let tools = self.state().tools.clone();
        let version = install::ytdlp_version(&tools);
        tools_status(&tools, version)
    }

    /// `yt-dlp -U`; returns its last line. Blocks for as long as the update takes.
    pub fn update_ytdlp(&self) -> Result<String, RedlineError> {
        let tools = self.state().tools.clone();
        if tools.ytdlp.is_none() {
            return Err(RedlineError::ToolsMissing("yt-dlp is not installed.".into()));
        }
        install::update_ytdlp(&tools).map_err(RedlineError::failed)
    }

    /// Looks up a link (title, formats, playlist entries). Blocks while yt-dlp runs. The
    /// result is kept so `queue_download` of the same link can use its picks and tags.
    pub fn fetch_info(&self, url: String) -> Result<MediaInfoRecord, RedlineError> {
        let url = url.trim().to_owned();
        if url.is_empty() {
            return Err(RedlineError::Invalid("Paste a link first.".into()));
        }
        let (tools, net) = {
            let st = self.state();
            (st.tools.clone(), st.settings.network.clone())
        };
        if tools.ytdlp.is_none() {
            return Err(RedlineError::ToolsMissing("Looking up links needs yt-dlp.".into()));
        }
        // fetch_info already adds friendly_error's hints.
        let info = ytdlp::fetch_info(&tools, &net, &url).map_err(RedlineError::failed)?;
        let record = media_info_record(&url, &info);
        self.state().remember(url, info);
        Ok(record)
    }

    /// Queues a download the way the desktop's Download button does, and remembers the
    /// options as the new defaults. Returns the job's id.
    pub fn queue_download(&self, choices: DownloadChoicesRecord) -> Result<u64, RedlineError> {
        let url = choices.url.trim().to_owned();
        if url.is_empty() {
            return Err(RedlineError::Invalid("Paste a link first.".into()));
        }
        let id = {
            let mut st = self.state();
            if !st.tools.ready_for_download() {
                return Err(RedlineError::ToolsMissing("Downloading needs yt-dlp and ffmpeg.".into()));
            }
            let info = st.lookup(&url).cloned();
            let duration = info.as_ref().and_then(|i| i.duration);
            let link = link_choices(info, &choices);
            let trim = if link.is_playlist() {
                None
            } else {
                check_trim(choices.trim, duration).map_err(RedlineError::Invalid)?
            };
            st.settings.download = (&choices.options).into();
            let plan = link.plan(&url, trim, &st.settings);
            st.jobs.push_at(plan.title, plan.subtitle, plan.spec, choices.not_before);
            st.jobs.jobs.last().map_or(0, |j| j.id)
        };
        self.shared.signal.poke();
        Ok(id)
    }

    /// The queue, newest first (as the desktop lists it).
    pub fn queue_snapshot(&self) -> Vec<JobRecord> {
        self.state().jobs.jobs.iter().rev().map(job_record).collect()
    }

    /// Jobs queued or running.
    pub fn active_count(&self) -> u32 {
        self.state().jobs.active_count() as u32
    }

    pub fn cancel(&self, id: u64) {
        self.change(|st| st.jobs.cancel(id));
    }

    pub fn cancel_all(&self) {
        self.change(|st| st.jobs.cancel_all());
    }

    /// Runs a finished job again.
    pub fn retry(&self, id: u64) {
        self.change(|st| st.jobs.retry(id));
    }

    /// "Download anyway" for a job skipped as already downloaded.
    pub fn retry_ignoring_archive(&self, id: u64) {
        self.change(|st| st.jobs.retry_ignoring_archive(id));
    }

    /// Drops a finished job from the list; active ones stay.
    pub fn remove(&self, id: u64) {
        self.change(|st| st.jobs.remove(id));
    }

    pub fn clear_finished(&self) {
        self.change(|st| st.jobs.clear_finished());
    }

    /// Jobs that ended since the last call, for notifications.
    pub fn drain_finished(&self) -> FinishedBatchRecord {
        let mut st = self.state();
        let jobs = std::mem::take(&mut st.finished);
        finished_batch(jobs, st.jobs.active_count(), st.settings.notify)
    }

    /// Finished jobs, newest first.
    pub fn history(&self) -> Vec<HistoryRecord> {
        self.state().history.entries.iter().enumerate().map(|(i, e)| history_record(i, e)).collect()
    }

    pub fn remove_history(&self, index: u32) {
        self.change(|st| st.history.remove(index as usize));
    }

    pub fn clear_history(&self) {
        self.change(|st| st.history.clear());
    }

    /// Queues a history entry again, past the download archive since it was asked for.
    /// Returns the new job's id.
    pub fn download_again(&self, index: u32) -> Result<u64, RedlineError> {
        let id = {
            let mut st = self.state();
            let entry = st
                .history
                .entries
                .get(index as usize)
                .cloned()
                .ok_or_else(|| RedlineError::Invalid("That history entry is gone.".into()))?;
            let mut spec = entry.spec;
            if let JobSpec::Download(r) = &mut spec {
                r.archive = None;
            }
            st.jobs.push(entry.title, entry.subtitle, spec);
            st.jobs.jobs.last().map_or(0, |j| j.id)
        };
        self.shared.signal.poke();
        Ok(id)
    }

    pub fn settings(&self) -> SettingsRecord {
        settings_record(&self.state().settings)
    }

    /// Writes back the fields [`SettingsRecord`] covers; saved on the queue thread.
    pub fn set_settings(&self, settings: SettingsRecord) {
        self.change(|st| apply_settings(&settings, &mut st.settings));
    }
}

impl Engine {
    /// Starts the engine with `tools` (the constructor's, or fakes in tests). Expects the app's
    /// folders to be set already.
    fn start(tools: ToolPaths, listener: Arc<dyn EngineListener>) -> Arc<Self> {
        let signal = Arc::new(Signal::default());
        let mut jobs = JobManager::load();
        let poke = signal.clone();
        jobs.set_wake(Arc::new(move || poke.poke()));
        let settings = Settings::load();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                saved_settings: settings.clone(),
                settings,
                tools,
                jobs,
                history: History::load(),
                lookups: VecDeque::new(),
                finished: Vec::new(),
            }),
            signal,
            listener,
        });
        let runner = shared.clone();
        let thread = thread::Builder::new()
            .name("redline-queue".into())
            .spawn(move || run(runner))
            .expect("can't start the queue thread");
        Arc::new(Self { shared, thread: Mutex::new(Some(thread)) })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap()
    }

    /// Changes the state and lets the queue thread act on it (start jobs, save, tell Kotlin).
    fn change(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state());
        self.shared.signal.poke();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.signal.stop();
        if let Some(t) = self.thread.lock().unwrap().take()
            // Kotlin could drop the last reference from inside on_change.
            && t.thread().id() != thread::current().id()
        {
            let _ = t.join();
        }
        // Don't leave yt-dlp/ffmpeg running, as the desktop app doesn't when it closes.
        let mut st = self.state();
        st.jobs.cancel_all();
        st.jobs.save_if_dirty();
        st.history.save_if_dirty();
        if st.settings != st.saved_settings {
            st.settings.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The process-wide folders every test in this crate uses, so nothing touches the real
    /// `%APPDATA%\Redline`.
    fn test_dirs() -> PathBuf {
        static DIR: OnceLock<PathBuf> = OnceLock::new();
        DIR.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("redline-mobile-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            redline_core::settings::set_app_dirs(redline_core::settings::AppDirs {
                data: dir.join("data"),
                downloads: dir.join("downloads"),
            })
            .unwrap();
            dir
        })
        .clone()
    }

    #[derive(Default)]
    struct Counter(AtomicUsize);

    impl EngineListener for Counter {
        fn on_change(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn signal_wakes_on_poke_unless_told_not_to() {
        let s = Signal::default();
        s.poke();
        assert!(s.wait(Duration::from_secs(5), true).poked);
        s.poke();
        let start = Instant::now();
        let f = s.wait(Duration::from_millis(50), false);
        assert!(f.poked && start.elapsed() >= Duration::from_millis(45), "slept through the poke");
        s.stop();
        assert!(s.wait(Duration::from_secs(5), false).stop);
    }

    #[test]
    fn runs_a_download_through_the_queue() {
        let dir = test_dirs();
        // Tools that don't exist: the job starts, fails, and comes out the other end.
        let missing = dir.join("no-such-tool");
        let tools = ToolPaths { ytdlp: Some(missing.clone()), ffmpeg: Some(missing.clone()), ffprobe: Some(missing), ..Default::default() };
        let listener = Arc::new(Counter::default());
        let engine = Engine::start(tools, listener.clone());
        wait_for("the first snapshot nudge", || listener.0.load(Ordering::SeqCst) > 0);

        let mut settings = engine.settings();
        settings.concurrency = 3;
        engine.set_settings(settings.clone());
        assert_eq!(engine.settings(), settings);

        let mut choices = DownloadChoicesRecord {
            url: "  https://example.com/v  ".into(),
            options: settings.download.clone(),
            pick_video: None,
            pick_audio: None,
            tags: None,
            trim: Some(TrimRecord { start_secs: 9.0, end_secs: Some(3.0), precise: false }),
            not_before: None,
        };
        assert!(matches!(engine.queue_download(choices.clone()), Err(RedlineError::Invalid(_))));
        choices.trim = None;
        let id = engine.queue_download(choices).unwrap();

        wait_for("the job to fail", || engine.queue_snapshot().iter().any(|j| j.id == id && j.state == JobState::Failed));
        let job = engine.queue_snapshot().into_iter().find(|j| j.id == id).unwrap();
        assert_eq!(job.url.as_deref(), Some("https://example.com/v"));
        assert!(job.error.is_some());
        wait_for("the finished batch", || {
            let b = engine.drain_finished();
            !b.jobs.is_empty() && b.announcement.is_some_and(|a| a.title == "Failed")
        });
        assert!(engine.drain_finished().jobs.is_empty(), "drained");
        assert!(engine.history().is_empty(), "failures stay out of history");

        engine.retry(id);
        wait_for("the retry to fail again", || engine.drain_finished().jobs.len() == 1);
        engine.remove(id);
        assert!(engine.queue_snapshot().iter().all(|j| j.id != id));
        assert!(matches!(engine.download_again(99), Err(RedlineError::Invalid(_))));

        drop(engine);
        let saved = std::fs::read_to_string(dir.join("data").join("settings.json")).unwrap();
        assert!(saved.contains("\"concurrency\": 3"), "settings saved");
    }
}
