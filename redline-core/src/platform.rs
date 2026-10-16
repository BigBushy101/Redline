//! Which site a link is from, and one spelling per link, so the same song found through
//! different URL shapes (youtu.be, `&t=`, mobile hosts) is recognized as the same.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Platform {
    YouTube,
    SoundCloud,
    Bandcamp,
    Mixcloud,
    Vimeo,
    Other,
}

impl Platform {
    /// In the order the sidebar lists them.
    pub const ALL: [Platform; 6] = [
        Platform::YouTube,
        Platform::SoundCloud,
        Platform::Bandcamp,
        Platform::Mixcloud,
        Platform::Vimeo,
        Platform::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Platform::YouTube => "YouTube",
            Platform::SoundCloud => "SoundCloud",
            Platform::Bandcamp => "Bandcamp",
            Platform::Mixcloud => "Mixcloud",
            Platform::Vimeo => "Vimeo",
            Platform::Other => "Other",
        }
    }

    /// The site a link is from, by its host. Derived every time rather than stored, so
    /// tracks saved before this existed sort themselves.
    pub fn of(url: &str) -> Platform {
        let Some(host) = host(url) else { return Platform::Other };
        let is = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
        if is("youtube.com") || is("youtu.be") || is("youtube-nocookie.com") {
            Platform::YouTube
        } else if is("soundcloud.com") || is("snd.sc") {
            Platform::SoundCloud
        } else if is("bandcamp.com") {
            Platform::Bandcamp
        } else if is("mixcloud.com") {
            Platform::Mixcloud
        } else if is("vimeo.com") {
            Platform::Vimeo
        } else {
            Platform::Other
        }
    }

    /// Fallback from yt-dlp's `extractor_key` (e.g. `YoutubeTab`, `SoundcloudSet`), for links
    /// whose host isn't one we know (custom Bandcamp domains, for one).
    pub fn from_extractor(extractor: &str) -> Platform {
        let e = extractor.to_lowercase();
        if e.starts_with("youtube") {
            Platform::YouTube
        } else if e.starts_with("soundcloud") {
            Platform::SoundCloud
        } else if e.starts_with("bandcamp") {
            Platform::Bandcamp
        } else if e.starts_with("mixcloud") {
            Platform::Mixcloud
        } else if e.starts_with("vimeo") {
            Platform::Vimeo
        } else {
            Platform::Other
        }
    }
}

/// Splits `scheme://host/rest` into the lowercased host (no port or login) and the rest.
fn split(url: &str) -> Option<(String, &str)> {
    let url = url.trim();
    let (has_scheme, after_scheme) = url.split_once("://").map_or((false, url), |(_, r)| (true, r));
    let end = after_scheme.find(['/', '?', '#']).unwrap_or(after_scheme.len());
    let (authority, rest) = after_scheme.split_at(end);
    let host = authority.rsplit('@').next()?.split(':').next()?.to_lowercase();
    let plausible = !host.is_empty() && !host.contains(char::is_whitespace) && (has_scheme || host.contains('.'));
    plausible.then_some((host, rest))
}

fn host(url: &str) -> Option<String> {
    split(url).map(|(h, _)| h)
}

/// The value of `key` in a query string (without the `?`).
fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .filter(|v| !v.is_empty())
}

/// One spelling for a link, used to tell whether two links are the same song:
/// - YouTube videos become `https://www.youtube.com/watch?v=ID` from any of its shapes
///   (youtu.be, shorts, embeds, music.youtube.com, a `watch` link inside a playlist);
///   playlist pages keep their `list`.
/// - Other links get a lowercase host without `www.`/`m.`, no query or fragment and no
///   trailing `/`.
///
/// Text that isn't a link comes back trimmed.
pub fn canonical(url: &str) -> String {
    let url = url.trim();
    let Some((host, rest)) = split(url) else { return url.to_owned() };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    if Platform::of(url) == Platform::YouTube
        && let Some(c) = youtube_canonical(&host, path, query)
    {
        return c;
    }
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(&host);
    format!("https://{host}{}", path.trim_end_matches('/'))
}

fn youtube_canonical(host: &str, path: &str, query: &str) -> Option<String> {
    let video = |id: &str| {
        let id = id.split('/').next().unwrap_or(id);
        (!id.is_empty()).then(|| format!("https://www.youtube.com/watch?v={id}"))
    };
    if host == "youtu.be" || host.ends_with(".youtu.be") {
        return video(path.trim_start_matches('/'));
    }
    if let Some(id) = query_param(query, "v").filter(|_| path.starts_with("/watch")) {
        return video(id);
    }
    for prefix in ["/shorts/", "/embed/", "/live/", "/v/"] {
        if let Some(id) = path.strip_prefix(prefix) {
            return video(id);
        }
    }
    if path.trim_end_matches('/') == "/playlist" {
        return query_param(query, "list").map(|l| format!("https://www.youtube.com/playlist?list={l}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_url_shapes() {
        use Platform::*;
        let yt = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let cases = [
            ("https://www.youtube.com/watch?v=dQw4w9WgXcQ", YouTube, yt),
            ("https://youtu.be/dQw4w9WgXcQ", YouTube, yt),
            ("https://youtu.be/dQw4w9WgXcQ?si=abc123&t=42", YouTube, yt),
            ("http://youtube.com/watch?v=dQw4w9WgXcQ&t=42s", YouTube, yt),
            ("https://m.youtube.com/watch?v=dQw4w9WgXcQ&feature=share", YouTube, yt),
            ("https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=xyz", YouTube, yt),
            ("https://www.youtube.com/watch?list=PL123&v=dQw4w9WgXcQ&index=3", YouTube, yt),
            ("https://www.youtube.com/shorts/dQw4w9WgXcQ?feature=share", YouTube, yt),
            ("https://www.youtube.com/embed/dQw4w9WgXcQ", YouTube, yt),
            ("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ", YouTube, yt),
            ("  https://WWW.YouTube.com/watch?v=dQw4w9WgXcQ#t=1  ", YouTube, yt),
            (
                "https://www.youtube.com/playlist?list=PLx0sYbCqOb8TBPRdmBHs5Iftvv9TPboYG&si=q",
                YouTube,
                "https://www.youtube.com/playlist?list=PLx0sYbCqOb8TBPRdmBHs5Iftvv9TPboYG",
            ),
            (
                "https://music.youtube.com/playlist?list=OLAK5uy_abc",
                YouTube,
                "https://www.youtube.com/playlist?list=OLAK5uy_abc",
            ),
            ("https://www.youtube.com/@channel/videos", YouTube, "https://youtube.com/@channel/videos"),
            (
                "https://soundcloud.com/artist/track-name?si=abc&utm_source=clipboard",
                SoundCloud,
                "https://soundcloud.com/artist/track-name",
            ),
            ("https://m.soundcloud.com/artist/track-name", SoundCloud, "https://soundcloud.com/artist/track-name"),
            ("https://soundcloud.com/artist/sets/my-set/", SoundCloud, "https://soundcloud.com/artist/sets/my-set"),
            ("https://on.soundcloud.com/AbCdE", SoundCloud, "https://on.soundcloud.com/AbCdE"),
            (
                "https://Artist.bandcamp.com/album/some-album?from=hp",
                Bandcamp,
                "https://artist.bandcamp.com/album/some-album",
            ),
            ("https://artist.bandcamp.com/track/a-song", Bandcamp, "https://artist.bandcamp.com/track/a-song"),
            ("https://www.mixcloud.com/dj/a-mix/", Mixcloud, "https://mixcloud.com/dj/a-mix"),
            ("https://vimeo.com/123456789?share=copy", Vimeo, "https://vimeo.com/123456789"),
            ("https://player.vimeo.com/video/123456789", Vimeo, "https://player.vimeo.com/video/123456789"),
            ("https://example.org/Path/To/File.mp3?x=1", Other, "https://example.org/Path/To/File.mp3"),
        ];
        for (url, platform, canon) in cases {
            assert_eq!(Platform::of(url), platform, "{url}");
            assert_eq!(canonical(url), canon, "{url}");
        }
        // Lookalikes and non-links.
        assert_eq!(Platform::of("https://notyoutube.com/watch?v=x"), Other);
        assert_eq!(Platform::of("not a link"), Other);
        assert_eq!(canonical(" not a link "), "not a link");
    }

    #[test]
    fn extractor_fallback() {
        assert_eq!(Platform::from_extractor("YoutubeTab"), Platform::YouTube);
        assert_eq!(Platform::from_extractor("SoundcloudSet"), Platform::SoundCloud);
        assert_eq!(Platform::from_extractor("BandcampAlbum"), Platform::Bandcamp);
        assert_eq!(Platform::from_extractor("Generic"), Platform::Other);
        assert_eq!(Platform::ALL.len(), 6);
        assert_eq!(Platform::Mixcloud.label(), "Mixcloud");
    }
}
