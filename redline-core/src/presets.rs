//! Named download and convert setups, with a few built-ins seeded on first run.

use serde::{Deserialize, Serialize};

use crate::convert_plan::ConvertItem;
use crate::tools::convert::{Category, ConvertOptions};
use crate::tools::filters::Filters;
use crate::tools::hwaccel::VideoCodec;
use crate::tools::ytdlp::{DownloadOptions, Mode, SubsMode};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct DownloadPreset {
    pub name: String,
    pub options: DownloadOptions,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ConvertPreset {
    pub name: String,
    pub options: ConvertOptions,
    /// Output format per kind of file; `None` leaves those files' targets alone.
    #[serde(default)]
    pub image_target: Option<String>,
    #[serde(default)]
    pub video_target: Option<String>,
    #[serde(default)]
    pub audio_target: Option<String>,
}

impl ConvertPreset {
    pub fn target_for(&self, category: Category) -> Option<&str> {
        match category {
            Category::Image => self.image_target.as_deref(),
            Category::Video => self.video_target.as_deref(),
            Category::Audio => self.audio_target.as_deref(),
        }
        .filter(|t| category.targets().contains(t))
    }

    /// Whether `o` is this preset's settings. The encoder is the machine's business, not the
    /// preset's, so it's ignored.
    pub fn matches(&self, o: &ConvertOptions) -> bool {
        let mut o = o.clone();
        o.hardware = self.options.hardware;
        o == self.options
    }

    /// The preset's options, keeping the current encoder choice.
    pub fn options_keeping_hardware(&self, current: &ConvertOptions) -> ConvertOptions {
        ConvertOptions {
            hardware: current.hardware,
            ..self.options.clone()
        }
    }

    /// Applies the preset to the shared options and to the formats of the files that follow
    /// them (files with their own settings keep theirs).
    pub fn apply<'a>(&self, shared: &mut ConvertOptions, items: impl IntoIterator<Item = &'a mut ConvertItem>) {
        *shared = self.options_keeping_hardware(shared);
        for item in items.into_iter().filter(|i| i.options.is_none()) {
            if let Some(t) = self.target_for(item.category) {
                item.target = t.to_owned();
            }
        }
    }

    /// A preset of `options`, remembering the formats in use when every file of a kind agrees.
    pub fn from_items<'a>(
        name: String,
        options: ConvertOptions,
        items: impl IntoIterator<Item = &'a ConvertItem> + Clone,
    ) -> Self {
        let target = |c: Category| {
            let mut ts = items.clone().into_iter().filter(|i| i.category == c).map(|i| i.target.clone());
            let first = ts.next()?;
            ts.all(|t| t == first).then_some(first)
        };
        Self {
            name,
            image_target: target(Category::Image),
            video_target: target(Category::Video),
            audio_target: target(Category::Audio),
            options,
        }
    }
}

pub fn default_download_presets() -> Vec<DownloadPreset> {
    let d = DownloadOptions::default();
    vec![
        DownloadPreset {
            name: "Podcast MP3 128k".into(),
            options: DownloadOptions {
                mode: Mode::Audio,
                audio_format: "mp3".into(),
                audio_bitrate: "128K".into(),
                ..d.clone()
            },
        },
        DownloadPreset {
            name: "Music FLAC".into(),
            options: DownloadOptions {
                mode: Mode::Audio,
                audio_format: "flac".into(),
                square_cover: true,
                ..d.clone()
            },
        },
        DownloadPreset {
            name: "720p MP4".into(),
            options: DownloadOptions {
                max_height: 720,
                container: "mp4".into(),
                ..d.clone()
            },
        },
        DownloadPreset {
            name: "Archive MKV best".into(),
            options: DownloadOptions {
                max_height: 0,
                container: "mkv".into(),
                embed_subs: true,
                subs_mode: SubsMode::Embed,
                ..d
            },
        },
    ]
}

pub fn default_convert_presets() -> Vec<ConvertPreset> {
    let d = ConvertOptions::default();
    let preset = |name: &str, options: ConvertOptions, image: Option<&str>, video: Option<&str>, audio: Option<&str>| {
        ConvertPreset {
            name: name.into(),
            options,
            image_target: image.map(str::to_owned),
            video_target: video.map(str::to_owned),
            audio_target: audio.map(str::to_owned),
        }
    };
    vec![
        preset(
            "Discord clip (10 MB)",
            ConvertOptions {
                target_size_mb: 10.0,
                max_height: 720,
                codec: VideoCodec::H264,
                audio_bitrate: 128,
                ..d.clone()
            },
            None,
            Some("mp4"),
            None,
        ),
        preset(
            "Web MP4 720p",
            ConvertOptions {
                crf: 24,
                max_height: 720,
                codec: VideoCodec::H264,
                preset: "slow".into(),
                ..d.clone()
            },
            None,
            Some("mp4"),
            None,
        ),
        preset(
            "Small WebP",
            ConvertOptions {
                image_quality: 80,
                resize_width: 1920,
                resize_height: 1920,
                ..d.clone()
            },
            Some("webp"),
            None,
            None,
        ),
        preset(
            "Even-volume MP3",
            ConvertOptions {
                audio_bitrate: 192,
                filters: Filters {
                    normalize: true,
                    ..Default::default()
                },
                ..d
            },
            None,
            Some("mp3"),
            Some("mp3"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::hwaccel::Hardware;

    #[test]
    fn built_ins_are_valid_and_match_ignoring_encoder() {
        let settings = crate::settings::Settings::default();
        assert_eq!(settings.download_presets.len(), 4);
        assert_eq!(settings.convert_presets.len(), 4);
        for p in &settings.convert_presets {
            for c in [Category::Image, Category::Video, Category::Audio] {
                if let Some(t) = &p.target_for(c) {
                    assert!(c.targets().contains(t), "{} {t}", p.name);
                }
            }
        }
        let discord = &settings.convert_presets[0];
        let mut current = ConvertOptions {
            hardware: Hardware::Nvidia,
            ..Default::default()
        };
        assert!(!discord.matches(&current));
        current = discord.options_keeping_hardware(&current);
        assert_eq!(current.hardware, Hardware::Nvidia);
        assert!(discord.matches(&current));

        let item = |path: &str, category, target: &str| ConvertItem {
            path: path.into(),
            category,
            size: 0,
            target: target.into(),
            options: None,
            burn_subs: None,
        };
        let mut items = vec![
            item("a.mkv", Category::Video, "webm"),
            item("b.mkv", Category::Video, "mkv"),
            item("c.flac", Category::Audio, "ogg"),
        ];
        let saved = ConvertPreset::from_items("Mine".into(), ConvertOptions::default(), &items);
        assert_eq!(saved.video_target, None, "the videos disagree");
        assert_eq!(saved.audio_target.as_deref(), Some("ogg"));
        assert_eq!(saved.image_target, None, "no images");

        items[1].options = Some(ConvertOptions::default());
        let mut shared = ConvertOptions { hardware: Hardware::Intel, ..Default::default() };
        discord.apply(&mut shared, &mut items);
        assert!(discord.matches(&shared) && shared.hardware == Hardware::Intel);
        assert_eq!(items[0].target, "mp4");
        assert_eq!(items[1].target, "mkv", "has its own settings");
        assert_eq!(items[2].target, "ogg", "the preset has no audio format");

        // Saved settings without preset fields still load the built-ins.
        let loaded: crate::settings::Settings = serde_json::from_str(r#"{"concurrency":3}"#).unwrap();
        assert_eq!(loaded.concurrency, 3);
        assert_eq!(loaded.download_presets.len(), 4);
    }
}
