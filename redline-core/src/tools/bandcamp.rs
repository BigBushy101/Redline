//! A Bandcamp track or album, read from its page instead of through yt-dlp: the page carries
//! its tracks' details and 128 kbps MP3 links in a `data-tralbum` attribute, so one page fetch
//! (under a second) stands in for a yt-dlp run (two seconds on a PC, far more on a phone).
//! Anything this doesn't expect gives `None`, and the caller asks yt-dlp as before.
//!
//! Only `*.bandcamp.com` links: custom domains look like any other site until fetched.

use super::ytdlp::{MediaInfo, NetworkOptions, PlaylistItem, StreamInfo};
use crate::platform::Platform;

/// `url`'s stream, if it's a Bandcamp track page with one (pre-orders and some tracks have none).
pub fn resolve(net: &NetworkOptions, url: &str) -> Option<StreamInfo> {
    if !url.contains("/track/") {
        return None;
    }
    stream(&tralbum(net, url)?)
}

/// A Bandcamp album page's tracks, for saving.
pub fn album(net: &NetworkOptions, url: &str) -> Option<MediaInfo> {
    if !url.contains("/album/") {
        return None;
    }
    listing(&tralbum(net, url)?, url)
}

/// The page's `data-tralbum` JSON.
fn tralbum(net: &NetworkOptions, url: &str) -> Option<serde_json::Value> {
    if Platform::of(url) != Platform::Bandcamp {
        return None;
    }
    let page = super::youtube::agent(net)?.get(url).call().ok()?.body_mut().read_to_string().ok()?;
    let attr = page.split_once("data-tralbum=\"")?.1.split_once('"')?.0;
    serde_json::from_str(&unescape(attr)).ok()
}

fn stream(t: &serde_json::Value) -> Option<StreamInfo> {
    let track = &t["trackinfo"][0];
    Some(StreamInfo {
        url: track["file"]["mp3-128"].as_str()?.to_owned(),
        headers: None,
        title: track["title"].as_str()?.to_owned(),
        artist: track["artist"].as_str().or(t["artist"].as_str()).map(str::to_owned),
        album: None,
        duration: track["duration"].as_f64().filter(|d| *d > 0.0),
        thumbnail: art(t),
    })
}

fn listing(t: &serde_json::Value, url: &str) -> Option<MediaInfo> {
    // Track links are relative to the artist's site.
    let origin = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
    let artist = t["artist"].as_str().map(str::to_owned);
    let items: Vec<PlaylistItem> = t["trackinfo"]
        .as_array()?
        .iter()
        .filter_map(|tr| {
            Some(PlaylistItem {
                url: format!("{origin}{}", tr["title_link"].as_str()?),
                title: tr["title"].as_str()?.to_owned(),
                uploader: tr["artist"].as_str().map(str::to_owned).or(artist.clone()),
                duration: tr["duration"].as_f64().filter(|d| *d > 0.0),
                thumbnail: art(t),
                availability: None,
            })
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    Some(MediaInfo {
        title: t["current"]["title"].as_str()?.to_owned(),
        uploader: artist,
        thumbnail: art(t),
        is_playlist: true,
        entries: items.len(),
        items,
        extractor: Some("BandcampAlbum".into()),
        ..Default::default()
    })
}

/// The cover, at 1200 px.
fn art(t: &serde_json::Value) -> Option<String> {
    t["art_id"].as_u64().map(|id| format!("https://f4.bcbits.com/img/a{id}_10.jpg"))
}

/// Undoes HTML attribute escaping: named `&quot; &amp; &#39; &lt; &gt;` and numeric references.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&end| end <= 10).and_then(|end| {
            let c = match &rest[1..end] {
                "quot" => '"',
                "amp" => '&',
                "apos" => '\'',
                "lt" => '<',
                "gt" => '>',
                n => {
                    let n = n.strip_prefix('#')?;
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => n.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes() {
        assert_eq!(unescape("&quot;a&quot;:&#39;b&#x27; &amp;amp; & x&lt;"), "\"a\":'b' &amp; & x<");
    }

    #[test]
    fn reads_track_and_album() {
        let t: serde_json::Value = serde_json::from_str(r#"{
            "artist": "Band", "art_id": 12345, "current": {"title": "Record"},
            "trackinfo": [
                {"title": "One", "duration": 61.5, "title_link": "/track/one", "file": {"mp3-128": "https://t4.bcbits.com/1"}},
                {"title": "Two", "duration": 0, "title_link": "/track/two", "artist": "Guest", "file": null}
            ]
        }"#).unwrap();
        let s = stream(&t).unwrap();
        assert_eq!((s.url.as_str(), s.title.as_str(), s.artist.as_deref()), ("https://t4.bcbits.com/1", "One", Some("Band")));
        assert_eq!(s.thumbnail.as_deref(), Some("https://f4.bcbits.com/img/a12345_10.jpg"));
        let a = listing(&t, "https://band.bandcamp.com/album/record?from=hp").unwrap();
        assert_eq!((a.title.as_str(), a.items.len()), ("Record", 2));
        assert_eq!(a.items[1].url, "https://band.bandcamp.com/track/two");
        assert_eq!((a.items[1].uploader.as_deref(), a.items[1].duration), (Some("Guest"), None));
        // No stream (a pre-order): yt-dlp's turn.
        assert!(stream(&serde_json::json!({"trackinfo": [{"title": "x", "file": null}]})).is_none());
    }

    #[test]
    #[ignore = "network"]
    fn live_track() {
        let t = std::time::Instant::now();
        let s = resolve(&NetworkOptions::default(), "https://youtube-dl.bandcamp.com/track/youtube-dl-test-song").expect("a stream");
        println!("{} by {:?} in {:?}: {}", s.title, s.artist, t.elapsed(), s.url);
        assert!(s.url.contains("bcbits.com"));
    }
}
