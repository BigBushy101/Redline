//! Turning a looked-up link and the Download page's choices into a download job.

use crate::format::fmt_time;
use crate::jobs::JobSpec;
use crate::settings::{self, Settings};
use crate::tools::ytdlp::{DownloadRequest, MediaInfo, Mode, Tags, Trim};

/// What's known and picked for the current link. Cleared when the link changes.
#[derive(Default)]
pub struct LinkChoices {
    pub info: Option<MediaInfo>,
    /// Exact formats picked from the lookup's format list.
    pub pick_video: Option<String>,
    pub pick_audio: Option<String>,
    /// Audio tags as edited, and as they came from the site.
    pub tags: Tags,
    pub tags_prefill: Tags,
}

/// "Start later" choice next to the Download button; kept between links.
pub struct Schedule {
    pub later: bool,
    pub hour: u32,
    pub minute: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            later: false,
            hour: 2,
            minute: 0,
        }
    }
}

impl Schedule {
    /// Next time the clock shows hour:minute, as Unix seconds.
    pub fn next(&self) -> Option<i64> {
        use chrono::{Duration, Local};
        let now = Local::now();
        let today = now.date_naive().and_hms_opt(self.hour, self.minute, 0)?;
        let mut at = today.and_local_timezone(Local).earliest()?;
        if at <= now {
            at += Duration::days(1);
        }
        Some(at.timestamp())
    }

    /// When a download queued now should start, if later.
    pub fn not_before(&self) -> Option<i64> {
        if self.later { self.next() } else { None }
    }
}

/// Tags as the site names them: the track's own title and artist where it has them.
pub fn tags_from(info: &MediaInfo) -> Tags {
    Tags {
        title: info.track.clone().unwrap_or_else(|| info.title.clone()),
        artist: info
            .artist
            .clone()
            .or_else(|| info.uploader.clone())
            .unwrap_or_default(),
        album: info.album.clone().unwrap_or_default(),
    }
}

/// A download job ready to queue.
pub struct PlannedDownload {
    pub title: String,
    pub subtitle: String,
    pub spec: JobSpec,
}

impl LinkChoices {
    /// Takes in a finished lookup, prefilling the tags from it.
    pub fn set_info(&mut self, info: MediaInfo) {
        self.tags_prefill = tags_from(&info);
        self.tags = self.tags_prefill.clone();
        self.info = Some(info);
    }

    pub fn is_playlist(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.is_playlist)
    }

    /// The `-f` selector for the picked formats, if any apply to `mode`.
    pub fn picked_format(&self, mode: Mode) -> Option<String> {
        let formats = &self.info.as_ref()?.formats;
        match mode {
            Mode::Audio => self.pick_audio.clone(),
            Mode::Video => {
                let v = self.pick_video.as_ref()?;
                let has_audio = formats.iter().any(|f| &f.id == v && f.has_audio());
                Some(match (&self.pick_audio, has_audio) {
                    (Some(a), _) => format!("{v}+{a}"),
                    (None, true) => v.clone(),
                    (None, false) => format!("{v}+ba"),
                })
            }
        }
    }

    /// The job for `url` with the current settings and picks.
    pub fn plan(&self, url: &str, trim: Option<Trim>, settings: &Settings) -> PlannedDownload {
        let is_playlist = self.is_playlist();
        let title = self.info.as_ref().map_or_else(|| url.to_owned(), |i| i.title.clone());
        let options = settings.download.clone();
        let mut options_for_job = options.clone();
        if !is_playlist {
            options_for_job.playlist = false;
        }
        let format = self.picked_format(options.mode);
        let mut subtitle = options.summary();
        if let Some(f) = &format {
            subtitle = format!("{} · format {f}", subtitle.split(" · ").take(2).collect::<Vec<_>>().join(" · "));
        }
        if let Some(t) = &trim {
            let end = t.end.map_or("end".to_owned(), fmt_time);
            subtitle.push_str(&format!(" · Clip {}–{end}", fmt_time(t.start)));
        }
        // Only send tags the user changed; otherwise yt-dlp's own metadata is better.
        let tags = (options.mode == Mode::Audio && !is_playlist && self.tags != self.tags_prefill)
            .then(|| self.tags.clone());
        PlannedDownload {
            title,
            subtitle,
            spec: JobSpec::Download(DownloadRequest {
                url: url.to_owned(),
                options: options_for_job,
                out_dir: settings.download_dir.clone(),
                // Clips of something already downloaded are still wanted.
                archive: (settings.skip_downloaded && trim.is_none()).then(settings::archive_path),
                network: settings.network.clone(),
                trim,
                format,
                tags,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ytdlp::FormatInfo;

    #[test]
    fn plans_download_with_picks_trim_and_tags() {
        let mut link = LinkChoices::default();
        link.set_info(MediaInfo {
            title: "Video".into(),
            uploader: Some("Chan".into()),
            formats: vec![FormatInfo {
                id: "137".into(),
                vcodec: Some("avc1".into()),
                ..Default::default()
            }],
            ..Default::default()
        });
        assert_eq!(link.tags.artist, "Chan");
        link.pick_video = Some("137".into());
        assert_eq!(link.picked_format(Mode::Video).as_deref(), Some("137+ba"));
        link.pick_audio = Some("140".into());
        assert_eq!(link.picked_format(Mode::Video).as_deref(), Some("137+140"));
        assert_eq!(link.picked_format(Mode::Audio).as_deref(), Some("140"));

        let mut settings = Settings::default();
        settings.download.playlist = true;
        let trim = Trim { start: 5.0, end: Some(65.5), precise: false };
        let plan = link.plan("https://x/v", Some(trim), &settings);
        assert_eq!(plan.title, "Video");
        assert!(plan.subtitle.contains("format 137+140"), "{}", plan.subtitle);
        assert!(plan.subtitle.ends_with(" · Clip 0:05–1:05.5"), "{}", plan.subtitle);
        let JobSpec::Download(r) = plan.spec else { panic!() };
        assert!(!r.options.playlist, "single videos never download a playlist");
        assert_eq!(r.archive, None, "clips skip the archive");
        assert_eq!(r.tags, None, "tags unchanged");

        settings.download.mode = Mode::Audio;
        link.tags.title = "Fixed".into();
        let JobSpec::Download(r) = link.plan("https://x/v", None, &settings).spec else { panic!() };
        assert_eq!(r.tags.map(|t| t.title).as_deref(), Some("Fixed"));
        assert!(r.archive.is_some());
    }
}
