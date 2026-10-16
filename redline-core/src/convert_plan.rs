//! Turning the Convert page's file list into convert and join jobs.

use std::path::PathBuf;

use crate::format::ARROW_RIGHT;
use crate::jobs::JobSpec;
use crate::settings::Settings;
use crate::tools::convert::{Category, ConcatRequest, ConvertOptions, ConvertRequest, classify, render_stem};
use crate::tools::ffmpeg::is_audio_target;
use crate::tools::filters::SubSource;
use crate::tools::hwaccel::{self, Encoders};

/// A file waiting to be converted.
#[derive(Clone, Debug)]
pub struct ConvertItem {
    pub path: PathBuf,
    pub category: Category,
    pub size: u64,
    pub target: String,
    /// Settings for this file only; `None` follows the shared options.
    pub options: Option<ConvertOptions>,
    pub burn_subs: Option<SubSource>,
}

impl ConvertItem {
    /// `None` if the path isn't a file of a supported type.
    pub fn from_path(path: PathBuf) -> Option<Self> {
        let category = classify(&path)?;
        if !path.is_file() {
            return None;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Some(Self {
            path,
            category,
            size,
            target: category.default_target().to_owned(),
            options: None,
            burn_subs: None,
        })
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "output".into())
    }

    pub fn is_customized(&self) -> bool {
        self.options.is_some() || self.burn_subs.is_some()
    }

    /// Video in, video out (not GIF): the only case where subtitles can be burned in.
    pub fn makes_video(&self) -> bool {
        self.category == Category::Video
            && !is_audio_target(&self.target)
            && !hwaccel::codecs_for(&self.target).is_empty()
    }
}

/// A job ready to queue.
pub struct PlannedJob {
    pub title: String,
    pub subtitle: String,
    pub spec: JobSpec,
}

/// The target all files would be joined into, if joining makes sense for this list.
pub fn join_target<'a>(items: impl IntoIterator<Item = &'a ConvertItem>) -> Option<&'a str> {
    let items: Vec<_> = items.into_iter().collect();
    let first = items.first()?;
    let same_kind = items.len() >= 2
        && first.category != Category::Image
        && items.iter().all(|i| i.category == first.category);
    let target = first.target.as_str();
    (same_kind && (is_audio_target(target) || !hwaccel::codecs_for(target).is_empty())).then_some(target)
}

/// Options as sent with a job: the GPU falls back to CPU if it can't encode this codec.
fn job_options(o: &ConvertOptions, target: &str, enc: &Encoders) -> ConvertOptions {
    let mut o = o.clone();
    o.hardware = enc.resolve(hwaccel::effective_codec(target, o.codec), o.hardware);
    o
}

/// One convert job per file, in list order.
pub fn requests(items: Vec<ConvertItem>, settings: &Settings, encoders: &Encoders) -> Vec<PlannedJob> {
    let template = &settings.rename_template;
    items
        .into_iter()
        .enumerate()
        .map(|(idx, item)| {
            let from = item
                .path
                .extension()
                .map(|e| e.to_string_lossy().to_uppercase())
                .unwrap_or_default();
            let options = item.options.as_ref().unwrap_or(&settings.convert);
            let stem = render_stem(template, &item.stem(), idx + 1, &item.target);
            let burn = item.burn_subs.clone().filter(|_| item.makes_video());
            let mut subtitle = format!("{from} {ARROW_RIGHT} {}", item.target.to_uppercase());
            if burn.is_some() {
                subtitle.push_str(" · subtitles burned in");
            }
            PlannedJob {
                title: item.name(),
                subtitle,
                spec: JobSpec::Convert(ConvertRequest {
                    options: job_options(options, &item.target, encoders),
                    category: item.category,
                    out_dir: settings.convert_dir.clone(),
                    output_stem: (stem != item.stem()).then_some(stem),
                    burn_subs: burn,
                    recycle_source: settings.recycle_originals,
                    target: item.target,
                    input: item.path,
                }),
            }
        })
        .collect()
}

/// One job joining every file into `target`, using the first file's settings.
pub fn join_request(items: Vec<ConvertItem>, target: &str, settings: &Settings, encoders: &Encoders) -> PlannedJob {
    let first = &items[0];
    let options = first.options.as_ref().unwrap_or(&settings.convert);
    let stem = render_stem(&settings.rename_template, &format!("{} (joined)", first.stem()), 1, target);
    PlannedJob {
        title: format!("{} + {} more", first.name(), items.len() - 1),
        subtitle: format!("Join {} files {ARROW_RIGHT} {}", items.len(), target.to_uppercase()),
        spec: JobSpec::Concat(ConcatRequest {
            options: job_options(options, target, encoders),
            target: target.to_owned(),
            out_dir: settings.convert_dir.clone(),
            output_stem: stem,
            recycle_sources: settings.recycle_originals,
            inputs: items.into_iter().map(|i| i.path).collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, category: Category, target: &str) -> ConvertItem {
        ConvertItem {
            path: path.into(),
            category,
            size: 0,
            target: target.into(),
            options: None,
            burn_subs: None,
        }
    }

    #[test]
    fn plans_conversions_and_joins() {
        let settings = Settings::default();
        let enc = Encoders::default();
        let mut a = item("C:/v/a.mkv", Category::Video, "mp4");
        a.burn_subs = Some(SubSource::Embedded);
        let b = item("C:/v/b.mkv", Category::Video, "mp3");
        assert_eq!(join_target([&a, &b]), Some("mp4"));
        assert_eq!(join_target([&a]), None);

        let jobs = requests(vec![a.clone(), b.clone()], &settings, &enc);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].title, "a.mkv");
        assert_eq!(jobs[0].subtitle, format!("MKV {ARROW_RIGHT} MP4 · subtitles burned in"));
        let JobSpec::Convert(r) = &jobs[1].spec else { panic!() };
        assert_eq!(r.burn_subs, None, "audio output can't burn subtitles");
        assert_eq!(r.output_stem, None, "default template keeps the name");

        let join = join_request(vec![a, b], "mp4", &settings, &enc);
        assert_eq!(join.title, "a.mkv + 1 more");
        let JobSpec::Concat(r) = &join.spec else { panic!() };
        assert_eq!(r.inputs.len(), 2);
        assert_eq!(r.output_stem, "a (joined)");
    }
}
