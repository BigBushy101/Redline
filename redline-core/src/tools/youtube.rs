//! A YouTube video's stream, asked of YouTube directly instead of through yt-dlp: one request
//! to the player API as the visionOS client, which hands out audio URLs that need no player
//! download, JS challenge or PO token. That's about half a second where a yt-dlp look-up is
//! several (Python on a phone, plus yt-dlp's other work). Anything this doesn't expect gives
//! `None`, and the caller asks yt-dlp as before.
//!
//! Each request carries an anonymous visitor ID, as YouTube's own apps (and yt-dlp) send:
//! asked without one, YouTube answers most less-watched videos with its bot check. The first
//! comes from YouTube's answer to a request without one, and is reused while it works.
//!
//! The client's details are copied from yt-dlp (`INNERTUBE_CLIENTS['visionos']` in
//! `yt_dlp/extractor/youtube/_base.py`). If YouTube stops answering this client, every look-up
//! falls back to yt-dlp; update them from there.

use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use super::ytdlp::{MediaInfo, NetworkOptions, PlaylistItem, StreamInfo};

const PLAYER_API: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const BROWSE_API: &str = "https://www.youtube.com/youtubei/v1/browse?prettyPrint=false";
const CLIENT_NAME: &str = "VISIONOS";
const CLIENT_ID: &str = "101";
const CLIENT_VERSION: &str = "1.02";
const DEVICE_MAKE: &str = "Apple";
const DEVICE_MODEL: &str = "RealityDevice17,1";
const OS_NAME: &str = "visionOS";
const OS_VERSION: &str = "26.5.23O471";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

const TIMEOUT: Duration = Duration::from_secs(10);

/// Most pages of a playlist read (20 songs each); a longer one is left to yt-dlp.
const MAX_PAGES: usize = 250;

/// The visitor ID requests carry, once YouTube handed one out.
static VISITOR: Mutex<Option<String>> = Mutex::new(None);

/// The video ID of a YouTube video link, from any of its shapes.
pub fn video_id(url: &str) -> Option<String> {
    let canonical = crate::platform::canonical(url);
    let id = canonical.strip_prefix("https://www.youtube.com/watch?v=")?;
    (id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).then(|| id.to_owned())
}

/// `url`'s best audio stream, signed out. `None` when it isn't a YouTube video, or YouTube
/// didn't hand one over; yt-dlp is asked then.
pub fn resolve(net: &NetworkOptions, url: &str) -> Option<StreamInfo> {
    let id = video_id(url)?;
    let agent = agent(net)?;
    let visitor = VISITOR.lock().unwrap().clone();
    let text = ask(&agent, &id, visitor.as_deref())?;
    if let Some(stream) = parse(&text) {
        if visitor.is_none() {
            *VISITOR.lock().unwrap() = visitor_of(&text);
        }
        return Some(stream);
    }
    // Refused: once more with the visitor ID this answer handed out, if it's a new one.
    let fresh = visitor_of(&text).filter(|v| Some(v) != visitor.as_ref())?;
    *VISITOR.lock().unwrap() = Some(fresh.clone());
    parse(&ask(&agent, &id, Some(&fresh))?)
}

pub(super) fn agent(net: &NetworkOptions) -> Option<ureq::Agent> {
    let mut config = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT));
    if !net.proxy.trim().is_empty() {
        config = config.proxy(Some(ureq::Proxy::new(net.proxy.trim()).ok()?));
    }
    Some(config.build().into())
}

/// A YouTube playlist link's songs, listed as the visionOS client sees them (the web client's
/// layout is newer and keeps changing). `None` when it isn't a playlist link, or any page of it
/// didn't come: a partial list would look like songs removed from it. yt-dlp lists it then.
pub fn playlist(net: &NetworkOptions, url: &str) -> Option<MediaInfo> {
    let canonical = crate::platform::canonical(url);
    let id = canonical.strip_prefix("https://www.youtube.com/playlist?list=")?;
    let agent = agent(net)?;
    let visitor = VISITOR.lock().unwrap().clone();
    let browse = |api: &str, body| -> Option<serde_json::Value> {
        serde_json::from_str(&call(&agent, api, body, visitor.as_deref())?).ok()
    };
    let first = browse(BROWSE_API, serde_json::json!({"browseId": format!("VL{id}")}))?;
    let mut items = Vec::new();
    let mut next = read_page(
        &first["contents"]["singleColumnBrowseResultsRenderer"]["tabs"][0]["tabRenderer"]["content"]
            ["sectionListRenderer"]["contents"][0]["playlistVideoListRenderer"],
        &mut items,
    )?;
    for _ in 0..MAX_PAGES {
        let Some(token) = next else { break };
        // The token comes URL-encoded, ready for the query string.
        let page = browse(&format!("{BROWSE_API}&ctoken={token}&continuation={token}&type=next"), serde_json::json!({}))?;
        next = read_page(&page["continuationContents"]["playlistVideoListContinuation"], &mut items)?;
    }
    if next.is_some() || items.is_empty() {
        return None;
    }
    let header = &first["header"]["playlistHeaderRenderer"];
    Some(MediaInfo {
        title: text(&header["title"])?,
        uploader: text(&header["ownerText"]),
        is_playlist: true,
        entries: items.len(),
        items,
        extractor: Some("youtube:tab".into()),
        ..Default::default()
    })
}

/// Adds a page's songs to `items`. `None` if it isn't a page of a playlist, else the token
/// of the page after it, if there is one.
fn read_page(list: &serde_json::Value, items: &mut Vec<PlaylistItem>) -> Option<Option<String>> {
    for entry in list["contents"].as_array()? {
        let v = &entry["playlistVideoRenderer"];
        let Some(id) = v["videoId"].as_str() else { continue };
        let url = format!("https://www.youtube.com/watch?v={id}");
        let title = text(&v["title"]);
        items.push(PlaylistItem {
            availability: crate::linkcheck::status_from_flat(title.as_deref(), None),
            title: title.unwrap_or_else(|| url.clone()),
            uploader: text(&v["shortBylineText"]),
            duration: v["lengthSeconds"].as_str().and_then(|s| s.parse().ok()),
            thumbnail: v["thumbnail"]["thumbnails"].as_array().and_then(|t| t.last()).and_then(|t| t["url"].as_str()).map(str::to_owned),
            url,
        });
    }
    Some(list["continuations"][0]["nextContinuationData"]["continuation"].as_str().map(str::to_owned))
}

/// A `{"runs": [{"text": ..}, ..]}` (or `{"simpleText": ..}`) label, joined.
fn text(v: &serde_json::Value) -> Option<String> {
    let s = match v["runs"].as_array() {
        Some(runs) => runs.iter().filter_map(|r| r["text"].as_str()).collect(),
        None => v["simpleText"].as_str()?.to_owned(),
    };
    (!s.is_empty()).then_some(s)
}

/// One player request for video `id`; YouTube's JSON answer.
fn ask(agent: &ureq::Agent, id: &str, visitor: Option<&str>) -> Option<String> {
    let body = serde_json::json!({"videoId": id, "contentCheckOk": true, "racyCheckOk": true});
    call(agent, PLAYER_API, body, visitor)
}

/// One request to YouTube's API as the visionOS client, `body` plus the client's context.
fn call(agent: &ureq::Agent, api: &str, mut body: serde_json::Value, visitor: Option<&str>) -> Option<String> {
    let mut client = serde_json::json!({
        "clientName": CLIENT_NAME,
        "clientVersion": CLIENT_VERSION,
        "deviceMake": DEVICE_MAKE,
        "deviceModel": DEVICE_MODEL,
        "osName": OS_NAME,
        "osVersion": OS_VERSION,
        "hl": "en",
    });
    let mut req = agent
        .post(api)
        .header("User-Agent", USER_AGENT)
        .header("X-YouTube-Client-Name", CLIENT_ID)
        .header("X-YouTube-Client-Version", CLIENT_VERSION)
        .header("Origin", "https://www.youtube.com");
    if let Some(v) = visitor {
        client["visitorData"] = v.into();
        req = req.header("X-Goog-Visitor-Id", v);
    }
    body["context"] = serde_json::json!({"client": client});
    req.content_type("application/json")
        .send(body.to_string())
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()
}

/// The visitor ID a player answer hands out.
fn visitor_of(json: &str) -> Option<String> {
    let r: serde_json::Value = serde_json::from_str(json).ok()?;
    r["responseContext"]["visitorData"].as_str().filter(|v| !v.is_empty()).map(str::to_owned)
}

fn parse(json: &str) -> Option<StreamInfo> {
    let r: Player = serde_json::from_str(json).ok()?;
    if r.playability.status != "OK" {
        return None;
    }
    let details = r.details?;
    // Live streams come as HLS, which yt-dlp handles.
    if details.is_live {
        return None;
    }
    let best = r
        .streaming?
        .adaptive
        .into_iter()
        .filter(|f| f.mime.starts_with("audio/") && f.url.is_some())
        // Dubbed tracks and the loudness-compressed (DRC) copies aren't the real thing.
        .filter(|f| f.track.as_ref().is_none_or(|t| t.default) && !f.drc)
        .max_by_key(|f| f.bitrate)?;
    let duration = best
        .duration_ms
        .and_then(|ms| ms.parse::<f64>().ok())
        .map(|ms| ms / 1000.0)
        .or_else(|| details.length.parse().ok());
    let author = details.author.map(|a| a.strip_suffix(" - Topic").map_or(a.clone(), str::to_owned));
    Some(StreamInfo {
        url: best.url?,
        headers: Some(format!("User-Agent: {USER_AGENT}\r\n")),
        title: details.title.unwrap_or_else(|| "Untitled".into()),
        artist: author,
        album: None,
        duration,
        thumbnail: details.thumbnail.thumbnails.into_iter().last().map(|t| t.url),
    })
}

#[derive(Deserialize)]
struct Player {
    #[serde(rename = "playabilityStatus")]
    playability: Playability,
    #[serde(rename = "streamingData")]
    streaming: Option<Streaming>,
    #[serde(rename = "videoDetails")]
    details: Option<Details>,
}

#[derive(Deserialize)]
struct Playability {
    status: String,
}

#[derive(Deserialize)]
struct Streaming {
    #[serde(rename = "adaptiveFormats", default)]
    adaptive: Vec<Format>,
}

#[derive(Deserialize)]
struct Format {
    #[serde(rename = "mimeType")]
    mime: String,
    url: Option<String>,
    #[serde(default)]
    bitrate: u64,
    #[serde(rename = "approxDurationMs")]
    duration_ms: Option<String>,
    #[serde(rename = "audioTrack")]
    track: Option<AudioTrack>,
    #[serde(rename = "isDrc", default)]
    drc: bool,
}

#[derive(Deserialize)]
struct AudioTrack {
    #[serde(rename = "audioIsDefault", default)]
    default: bool,
}

#[derive(Deserialize)]
struct Details {
    title: Option<String>,
    author: Option<String>,
    #[serde(rename = "lengthSeconds", default)]
    length: String,
    #[serde(rename = "isLive", default)]
    is_live: bool,
    #[serde(default)]
    thumbnail: Thumbnails,
}

#[derive(Deserialize, Default)]
struct Thumbnails {
    #[serde(default)]
    thumbnails: Vec<Thumb>,
}

#[derive(Deserialize)]
struct Thumb {
    url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids() {
        assert_eq!(video_id("https://youtu.be/dQw4w9WgXcQ?t=3").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(video_id("https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=x").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(video_id("https://www.youtube.com/playlist?list=PL123"), None);
        assert_eq!(video_id("https://soundcloud.com/a/b"), None);
    }

    #[test]
    fn picks_the_best_original_audio() {
        let json = r#"{
            "playabilityStatus": {"status": "OK"},
            "streamingData": {"adaptiveFormats": [
                {"mimeType": "video/mp4", "url": "v", "bitrate": 900000},
                {"mimeType": "audio/webm; codecs=\"opus\"", "url": "dub", "bitrate": 200000,
                 "audioTrack": {"audioIsDefault": false}},
                {"mimeType": "audio/webm; codecs=\"opus\"", "url": "drc", "bitrate": 190000, "isDrc": true},
                {"mimeType": "audio/webm; codecs=\"opus\"", "url": "best", "bitrate": 136544,
                 "approxDurationMs": "213061"},
                {"mimeType": "audio/mp4", "url": "aac", "bitrate": 130677}
            ]},
            "videoDetails": {"title": "Song", "author": "Band - Topic", "lengthSeconds": "213",
                "thumbnail": {"thumbnails": [{"url": "small"}, {"url": "big"}]}}
        }"#;
        let s = parse(json).unwrap();
        assert_eq!(s.url, "best");
        assert_eq!(s.artist.as_deref(), Some("Band"));
        assert_eq!(s.duration, Some(213.061));
        assert_eq!(s.thumbnail.as_deref(), Some("big"));
    }

    #[test]
    fn refusals() {
        let bot = r#"{"playabilityStatus": {"status": "LOGIN_REQUIRED",
            "reason": "Sign in to confirm you’re not a bot"}}"#;
        assert!(parse(bot).is_none());
        let gone = r#"{"playabilityStatus": {"status": "ERROR", "reason": "Video unavailable"}}"#;
        assert!(parse(gone).is_none());
        assert_eq!(visitor_of(r#"{"responseContext": {"visitorData": "Cgt"}}"#).as_deref(), Some("Cgt"));
        assert_eq!(visitor_of(bot), None);
        let live = r#"{"playabilityStatus": {"status": "OK"}, "videoDetails": {"isLive": true}}"#;
        assert!(parse(live).is_none());
    }

    #[test]
    fn reads_playlist_pages() {
        let page: serde_json::Value = serde_json::from_str(r#"{
            "contents": [
                {"playlistVideoRenderer": {"videoId": "aaaaaaaaaaa", "title": {"runs": [{"text": "Song"}]},
                    "shortBylineText": {"runs": [{"text": "Band"}]}, "lengthSeconds": "235",
                    "thumbnail": {"thumbnails": [{"url": "small"}, {"url": "big"}]}}},
                {"playlistVideoRenderer": {"videoId": "bbbbbbbbbbb", "title": {"simpleText": "[Deleted video]"}}},
                {"somethingElse": {}}
            ],
            "continuations": [{"nextContinuationData": {"continuation": "tok%3D"}}]
        }"#).unwrap();
        let mut items = Vec::new();
        assert_eq!(read_page(&page, &mut items), Some(Some("tok%3D".into())));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].url, "https://www.youtube.com/watch?v=aaaaaaaaaaa");
        assert_eq!((items[0].title.as_str(), items[0].uploader.as_deref()), ("Song", Some("Band")));
        assert_eq!((items[0].duration, items[0].thumbnail.as_deref()), (Some(235.0), Some("big")));
        assert_eq!(items[1].availability, Some(crate::linkcheck::LinkStatus::Dead));
        // The last page has no token; something that isn't a page at all is None.
        assert_eq!(read_page(&serde_json::json!({"contents": []}), &mut items), Some(None));
        assert_eq!(read_page(&serde_json::json!({}), &mut items), None);
    }

    /// Lists a public playlist of over 100 songs, so it takes several pages.
    #[test]
    #[ignore = "network"]
    fn live_playlist() {
        let url = "https://www.youtube.com/playlist?list=PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI";
        let info = playlist(&NetworkOptions::default(), url).expect("listed");
        assert!(info.is_playlist && info.items.len() > 100, "{} items", info.items.len());
        assert!(!info.title.is_empty());
    }

    /// Asks YouTube for a less-watched video that's refused without a visitor ID.
    #[test]
    #[ignore = "network"]
    fn live_look_up() {
        let s = resolve(&NetworkOptions::default(), "https://youtu.be/Zrq71nOI3C4").expect("a stream");
        assert!(s.url.contains("googlevideo.com"), "{}", s.url);
        assert!(VISITOR.lock().unwrap().is_some());
        println!("{} by {:?}", s.title, s.artist);
    }
}
