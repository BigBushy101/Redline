//! Rotate, flip, crop, speed, loudness and subtitle burn-in, as ffmpeg filter chains
//! (and the image-crate equivalents for still images).

use std::path::{Path, PathBuf};

use image::DynamicImage;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Rotation {
    #[default]
    None,
    /// 90° clockwise.
    Right,
    /// 90° counter-clockwise.
    Left,
    Half,
}

/// Center crop to an aspect ratio.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CropAspect {
    #[default]
    None,
    Wide,
    Tall,
    Square,
    Portrait,
}

impl CropAspect {
    /// (width, height) ratio.
    fn ratio(self) -> Option<(u32, u32)> {
        match self {
            CropAspect::None => None,
            CropAspect::Wide => Some((16, 9)),
            CropAspect::Tall => Some((9, 16)),
            CropAspect::Square => Some((1, 1)),
            CropAspect::Portrait => Some((4, 5)),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Filters {
    pub rotate: Rotation,
    pub flip: bool,
    pub crop: CropAspect,
    /// Playback speed for video and audio (GIFs have their own).
    pub speed: f32,
    /// EBU R128 loudness normalization to -16 LUFS.
    pub normalize: bool,
    pub gain_db: f32,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            rotate: Rotation::None,
            flip: false,
            crop: CropAspect::None,
            speed: 1.0,
            normalize: false,
            gain_db: 0.0,
        }
    }
}

pub const SPEEDS: &[(f32, &str)] = &[
    (0.5, "0.5×"),
    (0.75, "0.75×"),
    (1.0, "1×"),
    (1.25, "1.25×"),
    (1.5, "1.5×"),
    (2.0, "2×"),
];

impl Filters {
    pub fn changes_speed(&self) -> bool {
        (self.speed - 1.0).abs() > 0.001
    }

    /// Video filters that don't touch timing; safe before a GIF's own chain.
    pub fn geometry(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some((w, h)) = self.crop.ratio() {
            v.push(format!(
                "crop=w='trunc(min(iw,ih*{w}/{h})/2)*2':h='trunc(min(ih,iw*{h}/{w})/2)*2'"
            ));
        }
        match self.rotate {
            Rotation::None => {}
            Rotation::Right => v.push("transpose=1".into()),
            Rotation::Left => v.push("transpose=2".into()),
            Rotation::Half => v.push("hflip,vflip".into()),
        }
        if self.flip {
            v.push("hflip".into());
        }
        v
    }

    /// Full video chain: geometry, burned subtitles, then speed. Subtitles go before the
    /// speed change so they still line up with the original timestamps.
    pub fn video_chain(&self, subtitles: Option<&str>) -> Vec<String> {
        let mut v = self.geometry();
        if let Some(s) = subtitles {
            v.push(s.to_owned());
        }
        if self.changes_speed() {
            v.push(format!("setpts=PTS/{}", self.speed));
        }
        v
    }

    pub fn audio_chain(&self) -> Vec<String> {
        let mut a = Vec::new();
        if self.changes_speed() {
            // atempo only takes 0.5–100 per instance, so chain it for slower speeds.
            let mut s = self.speed.max(0.01) as f64;
            while s < 0.5 {
                a.push("atempo=0.5".into());
                s /= 0.5;
            }
            a.push(format!("atempo={}", trim_float(s)));
        }
        if self.normalize {
            a.push("loudnorm=I=-16:TP=-1.5:LRA=11".into());
        }
        if self.gain_db.abs() > 0.01 {
            a.push(format!("volume={}dB", trim_float(self.gain_db as f64)));
        }
        a
    }

    /// Rotate, flip and crop a still image the same way.
    pub fn apply_to_image(&self, mut img: DynamicImage) -> DynamicImage {
        if let Some((w, h)) = self.crop.ratio() {
            let (iw, ih) = (img.width(), img.height());
            let cw = iw.min(ih * w / h).max(1);
            let ch = ih.min(iw * h / w).max(1);
            img = img.crop_imm((iw - cw) / 2, (ih - ch) / 2, cw, ch);
        }
        img = match self.rotate {
            Rotation::None => img,
            Rotation::Right => img.rotate90(),
            Rotation::Left => img.rotate270(),
            Rotation::Half => img.rotate180(),
        };
        if self.flip { img.fliph() } else { img }
    }
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Where burned-in subtitles come from.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum SubSource {
    /// The input's first subtitle track.
    Embedded,
    File(PathBuf),
}

impl SubSource {
    /// `subtitles=…` filter for `input`.
    pub fn filter(&self, input: &Path) -> String {
        match self {
            SubSource::Embedded => format!("subtitles={}:si=0", escape_filter_path(input)),
            SubSource::File(f) => format!("subtitles={}", escape_filter_path(f)),
        }
    }
}

/// Escapes a path for use as a filter option inside a `-vf` graph: once for the option
/// parser (`\ ' :`) and again for the graph parser (`\ ' [ ] , ;`).
pub fn escape_filter_path(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let mut opt = String::new();
    for c in p.chars() {
        if matches!(c, '\\' | '\'' | ':') {
            opt.push('\\');
        }
        opt.push(c);
    }
    let mut graph = String::new();
    for c in opt.chars() {
        if matches!(c, '\\' | '\'' | '[' | ']' | ',' | ';') {
            graph.push('\\');
        }
        graph.push(c);
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_chains() {
        assert!(Filters::default().video_chain(None).is_empty());
        assert!(Filters::default().audio_chain().is_empty());
        let f = Filters {
            rotate: Rotation::Right,
            flip: true,
            crop: CropAspect::Square,
            speed: 2.0,
            normalize: true,
            gain_db: -3.0,
        };
        assert_eq!(
            f.video_chain(Some("subtitles=x")),
            vec![
                "crop=w='trunc(min(iw,ih*1/1)/2)*2':h='trunc(min(ih,iw*1/1)/2)*2'".to_string(),
                "transpose=1".into(),
                "hflip".into(),
                "subtitles=x".into(),
                "setpts=PTS/2".into(),
            ]
        );
        assert_eq!(
            f.audio_chain(),
            vec!["atempo=2", "loudnorm=I=-16:TP=-1.5:LRA=11", "volume=-3dB"]
        );
        let slow = Filters { speed: 0.25, ..Default::default() };
        assert_eq!(slow.audio_chain(), vec!["atempo=0.5", "atempo=0.5"]);
    }

    #[test]
    fn escapes_windows_paths() {
        assert_eq!(
            escape_filter_path(Path::new(r"C:\My Videos\it's [a],b.srt")),
            r"C\\:/My Videos/it\\\'s \[a\]\,b.srt"
        );
        assert_eq!(
            SubSource::Embedded.filter(Path::new("/v/a.mkv")),
            "subtitles=/v/a.mkv:si=0"
        );
    }

    #[test]
    fn crops_and_rotates_images() {
        let img = DynamicImage::new_rgba8(160, 90);
        let f = Filters {
            crop: CropAspect::Square,
            rotate: Rotation::Right,
            ..Default::default()
        };
        let out = f.apply_to_image(img);
        assert_eq!((out.width(), out.height()), (90, 90));
        let tall = Filters { crop: CropAspect::Tall, ..Default::default() }
            .apply_to_image(DynamicImage::new_rgba8(160, 90));
        assert_eq!((tall.width(), tall.height()), (50, 90));
        let turned = Filters { rotate: Rotation::Left, ..Default::default() }
            .apply_to_image(DynamicImage::new_rgba8(160, 90));
        assert_eq!((turned.width(), turned.height()), (90, 160));
    }
}
