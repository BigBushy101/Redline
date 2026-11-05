//! What the player keeps on disk between runs, so a link played before starts without a
//! look-up:
//! - Stream URLs, until shortly before they expire. One saved in an earlier run is checked
//!   with a one-byte request before it's used: YouTube's are tied to the network they were
//!   looked up from, and a phone moves between networks.
//! - Played links' audio, as downloaded (not decoded: a few MB a song), up to [`MAX_BYTES`] in
//!   all, the longest unplayed dropped first. It's fetched in [`CHUNK`]-sized ranges in the
//!   background while the song plays from its URL, so a dropped connection costs one piece.
//!
//! Android keeps them in its cache folder, which the system may clear; desktop in the
//! platform's cache folder.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::decode::Input;
use super::{Resolved, TrackInfo};
use crate::tools::ytdlp::NetworkOptions;

/// The most audio kept, in bytes.
pub const MAX_BYTES: u64 = 512 * 1024 * 1024;
/// One song's audio bigger than this isn't kept (a long mix, or a video stream).
const MAX_FILE: u64 = 64 * 1024 * 1024;
/// Bytes asked for per request.
const CHUNK: u64 = 5 * 1024 * 1024;
/// Tries per piece before the download is given up.
const TRIES: u32 = 3;
/// A stream URL is reused until this long before its expiry.
const EXPIRY_MARGIN: u64 = 30 * 60;
/// How long a stream URL that doesn't say when it expires is reused.
const DEFAULT_LIFE: u64 = 60 * 60;
const TIMEOUT: Duration = Duration::from_secs(20);
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// A looked-up link as saved.
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    location: String,
    headers: Option<String>,
    title: String,
    artist: Option<String>,
    album: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    /// Unix seconds after which the URL isn't used. Unset for audio files.
    #[serde(default)]
    until: u64,
}

impl Saved {
    fn new(r: &Resolved, until: u64) -> Self {
        Self {
            location: r.input.location.clone(),
            headers: r.input.headers.clone(),
            title: r.info.title.clone(),
            artist: r.info.artist.clone(),
            album: r.info.album.clone(),
            duration: r.info.duration,
            thumbnail: r.info.thumbnail.clone(),
            until,
        }
    }

    fn resolved(self) -> Resolved {
        Resolved {
            input: Input {
                location: self.location,
                headers: self.headers,
            },
            info: TrackInfo {
                title: self.title,
                artist: self.artist,
                album: self.album,
                duration: self.duration,
                thumbnail: self.thumbnail,
            },
        }
    }
}

/// `url`'s audio, if it was kept.
pub fn stored_audio(url: &str) -> Option<Resolved> {
    stored_audio_in(&root(), url)
}

/// `url`'s stream URL from an earlier look-up, if it hasn't expired and still answers.
pub fn stored_link(url: &str, net: &NetworkOptions) -> Option<Resolved> {
    let key = key(url);
    let saved = {
        let mut links = LINKS.lock().unwrap();
        let links = links.get_or_insert_with(|| load_links(&root()));
        links.get(&key).filter(|s| s.until > now()).cloned()?
    };
    if answers(&saved, net) {
        return Some(saved.resolved());
    }
    forget_link(url);
    None
}

/// Saves `url`'s fresh look-up, and starts keeping its audio.
pub fn remember(url: &str, r: &Resolved, net: &NetworkOptions) {
    let saved = Saved::new(r, expiry(&r.input.location, now()));
    {
        let mut links = LINKS.lock().unwrap();
        let links = links.get_or_insert_with(|| load_links(&root()));
        links.insert(key(url), saved);
        save_links(&root(), links);
    }
    keep_audio(url, r, net);
}

pub fn forget_link(url: &str) {
    let mut links = LINKS.lock().unwrap();
    if let Some(links) = links.as_mut()
        && links.remove(&key(url)).is_some()
    {
        save_links(&root(), links);
    }
}

/// Downloads `url`'s audio from its looked-up stream in the background, unless it's kept
/// already, being fetched, or can't be (a playlist manifest, too long, a rate limit set).
pub fn keep_audio(url: &str, r: &Resolved, net: &NetworkOptions) {
    let location = &r.input.location;
    let streamable = location.starts_with("http") && !location.contains(".m3u8") && !location.contains("manifest");
    let too_long = r.info.duration.is_some_and(|d| d > super::cache::MAX_SECS);
    // A rate limit is a wish to spend less bandwidth, and this would spend some twice.
    if !streamable || too_long || !net.rate_limit.trim().is_empty() {
        return;
    }
    let dir = root().join("audio");
    let key = key(url);
    if dir.join(format!("{key}.json")).is_file() || !FETCHING.lock().unwrap().insert(key.clone()) {
        return;
    }
    let (r, net) = (r.clone(), net.clone());
    thread::spawn(move || {
        let _ = save_audio(&dir, &key, &r, &net);
        FETCHING.lock().unwrap().remove(&key);
        prune(&dir, MAX_BYTES);
    });
}

static LINKS: Mutex<Option<HashMap<String, Saved>>> = Mutex::new(None);
/// Links whose audio is being downloaded.
static FETCHING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn root() -> PathBuf {
    #[cfg(not(target_os = "android"))]
    if let Some(d) = directories::BaseDirs::new() {
        return d.cache_dir().join("Redline");
    }
    crate::tools::temp_dir().join("redline-cache")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A file name for a link: the same for every spelling of it.
fn key(url: &str) -> String {
    // FNV-1a: stable across runs and builds, unlike std's hasher.
    let hash = crate::platform::canonical(url)
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}")
}

/// When a stream URL stops being reused: YouTube's say when they expire (`expire=`).
fn expiry(location: &str, now: u64) -> u64 {
    let query = location.split_once('?').map_or("", |(_, q)| q);
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("expire="))
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(now + DEFAULT_LIFE, |e| e.saturating_sub(EXPIRY_MARGIN))
}

fn load_links(dir: &Path) -> HashMap<String, Saved> {
    let mut links: HashMap<String, Saved> = fs::read(dir.join("links.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let now = now();
    links.retain(|_, s| s.until > now);
    links
}

fn save_links(dir: &Path, links: &HashMap<String, Saved>) {
    let now = now();
    let live: HashMap<_, _> = links.iter().filter(|(_, s)| s.until > now).collect();
    let _ = write_atomic(&dir.join("links.json"), &serde_json::to_vec(&live).unwrap_or_default());
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn agent(net: &NetworkOptions, timeout: Duration) -> Result<ureq::Agent> {
    let mut config = ureq::Agent::config_builder().timeout_global(Some(timeout));
    if !net.proxy.trim().is_empty() {
        config = config.proxy(Some(ureq::Proxy::new(net.proxy.trim())?));
    }
    Ok(config.build().into())
}

/// `Name: value\r\n` lines, as ffmpeg takes them, as pairs.
fn header_pairs(headers: Option<&str>) -> Vec<(&str, &str)> {
    headers
        .unwrap_or_default()
        .split("\r\n")
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

fn get(agent: &ureq::Agent, s: &Saved, range: &str) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let mut req = agent.get(&s.location).header("Range", range);
    for (k, v) in header_pairs(s.headers.as_deref()) {
        req = req.header(k, v);
    }
    req.call()
}

/// Whether a saved stream URL still gives audio.
fn answers(s: &Saved, net: &NetworkOptions) -> bool {
    let Ok(agent) = agent(net, CHECK_TIMEOUT) else { return false };
    get(&agent, s, "bytes=0-0").is_ok_and(|r| r.status().is_success())
}

fn stored_audio_in(root: &Path, url: &str) -> Option<Resolved> {
    let dir = root.join("audio");
    let key = key(url);
    let audio = dir.join(format!("{key}.audio"));
    let saved: Saved = serde_json::from_slice(&fs::read(dir.join(format!("{key}.json"))).ok()?).ok()?;
    if !audio.is_file() {
        return None;
    }
    // Played now: the last to go when space runs out.
    if let Ok(f) = File::options().append(true).open(&audio) {
        let _ = f.set_modified(SystemTime::now());
    }
    let mut r = saved.resolved();
    r.input = Input {
        location: audio.display().to_string(),
        headers: None,
    };
    Some(r)
}

fn save_audio(dir: &Path, key: &str, r: &Resolved, net: &NetworkOptions) -> Result<()> {
    fs::create_dir_all(dir)?;
    let part = dir.join(format!("{key}.part"));
    let saved = Saved::new(r, 0);
    let result = fetch(&agent(net, TIMEOUT)?, &saved, &part, CHUNK);
    if result.is_err() {
        let _ = fs::remove_file(&part);
        return result;
    }
    fs::rename(&part, dir.join(format!("{key}.audio")))?;
    // Written last: an audio file counts as kept only once this is there.
    write_atomic(&dir.join(format!("{key}.json")), &serde_json::to_vec(&saved)?)
}

/// Downloads `s` into `path`, `chunk` bytes per request, each tried [`TRIES`] times.
fn fetch(agent: &ureq::Agent, s: &Saved, path: &Path, chunk: u64) -> Result<()> {
    let mut file = File::create(path)?;
    let mut at = 0u64;
    let mut total = None;
    let mut fails = 0;
    while total.is_none_or(|t| at < t) {
        let range = format!("bytes={at}-{}", at + chunk - 1);
        let piece = get(agent, s, &range).map_err(anyhow::Error::from).and_then(|mut resp| {
            if resp.status().as_u16() != 206 {
                bail!("no ranges ({})", resp.status());
            }
            let size = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit_once('/'))
                .and_then(|(_, t)| t.parse::<u64>().ok())
                .context("no length")?;
            let mut bytes = Vec::new();
            resp.body_mut().as_reader().take(chunk).read_to_end(&mut bytes)?;
            Ok((size, bytes))
        });
        match piece {
            Ok((size, bytes)) => {
                if size > MAX_FILE {
                    bail!("too big to keep");
                }
                if bytes.is_empty() && at < size {
                    bail!("empty piece");
                }
                total = Some(size);
                file.write_all(&bytes)?;
                at += bytes.len() as u64;
                fails = 0;
            }
            Err(e) => {
                fails += 1;
                if fails >= TRIES {
                    return Err(e);
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
    file.sync_all()?;
    Ok(())
}

/// Drops the longest-unplayed audio until the rest fits in `max` bytes, and downloads a crash
/// left behind.
fn prune(dir: &Path, max: u64) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut kept = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        let modified = meta.modified().unwrap_or(UNIX_EPOCH);
        match path.extension().and_then(|x| x.to_str()) {
            Some("audio") => kept.push((modified, meta.len(), path)),
            Some("part") if modified.elapsed().is_ok_and(|a| a > Duration::from_secs(3600)) => {
                let _ = fs::remove_file(&path);
            }
            _ => {}
        }
    }
    kept.sort();
    let mut total: u64 = kept.iter().map(|(_, len, _)| len).sum();
    for (_, len, path) in kept {
        if total <= max {
            break;
        }
        // The description first, so a half-removed song isn't taken for kept.
        let _ = fs::remove_file(path.with_extension("json"));
        if fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("redline-disk-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keys_ignore_the_spelling() {
        assert_eq!(key("https://youtu.be/dQw4w9WgXcQ?si=a"), key("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert_ne!(key("https://youtu.be/dQw4w9WgXcQ"), key("https://youtu.be/aaaaaaaaaaa"));
    }

    #[test]
    fn expiry_comes_from_the_url() {
        assert_eq!(expiry("https://x.googlevideo.com/videoplayback?expire=100000&c=X", 5), 100000 - EXPIRY_MARGIN);
        assert_eq!(expiry("https://cdn.example/a.mp3", 5), 5 + DEFAULT_LIFE);
    }

    /// Serves `body` with byte ranges, `fail_first` requests answered with a cut connection.
    fn serve(body: Vec<u8>, mut fail_first: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(r) = line.to_lowercase().strip_prefix("range: bytes=") {
                        let (a, b) = r.trim().split_once('-').unwrap();
                        range = Some((a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()));
                    }
                }
                let mut stream = stream;
                if fail_first > 0 {
                    fail_first -= 1;
                    continue;
                }
                let (a, b) = range.unwrap();
                let b = b.min(body.len() - 1);
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {a}-{b}/{}\r\nConnection: close\r\n\r\n",
                    b - a + 1,
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body[a..=b]);
            }
        });
        format!("http://{addr}/song")
    }

    fn saved(location: String) -> Saved {
        Saved {
            location,
            headers: Some("User-Agent: test\r\n".into()),
            title: "Song".into(),
            artist: None,
            album: None,
            duration: Some(3.0),
            thumbnail: None,
            until: 0,
        }
    }

    #[test]
    fn fetches_in_pieces_and_recovers_from_a_drop() {
        let body: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let url = serve(body.clone(), 1);
        let dir = scratch("fetch");
        let path = dir.join("a.part");
        let agent = agent(&NetworkOptions::default(), TIMEOUT).unwrap();
        fetch(&agent, &saved(url), &path, 4096).unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
    }

    #[test]
    fn kept_audio_is_found_and_pruned_oldest_first() {
        let root = scratch("prune");
        let dir = root.join("audio");
        let body = vec![7u8; 3000];
        let url = serve(body.clone(), 0);
        let r = saved(url).resolved();
        let net = NetworkOptions::default();
        save_audio(&dir, &key("https://youtu.be/aaaaaaaaaaa"), &r, &net).unwrap();
        let found = stored_audio_in(&root, "https://www.youtube.com/watch?v=aaaaaaaaaaa").unwrap();
        assert_eq!(fs::read(&found.input.location).unwrap(), body);
        assert_eq!(found.input.headers, None);
        assert_eq!(found.info.title, "Song");

        let old = dir.join(format!("{}.audio", key("https://youtu.be/aaaaaaaaaaa")));
        File::options().append(true).open(&old).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(10)).unwrap();
        save_audio(&dir, &key("https://youtu.be/bbbbbbbbbbb"), &r, &net).unwrap();
        prune(&dir, 4000);
        assert!(stored_audio_in(&root, "https://youtu.be/aaaaaaaaaaa").is_none());
        assert!(stored_audio_in(&root, "https://youtu.be/bbbbbbbbbbb").is_some());
    }
}
