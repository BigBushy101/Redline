//! Waveform peaks for the current track, made with ffmpeg and cached on disk.
//!
//! A track is analyzed once: the peaks go to waveforms/<hash>.json under Redline's data
//! folder. A downloaded/local copy is read directly; a link is resolved to its audio stream
//! through yt-dlp first, then only the peaks are kept (the stream itself is not saved).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::history::History;
use crate::library::{self, Track};
use crate::player::Source;
use crate::settings;
use crate::tools::ytdlp::{self, NetworkOptions};
use crate::tools::ToolPaths;

/// Bars to cache. Drawing maps them onto whatever width the UI has.
const BINS: usize = 1000;
/// Mono 8 kHz is plenty to see the shape and keeps ffmpeg's output small.
const SAMPLE_RATE: u32 = 8000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Waveform {
    pub peaks: Vec<f32>,
    pub duration: Option<f64>,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    key: String,
    peaks: Vec<f32>,
    duration: Option<f64>,
}

/// The source a track will play from, and the key that tells one analysis of it from another.
pub fn source_and_key(track: &Track, history: &History) -> (Source, String) {
    let source = library::playable_source(track, history);
    let key = match &source {
        Source::Local(p) => {
            let meta = std::fs::metadata(p).ok();
            format!(
                "file:{}|{}|{}",
                p.display(),
                meta.as_ref().map_or(0, |m| m.len()),
                meta.and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs()),
            )
        }
        Source::Link(url) => format!("link:{url}|{}", track.duration.unwrap_or(0.0)),
    };
    (source, key)
}

pub fn for_track(
    tools: &ToolPaths,
    net: &NetworkOptions,
    track: &Track,
    history: &History,
) -> Result<Waveform> {
    let (source, key) = source_and_key(track, history);
    for_source(tools, net, &source, &key, track.duration)
}

/// Peaks for the source, from the cache or by decoding it now. A link is resolved first;
/// only the peaks are written, not the audio.
pub fn for_source(
    tools: &ToolPaths,
    net: &NetworkOptions,
    source: &Source,
    key: &str,
    duration: Option<f64>,
) -> Result<Waveform> {
    let cache = cache_path(key);
    if let Some(cached) = read_cache(&cache, key) {
        return Ok(cached);
    }
    let ffmpeg = tools.ffmpeg.as_ref().context("Waveforms need ffmpeg.")?;
    let (input, headers) = match source {
        Source::Local(p) => {
            if !p.is_file() {
                bail!("The file for this track is gone.");
            }
            (p.to_string_lossy().into_owned(), None)
        }
        Source::Link(url) => {
            let info = ytdlp::resolve_stream(tools, net, url)?;
            (info.url, info.headers)
        }
    };

    // The same environment as Redline's other ffmpeg runs (Android needs its libraries).
    let mut cmd = crate::tools::command(ffmpeg);
    cmd.args(["-v", "error", "-nostdin"]);
    if let Some(headers) = headers.as_deref() {
        cmd.args(["-headers", headers]);
    }
    cmd.args(["-i", &input, "-vn", "-ac", "1", "-ar", "8000", "-f", "s16le", "pipe:1"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("couldn't start ffmpeg")?;
    let mut pcm = Vec::new();
    child
        .stdout
        .take()
        .context("ffmpeg has no output")?
        .read_to_end(&mut pcm)
        .context("couldn't read ffmpeg's output")?;
    let out = child.wait_with_output().context("ffmpeg didn't finish")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("Couldn't read the waveform: {}", err.lines().next().unwrap_or("ffmpeg failed"));
    }
    let peaks = peaks_from_pcm(&pcm, BINS);
    if peaks.is_empty() {
        bail!("That track has no audio to show.");
    }
    let decoded = if pcm.len() >= 2 {
        Some(pcm.len() as f64 / 2.0 / SAMPLE_RATE as f64)
    } else {
        duration
    };
    let wave = Waveform { peaks, duration: decoded };
    write_cache(&cache, key, &wave);
    Ok(wave)
}

/// Absolute peak per bucket, normalized to the loudest bucket.
pub fn peaks_from_pcm(pcm: &[u8], bins: usize) -> Vec<f32> {
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    if samples.is_empty() || bins == 0 {
        return Vec::new();
    }
    let bins = bins.min(samples.len());
    let mut peaks = vec![0.0f32; bins];
    for (i, sample) in samples.iter().enumerate() {
        let bin = i * bins / samples.len();
        peaks[bin] = peaks[bin].max((*sample as f32).abs() / i16::MAX as f32);
    }
    let max = peaks.iter().copied().fold(0.0f32, f32::max);
    if max > 0.0 {
        for p in &mut peaks {
            *p = (*p / max).clamp(0.0, 1.0);
        }
    }
    peaks
}

fn hash(text: &str) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn cache_path(key: &str) -> PathBuf {
    settings::data_dir().join("waveforms").join(format!("{}.json", hash(key)))
}

fn read_cache(path: &Path, key: &str) -> Option<Waveform> {
    let text = std::fs::read_to_string(path).ok()?;
    let cached: Cached = serde_json::from_str(&text).ok()?;
    if cached.key != key || cached.peaks.is_empty() {
        return None;
    }
    Some(Waveform { peaks: cached.peaks, duration: cached.duration })
}

fn write_cache(path: &Path, key: &str, wave: &Waveform) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cached = Cached { key: key.to_owned(), peaks: wave.peaks.clone(), duration: wave.duration };
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_are_normalized_buckets() {
        let pcm: Vec<u8> = [0i16, 100, -3000, 32767, 0, -10000]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let peaks = peaks_from_pcm(&pcm, 3);
        assert_eq!(peaks.len(), 3);
        assert!((peaks[1] - 1.0).abs() < 0.001);
        assert!((peaks[2] - 10000.0 / 32767.0).abs() < 0.01);
    }

    #[test]
    fn empty_audio_has_no_peaks() {
        assert!(peaks_from_pcm(&[], 10).is_empty());
    }
}
