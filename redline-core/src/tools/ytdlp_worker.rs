//! yt-dlp kept running between stream look-ups. Starting yt-dlp and loading its extractors
//! takes about as long as looking a link up, so the player asks a running one instead of
//! starting a new one per song. [`warm`] starts one at app launch, so the first song doesn't
//! pay for it either.
//!
//! yt-dlp has no mode for that, so a small extractor plugin ([`PLUGIN`]) provides one: yt-dlp
//! is started on the made-up link `redline-worker:`, and the plugin reads links from stdin,
//! one per line, and answers each with one JSON line of the info `-J` would have printed, or
//! the error. It exits when stdin closes, so a worker never outlives the app. The reply is
//! trimmed to what playback needs: a full look-up is 88 KB to 650 KB, nearly all of it
//! formats, subtitles and comments.
//!
//! With QuickJS (Android's) the plugin also keeps YouTube's player, once made ready for the
//! JS challenge solver, in memory for the next link and in yt-dlp's cache for the next
//! worker. yt-dlp leaves that cache off (its files are big and never cleaned up); the plugin
//! turns it on and prunes the players to 32 MB and 14 days. Without it QuickJS takes seconds
//! to go through the whole player for every song, and again after each launch.
//!
//! A start that fails (an old yt-dlp without `--plugin-dirs`, a script being replaced under
//! it) is retried: three failures in a row wait a minute rather than turning the worker off
//! for the rest of the session, and an update ([`clear`]) retires the idle workers. Anything
//! else that goes wrong makes [`extract`] return `None`, and the caller runs yt-dlp once as
//! before.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, Once};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// The plugin, written to [`plugin_dir`] for yt-dlp's `--plugin-dirs`.
const PLUGIN: &str = include_str!("ytdlp_worker.py");

/// The made-up link the plugin's extractor claims.
const WORKER_LINK: &str = "redline-worker:";

/// A worker unused this long goes, so an idle app doesn't keep a yt-dlp around. Longer on
/// Android, where starting one and solving YouTube's first challenge takes about ten seconds.
const MAX_IDLE: Duration =
    Duration::from_secs(if cfg!(target_os = "android") { 15 * 60 } else { 5 * 60 });

/// A worker this old goes once it's idle, so a long session picks up a yt-dlp update and
/// doesn't keep whatever memory an old one has grown to.
const MAX_AGE: Duration = Duration::from_secs(30 * 60);

/// Idle workers kept for later; more than this at once are only from look-ups that overlapped.
const MAX_POOLED: usize = 2;

/// After this many workers fail to start in a row, starts wait for a while before trying
/// again. It usually means the yt-dlp is too old for `--plugin-dirs`, which an update fixes;
/// it can also be an update replacing the script under a start.
const MAX_FAILED_STARTS: u32 = 3;

/// How long a run of failed starts closes the door before one more try.
const RETRY_AFTER_FAILURES: Duration = Duration::from_secs(60);

static POOL: Mutex<Vec<Worker>> = Mutex::new(Vec::new());
static REAPER: Once = Once::new();
static FAILED: Mutex<Failures> = Mutex::new(Failures { count: 0, since: None });

struct Worker {
    /// The command it was started with; a look-up with other settings needs another worker.
    key: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    started: Instant,
    last_used: Instant,
}

impl Worker {
    fn fresh(&self, now: Instant) -> bool {
        now.duration_since(self.last_used) < MAX_IDLE && now.duration_since(self.started) < MAX_AGE
    }

    /// Sends a link and waits for its answer. `None` if the worker died.
    fn ask(&mut self, url: &str) -> Option<Result<Value, String>> {
        writeln!(self.stdin, "{url}").ok()?;
        self.stdin.flush().ok()?;
        loop {
            match read_reply(&mut self.stdout)? {
                Reply::Ready => continue,
                Reply::Ok(info) => return Some(Ok(info)),
                Reply::Error(e) => return Some(Err(e)),
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        super::kill_tree(&mut self.child);
        let _ = self.child.wait();
    }
}

#[derive(Debug, PartialEq)]
enum Reply {
    Ready,
    Ok(Value),
    Error(String),
}

/// The plugin's next answer, skipping lines that aren't one. `None` once the worker is gone.
fn read_reply(r: &mut impl BufRead) -> Option<Reply> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if r.read_until(b'\n', &mut buf).ok()? == 0 {
            return None;
        }
        let Ok(Value::Object(mut obj)) = serde_json::from_slice::<Value>(&buf) else {
            continue;
        };
        if obj.contains_key("ready") {
            return Some(Reply::Ready);
        }
        if let Some(info) = obj.remove("ok") {
            return Some(Reply::Ok(info));
        }
        if let Some(e) = obj.remove("error") {
            return Some(Reply::Error(e.as_str().map_or_else(|| e.to_string(), str::to_owned)));
        }
    }
}

/// Starts that failed in a row, and when the latest run of them began. A run closes the door
/// for `RETRY_AFTER_FAILURES`: the yt-dlp may be half-updated or too old, and both fix
/// themselves without keeping a worker off for the rest of the session.
#[derive(Default)]
struct Failures {
    count: u32,
    since: Option<Instant>,
}

impl Failures {
    /// Whether a start may be tried now, opening the door again after the wait.
    fn blocked(&mut self, now: Instant) -> bool {
        if self.count < MAX_FAILED_STARTS {
            return false;
        }
        match self.since {
            Some(t) if now.duration_since(t) < RETRY_AFTER_FAILURES => true,
            _ => {
                self.count = 0;
                self.since = None;
                false
            }
        }
    }

    fn failed(&mut self, now: Instant) {
        self.count += 1;
        if self.count >= MAX_FAILED_STARTS {
            self.since = Some(now);
        }
    }

    fn worked(&mut self) {
        self.count = 0;
        self.since = None;
    }
}

fn starts_blocked() -> bool {
    FAILED.lock().unwrap().blocked(Instant::now())
}

fn start_failed() {
    FAILED.lock().unwrap().failed(Instant::now());
}

fn start_worked() {
    FAILED.lock().unwrap().worked();
}

/// Forgets failed starts, e.g. after yt-dlp was updated. The next look-up (or warm-up) tries
/// the worker again.
pub(crate) fn reset() {
    FAILED.lock().unwrap().worked();
}

/// Retires the idle workers and forgets failed starts: after an update, the next look-up
/// should run the new yt-dlp, not one still running from the old script.
pub(crate) fn clear() {
    let workers = {
        let mut pool = POOL.lock().unwrap();
        std::mem::take(&mut *pool)
    };
    retire(workers);
    reset();
}

fn ensure_reaper() {
    REAPER.call_once(|| {
        thread::spawn(reap);
    });
}

/// How a worker started with `cmd` is recognized: the program, its arguments and its
/// environment, so a look-up with other settings gets a worker of its own.
fn key_of(cmd: &Command) -> String {
    format!(
        "{:?} {:?} {:?}",
        cmd.get_program(),
        cmd.get_args().collect::<Vec<_>>(),
        cmd.get_envs().collect::<Vec<_>>()
    )
}

/// Starts a worker for `cmd` and keeps it for the next look-up. This waits for yt-dlp and its
/// plugin to start, so call it on a thread of its own; a fresh worker with the same settings is
/// reused rather than started twice. Warming at app start keeps Python, yt-dlp and extractor
/// startup off the first song's way.
pub(crate) fn warm(cmd: Command) {
    let key = key_of(&cmd);
    ensure_reaper();
    {
        let now = Instant::now();
        let pool = POOL.lock().unwrap();
        if pool.iter().any(|w| w.key == key && w.fresh(now)) {
            return;
        }
    }
    if starts_blocked() {
        return;
    }
    match start(cmd, key) {
        Some(w) => {
            start_worked();
            put_back(w);
        }
        None => start_failed(),
    }
}

/// Looks `url` up with a worker started from `cmd` (yt-dlp with its options, but no link).
/// `Some(Err)` is yt-dlp's own error text; `None` means no worker could answer, and the caller
/// should run yt-dlp itself.
pub(crate) fn extract(cmd: Command, url: &str) -> Option<Result<Value, String>> {
    // A link is one stdin line.
    if url.is_empty() || url.contains(['\r', '\n']) {
        return None;
    }
    ensure_reaper();
    let key = key_of(&cmd);
    let mut worker = match take(&key) {
        Some(w) => w,
        // A run of failed starts closes the door for a while; a worker already running is
        // still worth asking.
        None if starts_blocked() => return None,
        None => match start(cmd, key) {
            Some(w) => w,
            None => {
                start_failed();
                return None;
            }
        },
    };
    match worker.ask(url) {
        Some(reply) => {
            start_worked();
            put_back(worker);
            Some(reply)
        }
        None => {
            retire(vec![worker]);
            None
        }
    }
}

/// An idle worker started with `key`. The rest go if they're stale or from other settings.
fn take(key: &str) -> Option<Worker> {
    let now = Instant::now();
    let (found, stale) = {
        let mut pool = POOL.lock().unwrap();
        let (usable, stale): (Vec<_>, Vec<_>) = std::mem::take(&mut *pool)
            .into_iter()
            .partition(|w| w.key == key && w.fresh(now));
        *pool = usable;
        (pool.pop(), stale)
    };
    retire(stale);
    found
}

fn put_back(mut worker: Worker) {
    worker.last_used = Instant::now();
    let mut pool = POOL.lock().unwrap();
    if pool.len() < MAX_POOLED {
        pool.push(worker);
    } else {
        drop(pool);
        retire(vec![worker]);
    }
}

/// Stops workers on a thread of their own: on Windows that's a `taskkill`, which the caller
/// shouldn't wait for.
fn retire(workers: Vec<Worker>) {
    if !workers.is_empty() {
        thread::spawn(move || drop(workers));
    }
}

/// Stops idle workers once they're stale, even if nothing looks a link up again.
fn reap() {
    loop {
        thread::sleep(Duration::from_secs(60));
        let now = Instant::now();
        let stale: Vec<_> = {
            let mut pool = POOL.lock().unwrap();
            let (fresh, stale) = std::mem::take(&mut *pool).into_iter().partition(|w| w.fresh(now));
            *pool = fresh;
            stale
        };
        drop(stale);
    }
}

/// Starts a worker and waits until it's ready for links.
fn start(mut cmd: Command, key: String) -> Option<Worker> {
    let dir = plugin_dir().ok()?;
    // Its errors would come colored: Windows' null device counts as a terminal.
    cmd.args(["--color", "never", "--plugin-dirs"])
        .arg(dir)
        .args(["--", WORKER_LINK])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
    let now = Instant::now();
    let mut worker = Worker {
        key,
        stdin: stdin?,
        stdout: BufReader::new(stdout?),
        child,
        started: now,
        last_used: now,
    };
    if read_reply(&mut worker.stdout) == Some(Reply::Ready) {
        Some(worker)
    } else {
        retire(vec![worker]);
        None
    }
}

/// The folder for `--plugin-dirs`, with the plugin in it. yt-dlp only loads plugins from a
/// package inside the folder, hence `redline/yt_dlp_plugins/extractor/`.
fn plugin_dir() -> std::io::Result<PathBuf> {
    let dir = super::temp_dir().join("redline-ytdlp-plugin");
    let file = dir.join("redline").join("yt_dlp_plugins").join("extractor").join("redline_worker.py");
    let current = || std::fs::read_to_string(&file).is_ok_and(|s| s == PLUGIN);
    if !current() {
        std::fs::create_dir_all(file.parent().expect("has a parent"))?;
        // Written aside and renamed into place, so another Redline starting a worker right now
        // never loads half a file.
        let tmp = file.with_extension(format!("py.{}", std::process::id()));
        std::fs::write(&tmp, PLUGIN)?;
        if let Err(e) = std::fs::rename(&tmp, &file) {
            let _ = std::fs::remove_file(&tmp);
            // Windows refuses while another process has it open; that one wrote it already.
            if !current() {
                return Err(e);
            }
        }
    }
    Ok(dir)
}

/// The idle workers' process ids, for tests.
#[cfg(test)]
pub(crate) fn idle_pids() -> Vec<u32> {
    POOL.lock().unwrap().iter().map(|w| w.child.id()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_skip_other_lines() {
        let out = b"null\n\nnot json\n[1]\n{\"ready\":true}\r\n{\"other\":1}\n{\"ok\":{\"title\":\"T\"}}\n\
                    {\"error\":\"ERROR: [youtube] x: Video unavailable\"}\n{\"error\":5}\n{\"ok\":";
        let mut r = &out[..];
        assert_eq!(read_reply(&mut r), Some(Reply::Ready));
        assert_eq!(read_reply(&mut r), Some(Reply::Ok(serde_json::json!({"title": "T"}))));
        assert_eq!(
            read_reply(&mut r),
            Some(Reply::Error("ERROR: [youtube] x: Video unavailable".into()))
        );
        assert_eq!(read_reply(&mut r), Some(Reply::Error("5".into())));
        // A worker that died halfway through a line.
        assert_eq!(read_reply(&mut r), None);
        assert_eq!(read_reply(&mut r), None);
    }

    #[test]
    fn failed_starts_back_off_then_try_again() {
        let mut f = Failures::default();
        let now = Instant::now();
        assert!(!f.blocked(now), "nothing has failed yet");
        for _ in 0..MAX_FAILED_STARTS {
            f.failed(now);
        }
        assert!(f.blocked(now), "three failures in a row wait");
        assert!(f.blocked(now + RETRY_AFTER_FAILURES - Duration::from_secs(1)));
        assert!(!f.blocked(now + RETRY_AFTER_FAILURES), "and then try again");
        assert!(!f.blocked(now + RETRY_AFTER_FAILURES), "the wait was taken once");
        f.failed(now);
        f.failed(now);
        f.failed(now);
        assert!(f.blocked(now));
        f.worked();
        assert!(!f.blocked(now), "a working start clears the streak");
    }

    #[test]
    fn links_that_are_not_one_line_are_left_to_the_caller() {
        let cmd = || Command::new("yt-dlp-that-does-not-exist");
        assert_eq!(extract(cmd(), ""), None);
        assert_eq!(extract(cmd(), "https://a\nhttps://b"), None);
    }
}
