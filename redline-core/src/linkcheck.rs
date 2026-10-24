//! Whether saved links still play, found out before they're played, and stand-ins for the
//! ones that don't.
//!
//! YouTube links are checked with its oEmbed endpoint (one small HTTP request, no yt-dlp);
//! other sites with a `yt-dlp --simulate` run, less often. Playlist listings already mark
//! deleted and private entries, which [`ytdlp::fetch_info`] records here as it reads them.
//! Results are kept in `linkcheck.json` for about a week. A check that gets no answer is
//! asked again later, and the checks slow down while a site is turning them away (HTTP 429).
//!
//! oEmbed only knows whether a video is there. Uploads that are there but won't play here
//! (YouTube's "This video is not available": blocked in this country, or for licensing) look
//! fine to it; only yt-dlp, which is a full lookup per link, sees them. So links that fail to
//! play are recorded too ([`PlayWatch`]), and that sticks: oEmbed saying they're there doesn't
//! undo it, and they're checked again with yt-dlp.
//!
//! YouTube Music swaps a dead upload for another one of the same song from its own catalogue,
//! which outside apps can't see; [`search`] and [`rank`] get close with a title, artist and
//! length search.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::library::{NowPlaying, PlaylistId, Track};
use crate::platform::{self, Platform};
use crate::player::{PlayerState, Source, Status};
use crate::settings;
use crate::tools::ytdlp::{self, NetworkOptions, PlaylistItem};
use crate::tools::{self, ToolPaths};
use crate::{Wake, no_wake};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LinkStatus {
    /// Not checked yet, or the check couldn't tell (no network).
    #[default]
    Unknown,
    Alive,
    /// There but not playable for everyone: private, or blocked from embedding.
    Restricted,
    /// Deleted, or never there.
    Dead,
}

impl LinkStatus {
    pub fn is_dead(self) -> bool {
        self == LinkStatus::Dead
    }

    /// What a row's tooltip or long-press label says, for the statuses rows show.
    pub fn label(self) -> Option<&'static str> {
        match self {
            LinkStatus::Dead => Some("This video is no longer available"),
            LinkStatus::Restricted => Some("This video is private or can't be played here"),
            _ => None,
        }
    }
}

/// What one check heard back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// What the site said about the link. `Unknown` counts as no answer.
    Status(LinkStatus),
    /// Nothing that tells (no network, a timeout, a server error): ask again later.
    NoAnswer,
    /// The site asked for fewer requests (HTTP 429, a bot check): ask again later, more slowly.
    SlowDown,
}

impl From<LinkStatus> for Answer {
    fn from(s: LinkStatus) -> Self {
        if s == LinkStatus::Unknown { Answer::NoAnswer } else { Answer::Status(s) }
    }
}

/// What an oEmbed reply means. `None` is no reply at all (a network error).
pub fn status_for_oembed(code: Option<u16>) -> LinkStatus {
    match code {
        Some(200) => LinkStatus::Alive,
        Some(400 | 404) => LinkStatus::Dead,
        Some(401 | 403) => LinkStatus::Restricted,
        _ => LinkStatus::Unknown,
    }
}

/// [`status_for_oembed`], telling a rate limit apart from the other replies that say nothing.
pub fn answer_for_oembed(code: Option<u16>) -> Answer {
    match code {
        Some(429) => Answer::SlowDown,
        code => status_for_oembed(code).into(),
    }
}

/// Words of errors that are the site turning this connection away, not anything about the
/// link. YouTube's own rate limit reads "Video unavailable. This content isn't available, try
/// again later".
const RATE_LIMITED: &[&str] = &["429", "too many requests", "not a bot", "rate-limit", "rate limit", "try again later"];

/// What a yt-dlp error (from `--simulate`, or from playing the link) says about the link.
/// Bot checks, rate limits, network trouble and a client YouTube turned away say nothing
/// about it.
pub fn status_for_ytdlp_error(msg: &str) -> LinkStatus {
    let m = msg.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| m.contains(w));
    if any(RATE_LIMITED)
        || any(&[
            "timed out",
            "unable to download",
            "connection",
            "network",
            "getaddrinfo",
            "service unavailable",
            "temporarily unavailable",
            // "This video is unavailable. Error code: 152 - 18" is YouTube refusing yt-dlp's
            // embedded client, which it says of every video.
            "error code",
            "requested format",
            "will begin",
            "premieres in",
        ])
    {
        return LinkStatus::Unknown;
    }
    if any(&["private", "sign in", "log in", "login", "members-only", "members only", "join this channel", "premium"]) {
        return LinkStatus::Restricted;
    }
    if any(&[
        "unavailable",
        "not available",
        "removed",
        "no longer available",
        "does not exist",
        "not found",
        "404",
        "has been terminated",
        "deleted",
        "copyright",
        "in your country",
    ]) {
        return LinkStatus::Dead;
    }
    LinkStatus::Unknown
}

/// [`status_for_ytdlp_error`], telling a rate limit apart from the other errors that say
/// nothing.
pub fn answer_for_ytdlp_error(msg: &str) -> Answer {
    let m = msg.to_lowercase();
    if RATE_LIMITED.iter().any(|w| m.contains(w)) { Answer::SlowDown } else { status_for_ytdlp_error(msg).into() }
}

/// What a flat playlist entry already tells: YouTube lists deleted and private videos under
/// stand-in titles, and some entries carry yt-dlp's `availability`.
pub fn status_from_flat(title: Option<&str>, availability: Option<&str>) -> Option<LinkStatus> {
    match title.map(str::trim) {
        Some("[Deleted video]" | "[Unavailable video]") => return Some(LinkStatus::Dead),
        Some("[Private video]") => return Some(LinkStatus::Restricted),
        _ => {}
    }
    match availability? {
        "private" | "needs_auth" | "premium_only" | "subscriber_only" => Some(LinkStatus::Restricted),
        "public" | "unlisted" => Some(LinkStatus::Alive),
        _ => None,
    }
}

const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const YTDLP_TIMEOUT: Duration = Duration::from_secs(30);

/// Asks YouTube's oEmbed endpoint about a video link.
pub fn check_youtube(url: &str, net: &NetworkOptions) -> Answer {
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false);
    if !net.proxy.trim().is_empty() {
        match ureq::Proxy::new(net.proxy.trim()) {
            Ok(p) => config = config.proxy(Some(p)),
            Err(_) => return Answer::NoAnswer,
        }
    }
    let agent: ureq::Agent = config.build().into();
    let endpoint = format!("https://www.youtube.com/oembed?url={}&format=json", encode(url));
    answer_for_oembed(agent.get(&endpoint).call().ok().map(|r| r.status().as_u16()))
}

/// Percent-encodes a link for a query parameter.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Asks yt-dlp whether a link (any site) would play, giving up after half a minute.
pub fn check_other(tools: &ToolPaths, net: &NetworkOptions, url: &str) -> Answer {
    let Ok(mut cmd) = ytdlp::base_command(tools, net) else { return Answer::NoAnswer };
    cmd.args(["--simulate", "-J", "--no-playlist", "--", url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let Ok(mut child) = cmd.spawn() else { return Answer::NoAnswer };
    let mut stderr = child.stderr.take();
    // Read stderr alongside, so a chatty run can't fill the pipe and stall.
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = std::io::Read::read_to_string(e, &mut s);
        }
        s
    });
    let deadline = Instant::now() + YTDLP_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                tools::kill_tree(&mut child);
                let _ = child.wait();
                break None;
            }
        }
    };
    let err = reader.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => Answer::Status(LinkStatus::Alive),
        Some(_) => {
            let line = err.lines().rev().find(|l| l.starts_with("ERROR:")).unwrap_or(&err);
            answer_for_ytdlp_error(line)
        }
        None => Answer::NoAnswer,
    }
}

/// Checks one link the way its site allows. `deep` asks yt-dlp even about a YouTube link,
/// for one that oEmbed can't tell about (it failed to play).
pub fn check_link(tools: &ToolPaths, net: &NetworkOptions, url: &str, deep: bool) -> Answer {
    if Platform::of(url) == Platform::YouTube && !deep {
        check_youtube(url, net)
    } else {
        check_other(tools, net, url)
    }
}

/// How long a check holds: a week for YouTube, two for links checked through yt-dlp.
fn max_age(url: &str, deep: bool) -> i64 {
    const DAY: i64 = 24 * 60 * 60;
    if Platform::of(url) == Platform::YouTube && !deep { 7 * DAY } else { 14 * DAY }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Checked {
    status: LinkStatus,
    /// Unix seconds.
    at: i64,
    /// Found by yt-dlp or by playing the link, which see more than oEmbed: checked again with
    /// yt-dlp, and not undone by a quick check or a listing saying it's there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deep: bool,
}

/// Puts a result in the cache, unless it's a quick "it's there" (oEmbed, a listing) over a
/// fresh finding that it won't play. `deep` is how it was found. Returns whether it went in.
fn put(cache: &mut HashMap<String, Checked>, key: String, url: &str, status: LinkStatus, deep: bool, at: i64) -> bool {
    let held = cache
        .get(&key)
        .is_some_and(|c| c.deep && c.status != LinkStatus::Alive && at - c.at < max_age(url, true));
    if held && !deep && status == LinkStatus::Alive {
        return false;
    }
    cache.insert(key, Checked { status, at, deep: deep && status != LinkStatus::Alive });
    true
}

/// Asks about one link: tools, network settings, the link, and whether to ask yt-dlp
/// ([`check_link`]'s `deep`).
type CheckFn = dyn Fn(&ToolPaths, &NetworkOptions, &str, bool) -> Answer + Send + Sync;

/// How many links are checked at once.
const WORKERS: usize = 3;
/// Tries in a row a link with no answer gets, before it's left alone for [`Waits::rest`].
const MAX_TRIES: u32 = 5;
/// Answers in a row that tell nothing before the checks pause (a rate limit pauses at once).
const FAILS_BEFORE_PAUSE: u32 = 3;
/// Answers in a row that tell something before the gap between checks halves.
const SPEED_UP_AFTER: u32 = 20;
/// A gap shorter than this goes back to none.
const MIN_GAP: Duration = Duration::from_millis(100);
/// Longest a waiting worker sleeps before looking again.
const TICK: Duration = Duration::from_millis(250);
/// Results are written out at least this often while checks run.
const SAVE_EVERY: Duration = Duration::from_secs(5);

/// How long the checks wait after trouble. Tests use shorter ones.
#[derive(Clone, Copy, Debug)]
struct Waits {
    /// Before asking again about a link that got no answer; doubles with each try.
    retry: Duration,
    /// The first pause after a rate limit or a run of failures; doubles while they go on.
    pause: Duration,
    max_pause: Duration,
    /// Between checks after a rate limit; doubles with each one.
    gap: Duration,
    max_gap: Duration,
    /// How long a link that never got an answer is left alone, even when asked about.
    rest: Duration,
}

const WAITS: Waits = Waits {
    retry: Duration::from_secs(15),
    pause: Duration::from_secs(10),
    max_pause: Duration::from_secs(5 * 60),
    gap: Duration::from_millis(250),
    max_gap: Duration::from_secs(4),
    rest: Duration::from_secs(30 * 60),
};

/// How fast the checks go: paused for a while after a rate limit or a run of failures, and
/// spaced out after a rate limit until things go well for a while.
#[derive(Debug, Default)]
struct Throttle {
    /// No check starts before then.
    pause_until: Option<Instant>,
    /// Least time between the starts of two checks.
    gap: Duration,
    last_start: Option<Instant>,
    /// Answers in a row that told nothing or asked to slow down.
    strikes: u32,
    /// Answers in a row that told something, since the gap last shrank.
    good: u32,
}

impl Throttle {
    /// When the next check may start, if not now.
    fn ready_at(&self) -> Option<Instant> {
        let spaced = self.last_start.map(|t| t + self.gap);
        match (self.pause_until, spaced) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    fn started(&mut self, at: Instant) {
        self.last_start = Some(at);
    }

    fn paused(&self, at: Instant) -> bool {
        self.pause_until.is_some_and(|p| at < p)
    }

    fn went_well(&mut self) {
        self.strikes = 0;
        self.good += 1;
        if self.good >= SPEED_UP_AFTER && !self.gap.is_zero() {
            self.good = 0;
            self.gap /= 2;
            if self.gap < MIN_GAP {
                self.gap = Duration::ZERO;
            }
        }
    }

    // Answers to checks that started before a pause began don't make it longer.

    fn slow_down(&mut self, w: &Waits, at: Instant) {
        self.good = 0;
        if self.paused(at) {
            return;
        }
        self.strikes += 1;
        self.gap = (self.gap * 2).max(w.gap).min(w.max_gap);
        self.pause(w, at, self.strikes);
    }

    fn no_answer(&mut self, w: &Waits, at: Instant) {
        self.good = 0;
        if self.paused(at) {
            return;
        }
        self.strikes += 1;
        if self.strikes >= FAILS_BEFORE_PAUSE {
            self.pause(w, at, self.strikes + 1 - FAILS_BEFORE_PAUSE);
        }
    }

    /// The `n`th pause in a row: twice as long as the one before, up to the longest.
    fn pause(&mut self, w: &Waits, at: Instant, n: u32) {
        let d = w.pause.saturating_mul(1 << (n - 1).min(16)).min(w.max_pause);
        self.pause_until = Some(at + d);
    }
}

/// A link waiting to be checked.
#[derive(Debug)]
struct Job {
    /// Its canonical form.
    key: String,
    url: String,
    /// Ask yt-dlp, even about a YouTube link.
    deep: bool,
    /// Tries so far that got no answer.
    tries: u32,
}

struct State {
    /// By canonical link. Loaded on first use, once the app has set its folders.
    cache: Option<HashMap<String, Checked>>,
    dirty: bool,
    saved_at: Instant,
    /// Ready for a worker, YouTube's quick checks first.
    queue: VecDeque<Job>,
    /// Links to ask about again later this session, and when.
    later: Vec<(Instant, Job)>,
    /// Keys queued, being checked, or waiting in `later`.
    pending: HashSet<String>,
    /// Keys that got no answer however often they were asked, and when they may be again.
    resting: HashMap<String, Instant>,
    throttle: Throttle,
    workers: usize,
    wake: Wake,
    tools: ToolPaths,
    net: NetworkOptions,
}

/// The link checks: a cache of results, and a few background workers filling it.
pub struct LinkCheck {
    state: Mutex<State>,
    /// Bumped whenever a result comes in, so pages know to read statuses again.
    version: AtomicU64,
    path: PathBuf,
    check: Box<CheckFn>,
    waits: Waits,
}

static SHARED: OnceLock<Arc<LinkCheck>> = OnceLock::new();

/// The app's link checks, kept in `linkcheck.json` in the data folder.
pub fn shared() -> &'static Arc<LinkCheck> {
    SHARED.get_or_init(|| Arc::new(LinkCheck::new(settings::data_dir().join("linkcheck.json"), Box::new(check_link))))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl LinkCheck {
    fn new(path: PathBuf, check: Box<CheckFn>) -> Self {
        Self::with_waits(path, check, WAITS)
    }

    fn with_waits(path: PathBuf, check: Box<CheckFn>, waits: Waits) -> Self {
        Self {
            state: Mutex::new(State {
                cache: None,
                dirty: false,
                saved_at: Instant::now(),
                queue: VecDeque::new(),
                later: Vec::new(),
                pending: HashSet::new(),
                resting: HashMap::new(),
                throttle: Throttle::default(),
                workers: 0,
                wake: no_wake(),
                tools: ToolPaths::default(),
                net: NetworkOptions::default(),
            }),
            version: AtomicU64::new(0),
            path,
            check,
            waits,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.cache.is_none() {
            s.cache = Some(load(&self.path));
        }
        s
    }

    fn cache(s: &mut State) -> &mut HashMap<String, Checked> {
        s.cache.get_or_insert_with(HashMap::new)
    }

    /// What to call when results come in (the UI's repaint, or Android's nudge).
    pub fn set_wake(&self, wake: Wake) {
        self.lock().wake = wake;
    }

    /// The tools and network settings checks run with.
    pub fn set_tools(&self, tools: ToolPaths, net: NetworkOptions) {
        let mut s = self.lock();
        s.tools = tools;
        s.net = net;
    }

    /// Changes whenever a result comes in.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    /// The last known status of a link, however old. Files are always `Unknown`.
    pub fn status(&self, url: &str) -> LinkStatus {
        let mut s = self.lock();
        Self::cache(&mut s).get(&platform::canonical(url)).map_or(LinkStatus::Unknown, |c| c.status)
    }

    /// [`Self::status`] of each track, under one lock.
    pub fn statuses<'a>(&self, tracks: impl IntoIterator<Item = &'a Track>) -> Vec<LinkStatus> {
        let mut s = self.lock();
        let cache = Self::cache(&mut s);
        tracks
            .into_iter()
            .map(|t| match &t.source {
                Source::Link(u) => cache.get(&platform::canonical(u)).map_or(LinkStatus::Unknown, |c| c.status),
                Source::Local(_) => LinkStatus::Unknown,
            })
            .collect()
    }

    /// Notes a status found some other way (a playlist listing, a replacement just found).
    pub fn record(&self, url: &str, status: LinkStatus) {
        self.record_all([(url, status)]);
    }

    /// Notes what playing a link, or trying to, showed ([`PlayWatch`] does this). Unlike
    /// [`Self::record`], a quick check saying the link is there doesn't undo it.
    pub fn record_played(&self, url: &str, status: LinkStatus) {
        self.record_with([(url, status)], true);
    }

    /// [`Self::record`] for many links, saved once.
    pub fn record_all<'a>(&self, results: impl IntoIterator<Item = (&'a str, LinkStatus)>) {
        self.record_with(results, false);
    }

    fn record_with<'a>(&self, results: impl IntoIterator<Item = (&'a str, LinkStatus)>, deep: bool) {
        let mut s = self.lock();
        let mut changed = false;
        for (url, status) in results {
            if status == LinkStatus::Unknown {
                continue;
            }
            let key = platform::canonical(url);
            changed |= put(Self::cache(&mut s), key, url, status, deep, now());
        }
        if changed {
            s.dirty = true;
            self.save(&mut s);
            let wake = s.wake.clone();
            drop(s);
            self.version.fetch_add(1, Ordering::Relaxed);
            wake();
        }
    }

    /// Statuses a playlist listing gave its entries.
    pub fn record_listing(&self, items: &[PlaylistItem]) {
        self.record_all(items.iter().filter_map(|i| Some((i.url.as_str(), i.availability?))));
    }

    /// Checks, in the background, the links among `tracks` with no fresh result. YouTube's
    /// go first: they're quick. Links that got no answer are asked again later by
    /// themselves; once they've had all their tries they rest for a while.
    pub fn request<'a>(self: &Arc<Self>, tracks: impl IntoIterator<Item = &'a Track>) {
        self.request_at(tracks, now());
    }

    fn request_at<'a>(self: &Arc<Self>, tracks: impl IntoIterator<Item = &'a Track>, now: i64) {
        let mut s = self.lock();
        let clock = Instant::now();
        let mut fresh: Vec<Job> = Vec::new();
        for t in tracks {
            let Source::Link(url) = &t.source else { continue };
            let key = platform::canonical(url);
            let entry = Self::cache(&mut s).get(&key).copied();
            let known = entry.is_some_and(|c| now - c.at < max_age(url, c.deep));
            if known || s.pending.contains(&key) || s.resting.get(&key).is_some_and(|until| clock < *until) {
                continue;
            }
            s.resting.remove(&key);
            s.pending.insert(key.clone());
            fresh.push(Job { key, url: url.clone(), deep: entry.is_some_and(|c| c.deep), tries: 0 });
        }
        fresh.sort_by_key(|j| j.deep || Platform::of(&j.url) != Platform::YouTube);
        s.queue.extend(fresh);
        while s.workers < WORKERS && s.workers < s.queue.len() {
            s.workers += 1;
            let me = self.clone();
            let spawned = std::thread::Builder::new().name("redline-linkcheck".into()).spawn(move || me.work());
            if spawned.is_err() {
                s.workers -= 1;
                break;
            }
        }
    }

    fn work(&self) {
        loop {
            let (job, tools, net) = {
                let mut s = self.lock();
                let clock = Instant::now();
                // Links due to be asked again go back in line.
                let mut i = 0;
                while i < s.later.len() {
                    if s.later[i].0 <= clock {
                        let (_, job) = s.later.swap_remove(i);
                        s.queue.push_back(job);
                    } else {
                        i += 1;
                    }
                }
                let ready = s.throttle.ready_at().filter(|r| *r > clock);
                let wait_until = if s.queue.is_empty() {
                    let next = s.later.iter().map(|(at, _)| *at).min();
                    // One worker is enough to wait for the links asked again later.
                    if next.is_none() || s.workers > 1 {
                        s.workers -= 1;
                        if s.workers == 0 {
                            self.save(&mut s);
                        }
                        return;
                    }
                    next.max(ready)
                } else {
                    ready
                };
                if let Some(until) = wait_until {
                    drop(s);
                    std::thread::sleep(until.saturating_duration_since(clock).min(TICK));
                    continue;
                }
                s.throttle.started(clock);
                let Some(job) = s.queue.pop_front() else { continue };
                (job, s.tools.clone(), s.net.clone())
            };
            let answer = (self.check)(&tools, &net, &job.url, job.deep);
            let wake = {
                let mut s = self.lock();
                self.take(&mut s, job, answer);
                s.wake.clone()
            };
            self.version.fetch_add(1, Ordering::Relaxed);
            wake();
        }
    }

    /// Files one check's answer: the status in the cache, or the link in line again for later.
    fn take(&self, s: &mut State, job: Job, answer: Answer) {
        let w = &self.waits;
        let clock = Instant::now();
        match answer {
            Answer::Status(status) if status != LinkStatus::Unknown => {
                s.throttle.went_well();
                s.pending.remove(&job.key);
                if put(Self::cache(s), job.key, &job.url, status, job.deep, now()) {
                    s.dirty = true;
                }
                if clock.duration_since(s.saved_at) >= SAVE_EVERY {
                    self.save(s);
                }
            }
            Answer::SlowDown => {
                s.throttle.slow_down(w, clock);
                // Not the link's fault, so not one of its tries: its turn comes after the pause.
                s.later.push((clock, job));
            }
            Answer::Status(_) | Answer::NoAnswer => {
                s.throttle.no_answer(w, clock);
                let tries = job.tries + 1;
                if tries >= MAX_TRIES {
                    s.pending.remove(&job.key);
                    s.resting.insert(job.key, clock + w.rest);
                } else {
                    let wait = w.retry.saturating_mul(1 << (tries - 1));
                    s.later.push((clock + wait, Job { tries, ..job }));
                }
            }
        }
    }

    /// Whether checks are still running, or waiting to ask again.
    pub fn busy(&self) -> bool {
        self.lock().workers > 0
    }

    fn save(&self, s: &mut State) {
        s.saved_at = Instant::now();
        if !std::mem::take(&mut s.dirty) {
            return;
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Some(cache) = &s.cache
            && let Ok(json) = serde_json::to_string(cache)
        {
            let _ = std::fs::write(&self.path, json);
        }
    }

    /// `tracks` with the dead ones taken out, and where `start` lands among the rest (the
    /// next live track when `start` itself is dead). Everything dead leaves the list as it
    /// is, so playing it still says why it can't.
    pub fn drop_dead(&self, tracks: Vec<Track>, start: usize) -> (Vec<Track>, usize) {
        let dead: Vec<bool> = self.statuses(&tracks).into_iter().map(LinkStatus::is_dead).collect();
        drop_dead(tracks, start, &dead)
    }

    /// After the current track failed to play for good: moves `now` on to the next track
    /// that isn't known to be dead, as Next would (wrapping round with repeat all). Returns
    /// false, staying put, when there's none.
    pub fn skip_failed(&self, now: &mut NowPlaying) -> bool {
        let Some(start) = now.current_index() else { return false };
        let dead: Vec<bool> = self.statuses(&now.tracks).into_iter().map(LinkStatus::is_dead).collect();
        for _ in 1..now.tracks.len() {
            if now.next().is_none() {
                break;
            }
            match now.current_index() {
                Some(i) if i == start => break,
                Some(i) if !dead[i] => return true,
                _ => {}
            }
        }
        now.jump(start);
        false
    }
}

// ---- Links that fail to play -------------------------------------------------------------

/// Lasting failures the player skips past before anything plays. More in a row than that and
/// it's likely something else (yt-dlp, the network, YouTube turning everyone away): it stops
/// there, and stops marking links, until something plays.
pub const MAX_SKIPS: u32 = 8;

/// A link the player couldn't play.
#[derive(Clone, Debug, PartialEq)]
pub struct FailedPlay {
    /// The queue's track (or a bare one for the link, if the queue had moved on).
    pub track: Track,
    /// The playlist the queue was started from.
    pub playlist: Option<PlaylistId>,
    /// The player's error.
    pub error: String,
    /// What the error says about the link; recorded in the link checks unless `Unknown`.
    pub status: LinkStatus,
    /// Move on to the next track: the link won't play however often it's asked for.
    pub skip: bool,
}

/// Watches the player for links that fail to play, and records what each error says about
/// its link, so the row shows it straight away: the current track's, once per load, and the
/// next one's when the player's look-up ahead of time fails. Both apps' frame loops call it.
#[derive(Debug, Default)]
pub struct PlayWatch {
    /// The load whose failure was handed out last.
    load: u64,
    /// The look-ahead failure last seen.
    next: Option<(String, String)>,
    /// Lasting failures since something last played.
    run: u32,
}

impl PlayWatch {
    /// Call whenever the player's state may have changed. Returns the current link's failure,
    /// once.
    pub fn check(&mut self, lc: &LinkCheck, ps: &PlayerState, now: &NowPlaying) -> Option<FailedPlay> {
        if ps.status == Status::Playing {
            self.run = 0;
        }
        if ps.next_failed != self.next {
            self.next = ps.next_failed.clone();
            if let Some((url, error)) = &self.next
                && self.run < MAX_SKIPS
            {
                lc.record_played(url, status_for_ytdlp_error(error));
            }
        }
        let (Status::Error(error), Some(Source::Link(url))) = (&ps.status, &ps.source) else { return None };
        if ps.load_id == self.load {
            return None;
        }
        self.load = ps.load_id;
        let status = status_for_ytdlp_error(error);
        let lasting = status != LinkStatus::Unknown;
        if lasting {
            self.run += 1;
        }
        let trusted = self.run <= MAX_SKIPS;
        if trusted {
            lc.record_played(url, status);
        }
        let track = now
            .current()
            .filter(|t| matches!(&t.source, Source::Link(u) if u == url))
            .cloned()
            .unwrap_or_else(|| Track::new(Source::Link(url.clone()), url.clone()));
        Some(FailedPlay { track, playlist: now.playlist, error: error.clone(), status, skip: lasting && trusted })
    }
}

fn load(path: &Path) -> HashMap<String, Checked> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// [`LinkCheck::drop_dead`] with the dead tracks marked in `dead`.
pub fn drop_dead(tracks: Vec<Track>, start: usize, dead: &[bool]) -> (Vec<Track>, usize) {
    let is_dead = |i: usize| dead.get(i).copied().unwrap_or(false);
    if (0..tracks.len()).all(is_dead) || !(0..tracks.len()).any(is_dead) {
        return (tracks, start);
    }
    let before = (0..start.min(tracks.len())).filter(|i| !is_dead(*i)).count();
    let kept: Vec<Track> = tracks.into_iter().enumerate().filter(|(i, _)| !is_dead(*i)).map(|(_, t)| t).collect();
    let start = before.min(kept.len() - 1);
    (kept, start)
}

// ---- Replacements ------------------------------------------------------------------------

/// Another upload a dead track could point at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidate {
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
}

impl From<PlaylistItem> for Candidate {
    fn from(i: PlaylistItem) -> Self {
        Self { url: i.url, title: i.title, uploader: i.uploader, duration: i.duration, thumbnail: i.thumbnail }
    }
}

/// A candidate and how well it matches.
#[derive(Clone, Debug, PartialEq)]
pub struct Ranked {
    pub candidate: Candidate,
    /// 0 to 1: how alike the titles (with artists) are.
    pub title_score: f64,
    /// Candidate's length minus the track's, in seconds, when both are known.
    pub delta_secs: Option<f64>,
    /// The ranking, 0 to 1.
    pub score: f64,
}

impl Ranked {
    /// Safe to swap in without asking: a similar title and the length within 3 seconds.
    pub fn is_clear_match(&self) -> bool {
        self.title_score >= 0.6 && self.delta_secs.is_some_and(|d| d.abs() <= 3.0)
    }
}

/// How many results a search asks for.
pub const SEARCH_RESULTS: usize = 5;

/// An artist name without YouTube's auto-channel suffix.
fn artist_name(a: &str) -> &str {
    a.trim().trim_end_matches(" - Topic").trim_end_matches("VEVO").trim()
}

/// The yt-dlp search for a track: `ytsearch5:artist title`.
pub fn search_query(track: &Track) -> String {
    let artist = track.artist.as_deref().map(artist_name).unwrap_or("");
    let terms = if artist.is_empty() || track.title.to_lowercase().contains(&artist.to_lowercase()) {
        track.title.clone()
    } else {
        format!("{artist} {}", track.title)
    };
    format!("ytsearch{SEARCH_RESULTS}:{terms}")
}

/// Uploads of the same song on YouTube, best first.
pub fn search(tools: &ToolPaths, net: &NetworkOptions, track: &Track) -> Result<Vec<Ranked>> {
    let info = ytdlp::fetch_info(tools, net, &search_query(track))?;
    Ok(rank(track, info.items.into_iter().map(Candidate::from).collect()))
}

/// Words that say nothing about which song it is.
const NOISE: &[&str] = &[
    "official", "video", "music", "audio", "lyrics", "lyric", "hd", "hq", "4k", "remastered", "remaster", "mv", "visualizer",
    "the", "a", "ft", "feat", "topic", "vevo",
];

fn words(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !NOISE.contains(w))
        .map(str::to_owned)
        .collect()
}

/// Dice similarity of the two word sets.
fn similarity(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    2.0 * a.intersection(b).count() as f64 / (a.len() + b.len()) as f64
}

/// Orders candidates by how much they look like `track`: mostly the title (with the artist),
/// then the length. The track's own link and the one it replaced are left out.
pub fn rank(track: &Track, candidates: Vec<Candidate>) -> Vec<Ranked> {
    let artist = |a: Option<&str>| a.map(artist_name).unwrap_or("").to_owned();
    let want = words(&format!("{} {}", track.title, artist(track.artist.as_deref())));
    let own: HashSet<String> = match &track.source {
        Source::Link(u) => [Some(u.as_str()), track.replaced_from.as_deref()]
            .into_iter()
            .flatten()
            .map(platform::canonical)
            .collect(),
        Source::Local(_) => HashSet::new(),
    };
    let mut seen = HashSet::new();
    let mut ranked: Vec<Ranked> = candidates
        .into_iter()
        .filter(|c| {
            let key = platform::canonical(&c.url);
            !own.contains(&key) && seen.insert(key)
        })
        .map(|c| {
            let got = words(&format!("{} {}", c.title, artist(c.uploader.as_deref())));
            let title_score = similarity(&want, &got);
            let delta_secs = track.duration.zip(c.duration).map(|(a, b)| b - a);
            let length_score = delta_secs.map_or(0.5, |d| 1.0 - (d.abs() / 30.0).min(1.0));
            Ranked { candidate: c, title_score, delta_secs, score: 0.7 * title_score + 0.3 * length_score }
        })
        .collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
    ranked
}

/// The replacements found for one track of a list.
#[derive(Clone, Debug)]
pub struct FixItem {
    /// Where the track is in its list.
    pub index: usize,
    pub track: Track,
    pub ranked: Vec<Ranked>,
}

impl FixItem {
    /// The pick to apply without asking, if there's a clear one.
    pub fn clear_match(&self) -> Option<&Candidate> {
        self.ranked.first().filter(|r| r.is_clear_match()).map(|r| &r.candidate)
    }
}

/// Searches for replacements of each track, one after another (slow: run it off the UI
/// thread). A failed search just leaves that track with nothing to pick.
pub fn fix_unavailable(tools: &ToolPaths, net: &NetworkOptions, tracks: Vec<(usize, Track)>) -> Vec<FixItem> {
    tracks
        .into_iter()
        .map(|(index, track)| {
            let ranked = search(tools, net, &track).unwrap_or_default();
            FixItem { index, track, ranked }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(url: &str, title: &str, artist: Option<&str>, duration: Option<f64>) -> Track {
        let mut t = Track::new(Source::Link(url.into()), title.into());
        t.artist = artist.map(Into::into);
        t.duration = duration;
        t
    }

    fn cand(url: &str, title: &str, uploader: &str, duration: f64) -> Candidate {
        Candidate { url: url.into(), title: title.into(), uploader: Some(uploader.into()), duration: Some(duration), thumbnail: None }
    }

    #[test]
    fn oembed_codes() {
        assert_eq!(status_for_oembed(Some(200)), LinkStatus::Alive);
        assert_eq!(status_for_oembed(Some(404)), LinkStatus::Dead);
        assert_eq!(status_for_oembed(Some(400)), LinkStatus::Dead);
        assert_eq!(status_for_oembed(Some(401)), LinkStatus::Restricted);
        assert_eq!(status_for_oembed(Some(403)), LinkStatus::Restricted);
        assert_eq!(status_for_oembed(Some(500)), LinkStatus::Unknown);
        assert_eq!(status_for_oembed(Some(429)), LinkStatus::Unknown);
        assert_eq!(status_for_oembed(None), LinkStatus::Unknown, "a network error tells nothing");
        assert_eq!(answer_for_oembed(Some(200)), Answer::Status(LinkStatus::Alive));
        assert_eq!(answer_for_oembed(Some(404)), Answer::Status(LinkStatus::Dead));
        assert_eq!(answer_for_oembed(Some(429)), Answer::SlowDown, "rate limited");
        assert_eq!(answer_for_oembed(Some(500)), Answer::NoAnswer);
        assert_eq!(answer_for_oembed(None), Answer::NoAnswer);
    }

    #[test]
    fn ytdlp_errors() {
        use LinkStatus::*;
        let s = status_for_ytdlp_error;
        assert_eq!(s("ERROR: [youtube] abc: Video unavailable"), Dead);
        assert_eq!(s("ERROR: [soundcloud] x: Unable to download JSON metadata: HTTP Error 404: Not Found"), Unknown);
        assert_eq!(s("ERROR: [soundcloud] x: HTTP Error 404: Not Found"), Dead);
        assert_eq!(s("ERROR: [youtube] abc: Private video. Sign in if you've been granted access"), Restricted);
        assert_eq!(s("ERROR: [youtube] abc: Video unavailable. This video is private"), Restricted);
        assert_eq!(s("ERROR: [youtube] abc: Sign in to confirm you're not a bot"), Unknown);
        assert_eq!(s("ERROR: something odd"), Unknown);

        // Gone, or never going to play here. The first is what the links oEmbed calls fine
        // (blocked here, or for licensing) say when they're played.
        for gone in [
            "ERROR: [youtube] CUGC3ya2rfA: Video unavailable. This video is not available",
            "This video is not available",
            "ERROR: [youtube] abc: The uploader has not made this video available in your country",
            "ERROR: [youtube] abc: Video unavailable. This video contains content from SME, who has blocked it in your country on copyright grounds",
            "ERROR: [youtube] abc: This video is no longer available due to a copyright claim by Someone",
            "ERROR: [youtube] abc: This video has been removed by the uploader",
            "ERROR: [youtube] abc: Video unavailable. This video is no longer available because the YouTube account associated with this video has been terminated.",
        ] {
            assert_eq!(s(gone), Dead, "{gone}");
        }
        for restricted in [
            "ERROR: [youtube] abc: Private video. Sign in if you've been granted access to this video",
            "ERROR: [youtube] abc: Join this channel to get access to members-only content like this video",
            "ERROR: [youtube] abc: Sign in to confirm your age. This video may be inappropriate for some users.",
        ] {
            assert_eq!(s(restricted), Restricted, "{restricted}");
        }
        // Nothing about the link, though some say "unavailable".
        let rate_limited = "ERROR: [youtube] abc: Video unavailable. This content isn't available, try again later. \
                            The current session has been rate-limited by YouTube for up to an hour.";
        for unknown in [
            rate_limited,
            "ERROR: [youtube] abc: This video is unavailable. Error code: 152 - 18 Watch video on YouTube",
            "ERROR: [youtube] abc: Requested format is not available. Use --list-formats for a list of available formats",
            "ERROR: [soundcloud] x: HTTP Error 503: Service Unavailable",
            "ERROR: [youtube] abc: HTTP Error 429: Too Many Requests",
            "ERROR: [youtube] abc: This live event will begin in a few moments.",
        ] {
            assert_eq!(s(unknown), Unknown, "{unknown}");
        }
        // As the player shows it, with the hint added.
        let bot = crate::tools::ytdlp::friendly_error("Sign in to confirm you’re not a bot. Use --cookies-from-browser");
        assert_eq!(s(&bot), Unknown);

        let a = answer_for_ytdlp_error;
        assert_eq!(a("ERROR: [youtube] abc: HTTP Error 429: Too Many Requests"), Answer::SlowDown);
        assert_eq!(a(&bot), Answer::SlowDown);
        assert_eq!(a(rate_limited), Answer::SlowDown);
        assert_eq!(a("ERROR: [youtube] abc: This video is unavailable. Error code: 152 - 18"), Answer::NoAnswer);
        assert_eq!(a("ERROR: [youtube] abc: Video unavailable. This video is not available"), Answer::Status(Dead));
    }

    #[test]
    fn flat_entries_say_whats_gone() {
        assert_eq!(status_from_flat(Some("[Deleted video]"), None), Some(LinkStatus::Dead));
        assert_eq!(status_from_flat(Some("[Private video]"), Some("private")), Some(LinkStatus::Restricted));
        assert_eq!(status_from_flat(Some("Song"), Some("needs_auth")), Some(LinkStatus::Restricted));
        assert_eq!(status_from_flat(Some("Song"), Some("public")), Some(LinkStatus::Alive));
        assert_eq!(status_from_flat(Some("Song"), None), None);
    }

    #[test]
    fn ranking_picks_the_matching_title_and_length() {
        let t = track("https://www.youtube.com/watch?v=dead", "Song Name", Some("Band - Topic"), Some(200.0));
        let ranked = rank(
            &t,
            vec![
                cand("https://www.youtube.com/watch?v=live", "Band - Song Name (Live at Wembley)", "Band", 262.0),
                cand("https://www.youtube.com/watch?v=other", "Different Tune", "Someone", 200.0),
                cand("https://www.youtube.com/watch?v=good", "Band - Song Name (Official Video)", "BandVEVO", 201.5),
                cand("https://youtu.be/dead", "Song Name", "Band", 200.0),
            ],
        );
        assert_eq!(ranked.len(), 3, "the dead link itself isn't offered");
        assert_eq!(ranked[0].candidate.url, "https://www.youtube.com/watch?v=good");
        assert!(ranked[0].is_clear_match());
        assert_eq!(ranked[0].delta_secs, Some(1.5));
        assert!(!ranked[1].is_clear_match());
        assert_eq!(ranked[2].candidate.url, "https://www.youtube.com/watch?v=other");

        // A good title but ten seconds off is for the user to decide.
        let off = rank(&t, vec![cand("https://www.youtube.com/watch?v=x", "Song Name", "Band", 210.0)]);
        assert!(!off[0].is_clear_match());
        let no_len = rank(&track("https://y/1", "Song Name", None, None), vec![cand("https://y/2", "Song Name", "B", 1.0)]);
        assert!(!no_len[0].is_clear_match(), "no length to compare");
    }

    #[test]
    fn search_query_names_artist_once() {
        let t = track("https://y", "Song", Some("Band - Topic"), None);
        assert_eq!(search_query(&t), "ytsearch5:Band Song");
        let t = track("https://y", "Band - Song", Some("Band"), None);
        assert_eq!(search_query(&t), "ytsearch5:Band - Song");
        assert_eq!(search_query(&track("https://y", "Song", None, None)), "ytsearch5:Song");
    }

    #[test]
    fn drops_dead_tracks_and_moves_the_start() {
        let ts: Vec<Track> = (0..4).map(|i| track(&format!("https://y/{i}"), &format!("T{i}"), None, None)).collect();
        let titles = |v: &[Track]| v.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        let (kept, start) = drop_dead(ts.clone(), 1, &[false, true, false, false]);
        assert_eq!(titles(&kept), ["T0", "T2", "T3"]);
        assert_eq!(kept[start].title, "T2", "a dead start moves on to the next live one");
        let (kept, start) = drop_dead(ts.clone(), 3, &[false, true, false, false]);
        assert_eq!(kept[start].title, "T3");
        let (kept, start) = drop_dead(ts.clone(), 3, &[false, false, false, true]);
        assert_eq!(kept[start].title, "T2", "nothing live after it: the last live one");
        let (kept, _) = drop_dead(ts.clone(), 0, &[true; 4]);
        assert_eq!(kept.len(), 4, "all dead: left alone");
    }

    /// Waits in milliseconds, so the tests don't take minutes.
    fn waits(retry: u64, pause: u64, gap: u64, rest: u64) -> Waits {
        let ms = Duration::from_millis;
        Waits { retry: ms(retry), pause: ms(pause), max_pause: ms(pause * 4), gap: ms(gap), max_gap: ms(gap * 4), rest: ms(rest) }
    }

    fn temp_check_with(name: &str, waits: Waits, check: Box<CheckFn>) -> (Arc<LinkCheck>, PathBuf) {
        let path = std::env::temp_dir().join(format!("redline-linkcheck-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        (Arc::new(LinkCheck::with_waits(path.clone(), check, waits)), path)
    }

    fn temp_check(name: &str, check: Box<CheckFn>) -> (Arc<LinkCheck>, PathBuf) {
        temp_check_with(name, WAITS, check)
    }

    fn wait(lc: &LinkCheck) {
        let until = Instant::now() + Duration::from_secs(10);
        while lc.busy() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!lc.busy(), "the checks didn't finish");
    }

    /// Each call's link and when it started.
    type Calls = Arc<Mutex<Vec<(String, Instant)>>>;

    fn calls_to(calls: &Calls, url: &str) -> Vec<Instant> {
        calls.lock().unwrap().iter().filter(|(u, _)| u == url).map(|(_, at)| *at).collect()
    }

    #[test]
    fn checks_in_the_background_and_caches() {
        let calls = Arc::new(AtomicU64::new(0));
        let counter = calls.clone();
        let (lc, path) = temp_check_with(
            "bg",
            // Resting a whole minute: the offline link isn't asked about again in this test.
            waits(5, 5, 5, 60_000),
            Box::new(move |_, _, url, _| {
                counter.fetch_add(1, Ordering::Relaxed);
                match url {
                    u if u.ends_with("dead") => Answer::Status(LinkStatus::Dead),
                    u if u.ends_with("offline") => Answer::NoAnswer,
                    _ => Answer::Status(LinkStatus::Alive),
                }
            }),
        );
        let ts = [
            track("https://www.youtube.com/watch?v=dead", "a", None, None),
            track("https://youtu.be/live", "b", None, None),
            track("https://soundcloud.com/x/offline", "c", None, None),
            Track::new(Source::Local("C:/a.mp3".into()), "d".into()),
        ];
        lc.request(&ts);
        wait(&lc);
        let tries = MAX_TRIES as u64;
        assert_eq!(calls.load(Ordering::Relaxed), 2 + tries, "files aren't checked; no answer is asked again");
        assert_eq!(lc.statuses(&ts), [LinkStatus::Dead, LinkStatus::Alive, LinkStatus::Unknown, LinkStatus::Unknown]);
        assert_eq!(lc.status("https://www.youtube.com/watch?v=dead&t=4"), LinkStatus::Dead, "by canonical link");
        assert!(lc.version() >= 3);

        lc.request(&ts);
        wait(&lc);
        assert_eq!(calls.load(Ordering::Relaxed), 2 + tries, "fresh results and resting links aren't asked again");

        // A week later the YouTube results are stale; SoundCloud's would last two.
        lc.request_at(&ts, now() + 8 * 24 * 3600);
        wait(&lc);
        assert_eq!(calls.load(Ordering::Relaxed), 4 + tries);

        // Saved, and loaded back by a new checker.
        let back = LinkCheck::new(path.clone(), Box::new(|_, _, _, _| Answer::NoAnswer));
        assert_eq!(back.status("https://youtu.be/dead"), LinkStatus::Dead);
        assert_eq!(back.status("https://youtu.be/live"), LinkStatus::Alive);
        let (kept, start) = back.drop_dead(ts.to_vec(), 0);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[start].title, "b");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn no_answer_is_asked_again_later_then_rests() {
        let calls: Calls = Arc::default();
        let log = calls.clone();
        let w = waits(40, 30, 5, 300);
        let (lc, path) = temp_check_with(
            "retry",
            w,
            Box::new(move |_, _, url, _| {
                let mut log = log.lock().unwrap();
                log.push((url.to_owned(), Instant::now()));
                let n = log.iter().filter(|(u, _)| u == url).count();
                match url {
                    // A timeout, then a server error, then an answer.
                    u if u.ends_with("flaky") && n < 3 => Answer::NoAnswer,
                    u if u.ends_with("never") => Answer::NoAnswer,
                    _ => Answer::Status(LinkStatus::Alive),
                }
            }),
        );
        let flaky = "https://www.youtube.com/watch?v=flaky";
        lc.request(&[track(flaky, "f", None, None)]);
        wait(&lc);
        assert_eq!(lc.status(flaky), LinkStatus::Alive, "got through in the end");
        let at = calls_to(&calls, flaky);
        assert_eq!(at.len(), 3);
        assert!(at[1] - at[0] >= w.retry, "asked again after a wait: {:?}", at[1] - at[0]);
        assert!(at[2] - at[1] >= w.retry * 2, "and twice as long the next time: {:?}", at[2] - at[1]);

        // One that never answers gets its tries, then rests even when asked about.
        let never = "https://www.youtube.com/watch?v=never";
        let t = [track(never, "n", None, None)];
        lc.request(&t);
        wait(&lc);
        assert_eq!(calls_to(&calls, never).len(), MAX_TRIES as usize);
        assert_eq!(lc.status(never), LinkStatus::Unknown);
        lc.request(&t);
        wait(&lc);
        assert_eq!(calls_to(&calls, never).len(), MAX_TRIES as usize, "resting");
        std::thread::sleep(w.rest);
        lc.request(&t);
        wait(&lc);
        assert_eq!(calls_to(&calls, never).len(), 2 * MAX_TRIES as usize, "asked again after its rest");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rate_limit_pauses_and_spaces_out_the_checks() {
        let calls: Calls = Arc::default();
        let limited: Arc<Mutex<Option<Instant>>> = Arc::default();
        let (log, limit) = (calls.clone(), limited.clone());
        let w = waits(10, 200, 60, 60_000);
        let (lc, path) = temp_check_with(
            "429",
            w,
            Box::new(move |_, _, url, _| {
                log.lock().unwrap().push((url.to_owned(), Instant::now()));
                let first_a = url.ends_with("=a") && limit.lock().unwrap().is_none();
                if first_a {
                    // Answer once all three workers have a check out, so the next ones all
                    // start after the 429 has been taken in.
                    let until = Instant::now() + Duration::from_secs(5);
                    while log.lock().unwrap().len() < WORKERS && Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    *limit.lock().unwrap() = Some(Instant::now());
                    return Answer::SlowDown;
                }
                std::thread::sleep(Duration::from_millis(20));
                Answer::Status(LinkStatus::Alive)
            }),
        );
        let urls: Vec<String> = ["a", "b", "c", "d", "e", "f"].iter().map(|v| format!("https://www.youtube.com/watch?v={v}")).collect();
        let ts: Vec<Track> = urls.iter().map(|u| track(u, "t", None, None)).collect();
        lc.request(&ts);
        wait(&lc);
        assert!(lc.statuses(&ts).iter().all(|s| *s == LinkStatus::Alive), "all got through");
        assert_eq!(calls_to(&calls, &urls[0]).len(), 2, "the turned-away link was asked again");

        let limited = limited.lock().unwrap().expect("a 429");
        let mut after: Vec<Instant> = calls.lock().unwrap().iter().map(|(_, at)| *at).filter(|at| *at > limited).collect();
        after.sort();
        assert_eq!(after.len(), 4, "a again, and d to f");
        // Nothing until the pause is over, then spaced out (less a little for scheduling).
        assert!(after[0] - limited >= w.pause, "paused only {:?}", after[0] - limited);
        let slack = Duration::from_millis(5);
        for pair in after.windows(2) {
            assert!(pair[1] - pair[0] + slack >= w.gap, "only {:?} apart", pair[1] - pair[0]);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn throttle_backs_off_and_recovers() {
        let w = waits(10, 100, 150, 1000);
        let mut t = Throttle::default();
        let t0 = Instant::now();
        assert_eq!(t.ready_at(), None);

        // A rate limit pauses everything and spaces the checks out.
        t.slow_down(&w, t0);
        assert_eq!(t.ready_at(), Some(t0 + w.pause));
        assert_eq!(t.gap, w.gap);
        // More from checks already out during the pause don't make it longer.
        t.slow_down(&w, t0 + Duration::from_millis(1));
        assert_eq!((t.ready_at(), t.strikes), (Some(t0 + w.pause), 1));
        // Another one after it: twice as long, twice as spaced out.
        let t1 = t0 + w.pause;
        t.slow_down(&w, t1);
        assert_eq!(t.pause_until, Some(t1 + w.pause * 2));
        assert_eq!(t.gap, w.gap * 2);
        t.started(t1 + w.pause * 2);
        assert_eq!(t.ready_at(), Some(t1 + w.pause * 2 + w.gap * 2), "the gap after a start");

        // Answers bring the strikes back down, and enough of them shrink the gap again.
        for _ in 0..SPEED_UP_AFTER {
            t.went_well();
        }
        assert_eq!((t.strikes, t.gap), (0, w.gap));
        for _ in 0..SPEED_UP_AFTER {
            t.went_well();
        }
        assert_eq!(t.gap, Duration::ZERO, "under the least gap is none");

        // Plain failures pause only after a few in a row.
        let mut t = Throttle::default();
        for _ in 1..FAILS_BEFORE_PAUSE {
            t.no_answer(&w, t0);
        }
        assert_eq!(t.pause_until, None);
        t.no_answer(&w, t0);
        assert_eq!(t.pause_until, Some(t0 + w.pause));
    }

    #[test]
    fn what_playing_found_sticks() {
        let deep_asks: Calls = Arc::default();
        let log = deep_asks.clone();
        let (lc, path) = temp_check(
            "played",
            Box::new(move |_, _, url, deep| {
                if deep {
                    log.lock().unwrap().push((url.to_owned(), Instant::now()));
                }
                // yt-dlp's answer: it plays again.
                Answer::Status(LinkStatus::Alive)
            }),
        );
        let url = "https://music.youtube.com/watch?v=blocked&si=x";
        lc.record_played(url, LinkStatus::Dead);
        assert_eq!(lc.status(url), LinkStatus::Dead);
        // A listing (or oEmbed) saying it's there doesn't undo what playing it showed.
        lc.record(url, LinkStatus::Alive);
        assert_eq!(lc.status("https://youtu.be/blocked"), LinkStatus::Dead);
        let back = LinkCheck::new(path.clone(), Box::new(|_, _, _, _| Answer::NoAnswer));
        assert_eq!(back.status(url), LinkStatus::Dead, "saved with it");
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"deep\":true"));

        // A week on it's still fresh; after two it's asked about again, with yt-dlp.
        let t = [track(url, "b", None, None)];
        lc.request_at(&t, now() + 8 * 24 * 3600);
        wait(&lc);
        assert!(deep_asks.lock().unwrap().is_empty());
        lc.request_at(&t, now() + 15 * 24 * 3600);
        wait(&lc);
        assert_eq!(deep_asks.lock().unwrap().len(), 1);
        assert_eq!(lc.status(url), LinkStatus::Alive, "yt-dlp's answer counts");
        let _ = std::fs::remove_file(&path);
    }

    fn failed(url: &str, error: &str, load_id: u64) -> PlayerState {
        PlayerState {
            status: Status::Error(error.into()),
            source: Some(Source::Link(url.into())),
            load_id,
            ..Default::default()
        }
    }

    #[test]
    fn failed_plays_are_recorded_and_skipped() {
        let (lc, path) = temp_check("plays", Box::new(|_, _, _, _| panic!("no checks")));
        let urls: Vec<String> = (0..5).map(|i| format!("https://music.youtube.com/watch?v=p{i}")).collect();
        let mut now = NowPlaying::default();
        now.start(urls.iter().map(|u| track(u, "t", None, None)).collect(), 1, Some(7));
        lc.record(&urls[2], LinkStatus::Dead);
        let gone = "ERROR: [youtube] p1: Video unavailable. This video is not available";

        let mut watch = PlayWatch::default();
        let f = watch.check(&lc, &failed(&urls[1], gone, 2), &now).expect("a failure");
        assert_eq!((f.status, f.skip, f.playlist), (LinkStatus::Dead, true, Some(7)));
        assert_eq!(f.track.source, Source::Link(urls[1].clone()));
        assert_eq!(lc.status(&urls[1]), LinkStatus::Dead, "the row shows it straight away");
        assert_eq!(watch.check(&lc, &failed(&urls[1], gone, 2), &now), None, "once per load");
        // On to the next one that isn't known to be dead.
        assert!(lc.skip_failed(&mut now));
        assert_eq!(now.current_index(), Some(3));

        // Network trouble says nothing about the link: shown, but not skipped or recorded.
        let bot = "Sign in to confirm you're not a bot";
        let f = watch.check(&lc, &failed(&urls[3], bot, 3), &now).unwrap();
        assert_eq!((f.status, f.skip), (LinkStatus::Unknown, false));
        assert_eq!(lc.status(&urls[3]), LinkStatus::Unknown);

        // The next track failing to look up ahead of time marks it before its turn.
        let ahead = PlayerState { next_failed: Some((urls[4].clone(), gone.into())), ..Default::default() };
        assert_eq!(watch.check(&lc, &ahead, &now), None);
        assert_eq!(lc.status(&urls[4]), LinkStatus::Dead);

        // Nothing left to skip to: stays put. (p0 is behind, and repeat is off.)
        now.jump(4);
        assert!(!lc.skip_failed(&mut now));
        assert_eq!(now.current_index(), Some(4));
        now.repeat = crate::library::Repeat::All;
        assert!(lc.skip_failed(&mut now), "round to the start with repeat all");
        assert_eq!(now.current_index(), Some(0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_run_of_failures_stops_skipping() {
        let (lc, path) = temp_check("run", Box::new(|_, _, _, _| panic!("no checks")));
        let now = NowPlaying::default();
        let mut watch = PlayWatch::default();
        let gone = "Video unavailable. This video is not available";
        let url = |i: u64| format!("https://www.youtube.com/watch?v=r{i}");
        for i in 1..=u64::from(MAX_SKIPS) {
            let f = watch.check(&lc, &failed(&url(i), gone, i), &now).unwrap();
            assert!(f.skip, "{i}");
        }
        // One too many: probably not the links. Not skipped, and not marked.
        let n = u64::from(MAX_SKIPS) + 1;
        let f = watch.check(&lc, &failed(&url(n), gone, n), &now).unwrap();
        assert!(!f.skip);
        assert_eq!(lc.status(&url(n)), LinkStatus::Unknown);
        // Once something plays, skipping starts over.
        let playing = PlayerState { status: Status::Playing, load_id: n + 1, ..Default::default() };
        assert_eq!(watch.check(&lc, &playing, &now), None);
        assert!(watch.check(&lc, &failed(&url(n + 2), gone, n + 2), &now).unwrap().skip);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn listings_record_what_they_know() {
        let (lc, path) = temp_check("listing", Box::new(|_, _, _, _| panic!("no checks")));
        let items = [
            PlaylistItem {
                url: "https://www.youtube.com/watch?v=gone".into(),
                title: "[Deleted video]".into(),
                availability: Some(LinkStatus::Dead),
                ..Default::default()
            },
            PlaylistItem { url: "https://www.youtube.com/watch?v=ok".into(), title: "Ok".into(), ..Default::default() },
        ];
        lc.record_listing(&items);
        assert_eq!(lc.status("https://youtu.be/gone"), LinkStatus::Dead);
        assert_eq!(lc.status("https://youtu.be/ok"), LinkStatus::Unknown);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn url_encoding_for_oembed() {
        assert_eq!(encode("https://youtu.be/a?b=c&d"), "https%3A%2F%2Fyoutu.be%2Fa%3Fb%3Dc%26d");
    }
}
