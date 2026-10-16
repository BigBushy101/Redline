//! Decoding with ffmpeg into raw samples, and the reader that feeds them to the ring buffer.

use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use rtrb::Producer;

use crate::jobs::CancelHandle;
use crate::tools::{self, ToolPaths};

/// Where to read a track from, once its source is resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    /// A file path or a stream URL.
    pub location: String,
    /// HTTP headers ffmpeg must send, as `Name: value\r\n` lines.
    pub headers: Option<String>,
}

/// A running decoder's output.
pub struct Pcm {
    /// Interleaved stereo `f32le` samples.
    pub reader: Box<dyn Read + Send>,
    /// Stops the decoder early (seek, stop, another load).
    pub kill: Box<dyn Fn() + Send + Sync>,
    /// Waits for the decoder once its output ends. An error says why it stopped early.
    pub finish: Box<dyn FnOnce() -> Result<()> + Send>,
}

/// The ffmpeg arguments that decode `input` from `start` seconds to raw stereo at `rate`.
pub fn ffmpeg_args(input: &Input, start: f64, rate: u32) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error"]
        .map(String::from)
        .into();
    if start > 0.0 {
        a.extend(["-ss".into(), format!("{start:.3}")]);
    }
    if let Some(h) = &input.headers {
        a.extend(["-headers".into(), h.clone()]);
    }
    if input.location.starts_with("http") {
        // Ride out short network drops instead of ending the track early.
        a.extend(["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "5"].map(String::from));
    }
    a.extend(["-i".into(), input.location.clone()]);
    a.extend(["-vn", "-sn", "-dn", "-f", "f32le", "-ac", "2"].map(String::from));
    a.extend(["-ar".into(), rate.to_string(), "pipe:1".into()]);
    a
}

/// Starts ffmpeg decoding `input` from `start` seconds.
pub fn ffmpeg(tools: &ToolPaths, input: &Input, start: f64, rate: u32) -> Result<Pcm> {
    let ffmpeg = tools
        .ffmpeg
        .as_ref()
        .ok_or_else(|| anyhow!("ffmpeg is not installed"))?;
    let mut cmd = tools::command(ffmpeg);
    cmd.args(ffmpeg_args(input, start, rate))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| anyhow!("failed to start ffmpeg: {e}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let process = Arc::new(CancelHandle::default());
    process.attach(child);

    let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
    let tail_writer = tail.clone();
    let stderr_thread = thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::BufReader::new(stderr).read_to_string(&mut text);
        let mut t = tail_writer.lock().unwrap();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if t.len() == 8 {
                t.pop_front();
            }
            t.push_back(line.trim().to_owned());
        }
    });

    let killer = process.clone();
    Ok(Pcm {
        reader: Box::new(stdout),
        kill: Box::new(move || killer.cancel()),
        finish: Box::new(move || {
            let status = process.wait()?;
            let _ = stderr_thread.join();
            if process.is_cancelled() {
                return Err(crate::jobs::Cancelled.into());
            }
            if !status.success() {
                let tail = tail.lock().unwrap();
                let msg = tail.iter().cloned().collect::<Vec<_>>().join("\n");
                return Err(anyhow!(if msg.is_empty() { format!("ffmpeg exited with {status}") } else { msg }));
            }
            Ok(())
        }),
    })
}

/// Copies samples from `reader` into the ring until the reader ends, `stop` is set, or the
/// player drops the stream. Waits while the ring is full, so decoding stays only a little
/// ahead of playback. Sets `eof` when the reader is exhausted.
pub fn pump(mut reader: impl Read, mut ring: Producer<f32>, stop: &AtomicBool, eof: &AtomicBool) -> std::io::Result<()> {
    let mut buf = vec![0u8; 16 * 1024];
    // Bytes of a sample split across two reads.
    let mut carry = 0usize;
    loop {
        if stop.load(Ordering::Relaxed) || ring.is_abandoned() {
            return Ok(());
        }
        let n = match reader.read(&mut buf[carry..]) {
            Ok(0) => {
                eof.store(true, Ordering::Release);
                return Ok(());
            }
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eof.store(true, Ordering::Release);
                return Err(e);
            }
        };
        let filled = carry + n;
        let whole = filled / 4 * 4;
        let mut samples = buf[..whole]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let mut left = whole / 4;
        while left > 0 {
            if stop.load(Ordering::Relaxed) || ring.is_abandoned() {
                return Ok(());
            }
            let room = ring.slots().min(left);
            if room == 0 {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            for _ in 0..room {
                // `room` was free, so the push can't fail.
                let _ = ring.push(samples.next().unwrap_or(0.0));
            }
            left -= room;
        }
        buf.copy_within(whole..filled, 0);
        carry = filled - whole;
    }
}

/// Duration and tags of a local file, from ffprobe.
pub fn probe(ffprobe: &Path, file: &Path) -> Option<Probe> {
    let out = tools::command(ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(file)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_probe(&out.stdout)
}

#[derive(Debug, Default, PartialEq)]
pub struct Probe {
    pub duration: Option<f64>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub has_audio: bool,
}

fn parse_probe(json: &[u8]) -> Option<Probe> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let format = &v["format"];
    // Tag names vary in case between containers (TITLE in FLAC, title in MP3).
    let tag = |name: &str| {
        format["tags"].as_object().and_then(|tags| {
            tags.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .and_then(|(_, v)| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
    };
    let has_audio = v["streams"]
        .as_array()
        .is_some_and(|s| s.iter().any(|s| s["codec_type"] == "audio"));
    Some(Probe {
        duration: format["duration"].as_str().and_then(|d| d.parse().ok()),
        title: tag("title"),
        artist: tag("artist").or_else(|| tag("album_artist")),
        album: tag("album"),
        has_audio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;

    #[test]
    fn decode_args_seek_before_input() {
        let input = Input {
            location: "C:/m/a.flac".into(),
            headers: None,
        };
        let a = ffmpeg_args(&input, 12.5, 48000);
        let ss = a.iter().position(|x| x == "-ss").unwrap();
        let i = a.iter().position(|x| x == "-i").unwrap();
        assert!(ss < i, "input seeking is fast");
        assert_eq!(a[ss + 1], "12.500");
        assert_eq!(a[i + 1], "C:/m/a.flac");
        assert!(a.ends_with(&["-ar".into(), "48000".into(), "pipe:1".into()]));
        assert!(ffmpeg_args(&input, 0.0, 44100).iter().all(|x| x != "-ss"), "no seek from the start");

        let link = Input {
            location: "https://cdn/x".into(),
            headers: Some("User-Agent: a\r\n".into()),
        };
        let a = ffmpeg_args(&link, 0.0, 44100);
        let h = a.iter().position(|x| x == "-headers").unwrap();
        assert!(h < a.iter().position(|x| x == "-i").unwrap());
        assert!(a.contains(&"-reconnect".into()));
    }

    /// Hands out bytes in awkward chunk sizes, so samples straddle reads.
    struct Choppy(Vec<u8>, usize);

    impl Read for Choppy {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = (self.0.len() - self.1).min(out.len()).min(7);
            out[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
            self.1 += n;
            Ok(n)
        }
    }

    #[test]
    fn pump_hands_every_sample_over_in_order() {
        let samples: Vec<f32> = (0..5000).map(|i| i as f32).collect();
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        // Smaller than the input, so the pump has to wait for the reader side.
        let (p, mut c) = RingBuffer::new(256);
        let stop = Arc::new(AtomicBool::new(false));
        let eof = Arc::new(AtomicBool::new(false));
        let (stop2, eof2) = (stop.clone(), eof.clone());
        let t = thread::spawn(move || pump(Choppy(bytes, 0), p, &stop2, &eof2));
        let mut got = Vec::new();
        while got.len() < samples.len() {
            match c.pop() {
                Ok(s) => got.push(s),
                Err(_) => thread::yield_now(),
            }
        }
        t.join().unwrap().unwrap();
        assert_eq!(got, samples);
        assert!(eof.load(Ordering::Acquire));
    }

    #[test]
    fn pump_stops_when_the_stream_is_dropped() {
        let (p, c) = RingBuffer::new(16);
        drop(c);
        let eof = AtomicBool::new(false);
        pump(std::io::repeat(0), p, &AtomicBool::new(false), &eof).unwrap();
        assert!(!eof.load(Ordering::Acquire), "abandoned, not finished");
    }

    #[test]
    fn parses_probe_json() {
        let json = br#"{"streams":[{"codec_type":"video"},{"codec_type":"audio"}],
            "format":{"duration":"183.250000","tags":{"TITLE":"Song","album_artist":"Band","album":" "}}}"#;
        let p = parse_probe(json).unwrap();
        assert_eq!(p.duration, Some(183.25));
        assert_eq!(p.title.as_deref(), Some("Song"));
        assert_eq!(p.artist.as_deref(), Some("Band"));
        assert_eq!(p.album, None);
        assert!(p.has_audio);
    }
}
