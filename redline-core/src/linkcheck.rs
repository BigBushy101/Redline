//! Whether saved links still play, found out before they're played, and stand-ins for the
//! ones that don't.
//!
//! YouTube links are checked with its oEmbed endpoint (one small HTTP request, no yt-dlp);
//! other sites with a `yt-dlp --simulate` run, less often. Playlist listings already mark
//! deleted and private entries, which [`ytdlp::fetch_info`] records here as it reads them.
//! Results are kept in `linkcheck.json` for about a week.
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

use crate::library::Track;
use crate::platform::{self, Platform};
use crate::player::Source;
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

/// What an oEmbed reply means. `None` is no reply at all (a network error).
pub fn status_for_oembed(code: Option<u16>) -> LinkStatus {
    match code {
        Some(200) => LinkStatus::Alive,
        Some(400 | 404) => LinkStatus::Dead,
        Some(401 | 403) => LinkStatus::Restricted,
        _ => LinkStatus::Unknown,
    }
}

/// What a failed `yt-dlp --simulate` says about the link. Bot checks and network trouble say
/// nothing about it.
pub fn status_for_ytdlp_error(msg: &str) -> LinkStatus {
    let m = msg.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| m.contains(w));
    if any(&["not a bot", "timed out", "unable to download", "connection", "network", "getaddrinfo", "429"]) {
        return LinkStatus::Unknown;
    }
    if any(&["private", "sign in", "log in", "login", "members-only", "members only", "join this channel", "premium"]) {
        return LinkStatus::Restricted;
    }
    if any(&[
        "unavailable",
        "removed",
        "no longer available",
        "does not exist",
        "not found",
        "404",
        "has been terminated",
        "deleted",
    ]) {
        return LinkStatus::Dead;
    }
    LinkStatus::Unknown
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
pub fn check_youtube(url: &str, net: &NetworkOptions) -> LinkStatus {
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false);
    if !net.proxy.trim().is_empty() {
        match ureq::Proxy::new(net.proxy.trim()) {
            Ok(p) => config = config.proxy(Some(p)),
            Err(_) => return LinkStatus::Unknown,
        }
    }
    let agent: ureq::Agent = config.build().into();
    let endpoint = format!("https://www.youtube.com/oembed?url={}&format=json", encode(url));
    status_for_oembed(agent.get(&endpoint).call().ok().map(|r| r.status().as_u16()))
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
pub fn check_other(tools: &ToolPaths, net: &NetworkOptions, url: &str) -> LinkStatus {
    let Ok(mut cmd) = ytdlp::base_command(tools, net) else { return LinkStatus::Unknown };
    cmd.args(["--simulate", "-J", "--no-playlist", "--", url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let Ok(mut child) = cmd.spawn() else { return LinkStatus::Unknown };
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
        Some(s) if s.success() => LinkStatus::Alive,
        Some(_) => {
            let line = err.lines().rev().find(|l| l.starts_with("ERROR:")).unwrap_or(&err);
            status_for_ytdlp_error(line)
        }
        None => LinkStatus::Unknown,
    }
}

/// Checks one link the way its site allows.
pub fn check_link(tools: &ToolPaths, net: &NetworkOptions, url: &str) -> LinkStatus {
    if Platform::of(url) == Platform::YouTube {
        check_youtube(url, net)
    } else {
        check_other(tools, net, url)
    }
}

/// How long a check holds: a week for YouTube, two for sites checked through yt-dlp.
fn max_age(url: &str) -> i64 {
    const DAY: i64 = 24 * 60 * 60;
    if Platform::of(url) == Platform::YouTube { 7 * DAY } else { 14 * DAY }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Checked {
    status: LinkStatus,
    /// Unix seconds.
    at: i64,
}

type CheckFn = dyn Fn(&ToolPaths, &NetworkOptions, &str) -> LinkStatus + Send + Sync;

/// How many links are checked at once.
const WORKERS: usize = 3;

struct State {
    /// By canonical link. Loaded on first use, once the app has set its folders.
    cache: Option<HashMap<String, Checked>>,
    dirty: bool,
    /// (key, link) waiting for a worker.
    queue: VecDeque<(String, String)>,
    /// Keys queued or being checked.
    pending: HashSet<String>,
    /// Keys whose check couldn't tell this session; not asked about again until restart.
    tried: HashSet<String>,
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
        Self {
            state: Mutex::new(State {
                cache: None,
                dirty: false,
                queue: VecDeque::new(),
                pending: HashSet::new(),
                tried: HashSet::new(),
                workers: 0,
                wake: no_wake(),
                tools: ToolPaths::default(),
                net: NetworkOptions::default(),
            }),
            version: AtomicU64::new(0),
            path,
            check,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.cache.is_none() {
            s.cache = Some(load(&self.path));
        }
        s
    }

    fn cache<'a>(s: &'a mut MutexGuard<'_, State>) -> &'a mut HashMap<String, Checked> {
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

    /// [`Self::record`] for many links, saved once.
    pub fn record_all<'a>(&self, results: impl IntoIterator<Item = (&'a str, LinkStatus)>) {
        let mut s = self.lock();
        let mut changed = false;
        for (url, status) in results {
            if status == LinkStatus::Unknown {
                continue;
            }
            let key = platform::canonical(url);
            Self::cache(&mut s).insert(key, Checked { status, at: now() });
            changed = true;
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
    /// go first: they're quick.
    pub fn request<'a>(self: &Arc<Self>, tracks: impl IntoIterator<Item = &'a Track>) {
        self.request_at(tracks, now());
    }

    fn request_at<'a>(self: &Arc<Self>, tracks: impl IntoIterator<Item = &'a Track>, now: i64) {
        let mut s = self.lock();
        let mut fresh: Vec<(String, String)> = Vec::new();
        for t in tracks {
            let Source::Link(url) = &t.source else { continue };
            let key = platform::canonical(url);
            let known = Self::cache(&mut s).get(&key).is_some_and(|c| now - c.at < max_age(url));
            if known || s.pending.contains(&key) || s.tried.contains(&key) {
                continue;
            }
            s.pending.insert(key.clone());
            fresh.push((key, url.clone()));
        }
        fresh.sort_by_key(|(_, u)| Platform::of(u) != Platform::YouTube);
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
            let (key, url, tools, net) = {
                let mut s = self.lock();
                match s.queue.pop_front() {
                    Some((key, url)) => (key, url, s.tools.clone(), s.net.clone()),
                    None => {
                        s.workers -= 1;
                        if s.workers == 0 {
                            self.save(&mut s);
                        }
                        return;
                    }
                }
            };
            let status = (self.check)(&tools, &net, &url);
            let wake = {
                let mut s = self.lock();
                s.pending.remove(&key);
                if status == LinkStatus::Unknown {
                    s.tried.insert(key);
                } else {
                    Self::cache(&mut s).insert(key, Checked { status, at: now() });
                    s.dirty = true;
                }
                s.wake.clone()
            };
            self.version.fetch_add(1, Ordering::Relaxed);
            wake();
        }
    }

    /// Whether checks are still running.
    pub fn busy(&self) -> bool {
        self.lock().workers > 0
    }

    fn save(&self, s: &mut MutexGuard<'_, State>) {
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
    }

    #[test]
    fn ytdlp_errors() {
        let s = status_for_ytdlp_error;
        assert_eq!(s("ERROR: [youtube] abc: Video unavailable"), LinkStatus::Dead);
        assert_eq!(s("ERROR: [soundcloud] x: Unable to download JSON metadata: HTTP Error 404: Not Found"), LinkStatus::Unknown);
        assert_eq!(s("ERROR: [soundcloud] x: HTTP Error 404: Not Found"), LinkStatus::Dead);
        assert_eq!(s("ERROR: [youtube] abc: Private video. Sign in if you've been granted access"), LinkStatus::Restricted);
        assert_eq!(s("ERROR: [youtube] abc: Video unavailable. This video is private"), LinkStatus::Restricted);
        assert_eq!(s("ERROR: [youtube] abc: Sign in to confirm you're not a bot"), LinkStatus::Unknown);
        assert_eq!(s("ERROR: something odd"), LinkStatus::Unknown);
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

    fn temp_check(name: &str, check: Box<CheckFn>) -> (Arc<LinkCheck>, PathBuf) {
        let path = std::env::temp_dir().join(format!("redline-linkcheck-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        (Arc::new(LinkCheck::new(path.clone(), check)), path)
    }

    fn wait(lc: &LinkCheck) {
        let until = Instant::now() + Duration::from_secs(5);
        while lc.busy() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn checks_in_the_background_and_caches() {
        let calls = Arc::new(AtomicU64::new(0));
        let counter = calls.clone();
        let (lc, path) = temp_check(
            "bg",
            Box::new(move |_, _, url| {
                counter.fetch_add(1, Ordering::Relaxed);
                match url {
                    u if u.ends_with("dead") => LinkStatus::Dead,
                    u if u.ends_with("offline") => LinkStatus::Unknown,
                    _ => LinkStatus::Alive,
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
        assert_eq!(calls.load(Ordering::Relaxed), 3, "files aren't checked");
        assert_eq!(lc.statuses(&ts), [LinkStatus::Dead, LinkStatus::Alive, LinkStatus::Unknown, LinkStatus::Unknown]);
        assert_eq!(lc.status("https://www.youtube.com/watch?v=dead&t=4"), LinkStatus::Dead, "by canonical link");
        assert!(lc.version() >= 3);

        lc.request(&ts);
        wait(&lc);
        assert_eq!(calls.load(Ordering::Relaxed), 3, "fresh results and failed tries aren't asked again");

        // A week later the YouTube results are stale; SoundCloud's would last two.
        lc.request_at(&ts, now() + 8 * 24 * 3600);
        wait(&lc);
        assert_eq!(calls.load(Ordering::Relaxed), 5);

        // Saved, and loaded back by a new checker.
        let back = LinkCheck::new(path.clone(), Box::new(|_, _, _| LinkStatus::Unknown));
        assert_eq!(back.status("https://youtu.be/dead"), LinkStatus::Dead);
        assert_eq!(back.status("https://youtu.be/live"), LinkStatus::Alive);
        let (kept, start) = back.drop_dead(ts.to_vec(), 0);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[start].title, "b");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn listings_record_what_they_know() {
        let (lc, path) = temp_check("listing", Box::new(|_, _, _| panic!("no checks")));
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
