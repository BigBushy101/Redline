//! A SoundCloud track or set, asked of SoundCloud's own API (api-v2, as its web player does)
//! instead of through yt-dlp: about 0.8 s a track where yt-dlp takes two seconds on a PC and
//! far more on a phone. Sets are listed whole in a few requests, where yt-dlp names their
//! tracks past the first few one by one. Anything this doesn't expect gives `None`, and the
//! caller asks yt-dlp as before.
//!
//! The API wants the web player's `client_id`, which (as yt-dlp does) is read from the scripts
//! soundcloud.com loads: about two seconds, once per run, and again if SoundCloud rotates it.

use std::sync::Mutex;

use super::ytdlp::{MediaInfo, NetworkOptions, PlaylistItem, StreamInfo};
use crate::platform::Platform;

const API: &str = "https://api-v2.soundcloud.com";

/// Most tracks one `/tracks?ids=` request returns.
const IDS_PER_REQUEST: usize = 50;

/// The web player's client ID, once read.
static CLIENT_ID: Mutex<Option<String>> = Mutex::new(None);

/// `url`'s stream, if it's a playable SoundCloud track (not a preview, not encrypted-only).
pub fn resolve(net: &NetworkOptions, url: &str) -> Option<StreamInfo> {
    let agent = agent_for(net, url)?;
    let t = get(&agent, "/resolve", &[("url", url)])?;
    let (transcoding, auth) = pick(&t)?;
    let path = transcoding.strip_prefix(API)?;
    let stream = get(&agent, path, &[("track_authorization", auth)])?;
    let info = track_info(&t)?;
    Some(StreamInfo {
        url: stream["url"].as_str()?.to_owned(),
        headers: None,
        title: info.title,
        artist: info.uploader,
        album: None,
        duration: info.duration,
        thumbnail: info.thumbnail,
    })
}

/// A SoundCloud set's tracks, for saving. `None` if it isn't a set, or any track of it
/// didn't come back (removed or private ones): a partial list would look like tracks removed.
pub fn set(net: &NetworkOptions, url: &str) -> Option<MediaInfo> {
    if !url.contains("/sets/") {
        return None;
    }
    let agent = agent_for(net, url)?;
    let s = get(&agent, "/resolve", &[("url", url)])?;
    if s["kind"] != "playlist" {
        return None;
    }
    let listed = s["tracks"].as_array()?;
    // Only the first few tracks come with details; the rest are asked for by ID.
    let missing: Vec<String> = listed.iter().filter(|t| t["title"].is_null()).filter_map(|t| t["id"].as_u64()).map(|id| id.to_string()).collect();
    let mut found = std::collections::HashMap::new();
    for ids in missing.chunks(IDS_PER_REQUEST) {
        for t in get(&agent, "/tracks", &[("ids", &ids.join(","))])?.as_array()? {
            found.insert(t["id"].as_u64()?, t.clone());
        }
    }
    let items = listed
        .iter()
        .map(|t| if t["title"].is_null() { found.get(&t["id"].as_u64()?).and_then(track_info) } else { track_info(t) })
        .collect::<Option<Vec<_>>>()?;
    if items.is_empty() {
        return None;
    }
    Some(MediaInfo {
        title: s["title"].as_str()?.to_owned(),
        uploader: s["user"]["username"].as_str().map(str::to_owned),
        thumbnail: cover(&s),
        is_playlist: true,
        entries: items.len(),
        items,
        extractor: Some("SoundcloudSet".into()),
        ..Default::default()
    })
}

fn agent_for(net: &NetworkOptions, url: &str) -> Option<ureq::Agent> {
    (Platform::of(url) == Platform::SoundCloud).then(|| super::youtube::agent(net)).flatten()
}

/// The best stream a track offers, and the authorization asking for it needs: a plain MP3
/// over a playlist of pieces, skipping previews and DRM-encrypted ones (ffmpeg can't play those).
fn pick(t: &serde_json::Value) -> Option<(&str, &str)> {
    if t["kind"] != "track" || t["policy"] == "SNIP" {
        return None;
    }
    let usable: Vec<&serde_json::Value> = t["media"]["transcodings"]
        .as_array()?
        .iter()
        .filter(|c| !c["snipped"].as_bool().unwrap_or(false) && matches!(c["format"]["protocol"].as_str(), Some("progressive" | "hls")))
        .collect();
    let best = usable.iter().find(|c| c["format"]["protocol"] == "progressive").or(usable.first())?;
    Some((best["url"].as_str()?, t["track_authorization"].as_str()?))
}

/// A track's details, named as yt-dlp names them (its uploader stands for the artist).
fn track_info(t: &serde_json::Value) -> Option<PlaylistItem> {
    Some(PlaylistItem {
        url: t["permalink_url"].as_str()?.to_owned(),
        title: t["title"].as_str()?.to_owned(),
        uploader: t["user"]["username"].as_str().map(str::to_owned),
        duration: t["duration"].as_f64().map(|ms| ms / 1000.0),
        thumbnail: cover(t),
        availability: None,
    })
}

/// The full-size cover (the API hands out its 100 px `-large` one), else the uploader's picture.
fn cover(t: &serde_json::Value) -> Option<String> {
    let art = t["artwork_url"].as_str().or(t["user"]["avatar_url"].as_str())?;
    Some(art.replace("-large.", "-original."))
}

/// An API answer, asking with a fresh client ID once if the kept one is refused.
fn get(agent: &ureq::Agent, path: &str, query: &[(&str, &str)]) -> Option<serde_json::Value> {
    let ask = |id: &str| {
        let mut req = agent.get(format!("{API}{path}")).query("client_id", id);
        for (k, v) in query {
            req = req.query(*k, *v);
        }
        req.call()
    };
    let kept = CLIENT_ID.lock().unwrap().clone();
    let mut answer = match &kept {
        Some(id) => ask(id),
        None => Err(ureq::Error::StatusCode(401)),
    };
    if matches!(answer, Err(ureq::Error::StatusCode(401 | 403))) {
        let fresh = client_id(agent)?;
        *CLIENT_ID.lock().unwrap() = Some(fresh.clone());
        answer = ask(&fresh);
    }
    serde_json::from_str(&answer.ok()?.body_mut().read_to_string().ok()?).ok()
}

/// Reads the web player's client ID from the scripts soundcloud.com loads (it's in one of the
/// last few).
fn client_id(agent: &ureq::Agent) -> Option<String> {
    let page = text(agent, "https://soundcloud.com/")?;
    const ASSETS: &str = "src=\"https://a-v2.sndcdn.com/assets/";
    let scripts: Vec<&str> = page.split(ASSETS).skip(1).filter_map(|s| s.split_once('"').map(|(name, _)| name)).collect();
    scripts.iter().rev().find_map(|name| id_in(&text(agent, &format!("{}{name}", &ASSETS[5..]))?))
}

fn id_in(script: &str) -> Option<String> {
    let after = script.split_once("client_id:\"")?.1;
    let id: String = after.chars().take_while(char::is_ascii_alphanumeric).collect();
    (id.len() >= 20).then_some(id)
}

fn text(agent: &ureq::Agent, url: &str) -> Option<String> {
    agent.get(url).call().ok()?.body_mut().read_to_string().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_client_id() {
        assert_eq!(id_in(r#"x={client_id:"AbCdEfGhIjKlMnOpQrStUvWx12",env:"p"}"#).as_deref(), Some("AbCdEfGhIjKlMnOpQrStUvWx12"));
        assert_eq!(id_in(r#"client_id:"short""#), None);
    }

    #[test]
    fn picks_a_stream_and_skips_previews() {
        let t = serde_json::json!({
            "kind": "track", "policy": "ALLOW", "track_authorization": "auth",
            "title": "Song", "permalink_url": "https://soundcloud.com/a/song", "duration": 143206,
            "user": {"username": "Band"}, "artwork_url": "https://i1.sndcdn.com/artworks-1-large.jpg",
            "media": {"transcodings": [
                {"url": "drm", "format": {"protocol": "ctr-encrypted-hls"}},
                {"url": "hls", "format": {"protocol": "hls"}},
                {"url": "mp3", "format": {"protocol": "progressive"}}
            ]}
        });
        assert_eq!(pick(&t), Some(("mp3", "auth")));
        let i = track_info(&t).unwrap();
        assert_eq!((i.title.as_str(), i.uploader.as_deref(), i.duration), ("Song", Some("Band"), Some(143.206)));
        assert_eq!(i.thumbnail.as_deref(), Some("https://i1.sndcdn.com/artworks-1-original.jpg"));
        let mut preview = t.clone();
        preview["policy"] = "SNIP".into();
        assert_eq!(pick(&preview), None);
        let mut drm_only = t.clone();
        drm_only["media"]["transcodings"] = serde_json::json!([{"url": "drm", "format": {"protocol": "ctr-encrypted-hls"}}]);
        assert_eq!(pick(&drm_only), None);
    }

    #[test]
    #[ignore = "network"]
    fn live_track_and_set() {
        let net = NetworkOptions::default();
        for _ in 0..2 {
            let t = std::time::Instant::now();
            let s = resolve(&net, "https://soundcloud.com/ethmusic/lostin-powers-she-so-heavy").expect("a stream");
            println!("{} by {:?} in {:?}", s.title, s.artist, t.elapsed());
        }
        let t = std::time::Instant::now();
        let set = set(&net, "https://soundcloud.com/the-concept-band/sets/the-royal-concept-ep").expect("a set");
        println!("set {} of {} tracks in {:?}", set.title, set.items.len(), t.elapsed());
        assert!(set.items.iter().all(|i| i.title != i.url));
    }
}
