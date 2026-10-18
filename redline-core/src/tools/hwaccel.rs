//! Video codec and encoder selection: CPU encoders plus whichever GPU encoders actually work.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::command;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default, Hash, PartialOrd, Ord)]
pub enum VideoCodec {
    #[default]
    H264,
    H265,
    Av1,
    Vp9,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
        }
    }

    fn base(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "hevc",
            VideoCodec::Av1 => "av1",
            VideoCodec::Vp9 => "vp9",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default, Hash, PartialOrd, Ord)]
pub enum Hardware {
    #[default]
    Cpu,
    Nvidia,
    Intel,
    Amd,
}

impl Hardware {
    pub const ALL: [Hardware; 4] = [Hardware::Cpu, Hardware::Nvidia, Hardware::Intel, Hardware::Amd];

    pub fn label(self) -> &'static str {
        match self {
            Hardware::Cpu => "CPU",
            Hardware::Nvidia => "NVIDIA",
            Hardware::Intel => "Intel",
            Hardware::Amd => "AMD",
        }
    }
}

/// Codecs a container can hold, best default first. Empty for targets without a codec choice.
pub fn codecs_for(target: &str) -> &'static [VideoCodec] {
    match target {
        "mp4" | "mkv" | "mov" => &[VideoCodec::H264, VideoCodec::H265, VideoCodec::Av1],
        "webm" => &[VideoCodec::Vp9, VideoCodec::Av1],
        _ => &[],
    }
}

/// `codec` if the container takes it, else the container's default.
pub fn effective_codec(target: &str, codec: VideoCodec) -> VideoCodec {
    let allowed = codecs_for(target);
    if allowed.contains(&codec) {
        codec
    } else {
        allowed.first().copied().unwrap_or(VideoCodec::H264)
    }
}

/// ffmpeg encoder name, if that combination exists at all.
pub fn encoder_name(codec: VideoCodec, hw: Hardware) -> Option<String> {
    Some(match (hw, codec) {
        (Hardware::Cpu, VideoCodec::H264) => "libx264".into(),
        (Hardware::Cpu, VideoCodec::H265) => "libx265".into(),
        (Hardware::Cpu, VideoCodec::Av1) => "libsvtav1".into(),
        (Hardware::Cpu, VideoCodec::Vp9) => "libvpx-vp9".into(),
        (_, VideoCodec::Vp9) => return None,
        (Hardware::Nvidia, c) => format!("{}_nvenc", c.base()),
        (Hardware::Intel, c) => format!("{}_qsv", c.base()),
        (Hardware::Amd, c) => format!("{}_amf", c.base()),
    })
}

/// Encoders that ffmpeg has and that ran a test frame on this machine.
#[derive(Clone, Debug, Default)]
pub struct Encoders {
    working: BTreeSet<String>,
}

impl Encoders {
    pub fn supports(&self, codec: VideoCodec, hw: Hardware) -> bool {
        encoder_name(codec, hw).is_some_and(|e| self.working.contains(&e))
    }

    /// GPU vendors with at least one working encoder.
    pub fn gpus(&self) -> Vec<Hardware> {
        Hardware::ALL[1..]
            .iter()
            .copied()
            .filter(|hw| {
                [VideoCodec::H264, VideoCodec::H265, VideoCodec::Av1]
                    .iter()
                    .any(|c| self.supports(*c, *hw))
            })
            .collect()
    }

    /// `hw` if it can encode `codec`, else CPU.
    pub fn resolve(&self, codec: VideoCodec, hw: Hardware) -> Hardware {
        if hw == Hardware::Cpu || self.supports(codec, hw) {
            hw
        } else {
            Hardware::Cpu
        }
    }

    #[cfg(test)]
    pub fn with(names: &[&str]) -> Self {
        Self::from_names(names.iter().map(|s| s.to_string()))
    }

    /// Encoders found earlier by [`detect`] (their ffmpeg names), e.g. kept by another process.
    pub fn from_names(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            working: names.into_iter().collect(),
        }
    }

    /// The working encoders' ffmpeg names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.working.iter().map(String::as_str)
    }
}

/// Lists ffmpeg's encoders and test-encodes one frame with each GPU candidate, since GPU
/// encoders are listed whether or not the hardware or driver is present.
pub fn detect(ffmpeg: &Path) -> Encoders {
    let Ok(out) = command(ffmpeg).args(["-hide_banner", "-encoders"]).output() else {
        return Encoders::default();
    };
    let listed: BTreeSet<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_owned))
        .collect();
    let mut working = BTreeSet::new();
    for hw in Hardware::ALL {
        for codec in [VideoCodec::H264, VideoCodec::H265, VideoCodec::Av1, VideoCodec::Vp9] {
            let Some(name) = encoder_name(codec, hw) else { continue };
            if !listed.contains(&name) {
                continue;
            }
            if hw == Hardware::Cpu || test_encode(ffmpeg, &name) {
                working.insert(name);
            }
        }
    }
    Encoders { working }
}

fn test_encode(ffmpeg: &Path, encoder: &str) -> bool {
    command(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=s=256x256:d=0.1"])
        .args(["-frames:v", "1", "-c:v", encoder, "-f", "null", "-"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// How the encoder should spend bits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rate {
    /// x264-style CRF; each encoder gets an equivalent quality value.
    Crf(u8),
    /// Average video bitrate in kbit/s.
    Bitrate(u32),
}

/// `-c:v …` and its rate/speed options. `preset` uses x264's names.
pub fn video_encoder_args(codec: VideoCodec, hw: Hardware, rate: Rate, preset: &str) -> Vec<String> {
    let enc = encoder_name(codec, hw).unwrap_or_else(|| "libx264".into());
    let speed = ["ultrafast", "veryfast", "fast", "medium", "slow", "veryslow"]
        .iter()
        .position(|p| *p == preset)
        .unwrap_or(3);
    let mut a: Vec<String> = vec!["-c:v".into(), enc.clone()];
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    let q = |offset: u32, max: u32| match rate {
        Rate::Crf(c) => (c as u32 + offset).min(max).to_string(),
        Rate::Bitrate(_) => String::new(),
    };

    match (hw, codec) {
        (Hardware::Cpu, VideoCodec::H264) => {
            push(&["-preset", preset]);
            if let Rate::Crf(_) = rate {
                push(&["-crf", &q(0, 51)]);
            }
        }
        (Hardware::Cpu, VideoCodec::H265) => {
            push(&["-preset", preset, "-x265-params", "log-level=error"]);
            // x265 looks about as good as x264 at a CRF ~5 higher.
            if let Rate::Crf(_) = rate {
                push(&["-crf", &q(5, 51)]);
            }
        }
        (Hardware::Cpu, VideoCodec::Av1) => {
            let p = ["12", "11", "10", "8", "6", "4"][speed];
            push(&["-preset", p]);
            if let Rate::Crf(_) = rate {
                push(&["-crf", &q(10, 63)]);
            }
        }
        (Hardware::Cpu, VideoCodec::Vp9) => {
            let cpu_used = ["8", "6", "5", "4", "2", "1"][speed];
            push(&["-row-mt", "1", "-deadline", "good", "-cpu-used", cpu_used]);
            // VP9's CRF scale is wider than x264's; shift it to land at similar quality.
            if let Rate::Crf(_) = rate {
                push(&["-crf", &q(8, 63), "-b:v", "0"]);
            }
        }
        (Hardware::Nvidia, c) => {
            let p = ["p1", "p2", "p3", "p4", "p6", "p7"][speed];
            push(&["-preset", p]);
            if let Rate::Crf(_) = rate {
                let off = match c {
                    VideoCodec::H264 => 0,
                    VideoCodec::H265 => 3,
                    _ => 6,
                };
                push(&["-rc", "vbr", "-cq", &q(off, 51), "-b:v", "0"]);
            }
        }
        (Hardware::Intel, c) => {
            let p = ["veryfast", "veryfast", "fast", "medium", "slow", "veryslow"][speed];
            push(&["-preset", p]);
            if let Rate::Crf(_) = rate {
                let off = if c == VideoCodec::H264 { 0 } else { 3 };
                push(&["-global_quality", &q(off, 51)]);
            }
        }
        (Hardware::Amd, c) => {
            let p = ["speed", "speed", "balanced", "balanced", "quality", "quality"][speed];
            push(&["-quality", p]);
            if let Rate::Crf(_) = rate {
                let off = if c == VideoCodec::H264 { 0 } else { 3 };
                let qp = q(off, 51);
                push(&["-rc", "cqp", "-qp_i", &qp, "-qp_p", &qp]);
                if c != VideoCodec::Av1 {
                    push(&["-qp_b", &qp]);
                }
            }
        }
    }
    if let Rate::Bitrate(k) = rate {
        let k = k.max(1);
        push(&[
            "-b:v",
            &format!("{k}k"),
            "-maxrate",
            &format!("{}k", k * 3 / 2),
            "-bufsize",
            &format!("{}k", k * 2),
        ]);
    }
    // QSV wants NV12; everything else takes the widely playable yuv420p.
    push(&["-pix_fmt", if hw == Hardware::Intel { "nv12" } else { "yuv420p" }]);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(codec: VideoCodec, hw: Hardware, rate: Rate, preset: &str) -> String {
        video_encoder_args(codec, hw, rate, preset).join(" ")
    }

    #[test]
    fn maps_codecs_and_hardware() {
        assert_eq!(
            s(VideoCodec::H264, Hardware::Cpu, Rate::Crf(23), "medium"),
            "-c:v libx264 -preset medium -crf 23 -pix_fmt yuv420p"
        );
        assert!(s(VideoCodec::H265, Hardware::Cpu, Rate::Crf(23), "slow").contains("libx265 -preset slow -x265-params log-level=error -crf 28"));
        assert!(s(VideoCodec::Av1, Hardware::Cpu, Rate::Crf(23), "fast").contains("libsvtav1 -preset 10 -crf 33"));
        assert!(s(VideoCodec::Vp9, Hardware::Cpu, Rate::Crf(60), "medium").contains("-cpu-used 4 -crf 63 -b:v 0"));
        assert!(s(VideoCodec::H265, Hardware::Nvidia, Rate::Crf(23), "veryslow").contains("hevc_nvenc -preset p7 -rc vbr -cq 26 -b:v 0"));
        assert!(s(VideoCodec::H264, Hardware::Intel, Rate::Crf(20), "ultrafast").contains("h264_qsv -preset veryfast -global_quality 20 -pix_fmt nv12"));
        let amd = s(VideoCodec::Av1, Hardware::Amd, Rate::Crf(20), "medium");
        assert!(amd.contains("av1_amf -quality balanced -rc cqp -qp_i 23 -qp_p 23") && !amd.contains("qp_b"));
        let br = s(VideoCodec::H264, Hardware::Nvidia, Rate::Bitrate(1000), "medium");
        assert!(br.contains("-b:v 1000k -maxrate 1500k -bufsize 2000k") && !br.contains("-cq"));
        assert_eq!(encoder_name(VideoCodec::Vp9, Hardware::Nvidia), None);
    }

    #[test]
    fn codec_and_hardware_fallbacks() {
        assert_eq!(effective_codec("webm", VideoCodec::H264), VideoCodec::Vp9);
        assert_eq!(effective_codec("webm", VideoCodec::Av1), VideoCodec::Av1);
        assert_eq!(effective_codec("mkv", VideoCodec::H265), VideoCodec::H265);
        assert_eq!(effective_codec("mp4", VideoCodec::Vp9), VideoCodec::H264);

        let e = Encoders::with(&["libx264", "h264_nvenc", "hevc_qsv"]);
        assert_eq!(e.gpus(), vec![Hardware::Nvidia, Hardware::Intel]);
        assert_eq!(e.resolve(VideoCodec::H264, Hardware::Nvidia), Hardware::Nvidia);
        assert_eq!(e.resolve(VideoCodec::Av1, Hardware::Nvidia), Hardware::Cpu);
        assert_eq!(e.resolve(VideoCodec::H265, Hardware::Intel), Hardware::Intel);
    }
}
