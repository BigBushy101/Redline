//! First-run download of yt-dlp, ffmpeg/ffprobe and deno into the app's bin folder.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use eframe::egui;

use super::{ToolPaths, bin_dir, exe_name};

#[cfg(windows)]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
#[cfg(target_os = "macos")]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos";
#[cfg(all(unix, not(target_os = "macos")))]
const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";

#[cfg(windows)]
const FFMPEG_ZIP_URL: Option<&str> = Some(
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
);
#[cfg(not(windows))]
const FFMPEG_ZIP_URL: Option<&str> = None;

#[cfg(windows)]
const DENO_ZIP_URL: Option<&str> =
    Some("https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip");
#[cfg(target_os = "macos")]
const DENO_ZIP_URL: Option<&str> =
    Some("https://github.com/denoland/deno/releases/latest/download/deno-aarch64-apple-darwin.zip");
#[cfg(all(unix, not(target_os = "macos")))]
const DENO_ZIP_URL: Option<&str> =
    Some("https://github.com/denoland/deno/releases/latest/download/deno-x86_64-unknown-linux-gnu.zip");

#[derive(Clone, Default)]
pub struct InstallStatus {
    pub running: bool,
    pub step: String,
    pub progress: Option<f32>,
    pub error: Option<String>,
    /// Set when a run ends; the app clears it after re-locating the tools.
    pub just_finished: bool,
}

#[derive(Default)]
pub struct Installer {
    status: Arc<Mutex<InstallStatus>>,
}

impl Installer {
    pub fn status(&self) -> InstallStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn take_finished(&self) -> bool {
        std::mem::take(&mut self.status.lock().unwrap().just_finished)
    }

    pub fn start(&self, tools: &ToolPaths, ctx: &egui::Context) {
        {
            let mut s = self.status.lock().unwrap();
            if s.running {
                return;
            }
            *s = InstallStatus {
                running: true,
                step: "Preparing…".into(),
                ..Default::default()
            };
        }
        let want_ytdlp = tools.ytdlp.is_none();
        let want_ffmpeg = tools.ffmpeg.is_none() || tools.ffprobe.is_none();
        let want_deno = tools.deno.is_none();
        let status = self.status.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let p = Progress {
                status: status.clone(),
                ctx: ctx.clone(),
            };
            let result = install(&p, want_ytdlp, want_ffmpeg, want_deno);
            let mut s = status.lock().unwrap();
            s.running = false;
            s.just_finished = true;
            s.progress = None;
            match result {
                Ok(()) => s.step = "All tools installed".into(),
                Err(e) => s.error = Some(format!("{e:#}")),
            }
            ctx.request_repaint();
        });
    }
}

struct Progress {
    status: Arc<Mutex<InstallStatus>>,
    ctx: egui::Context,
}

impl Progress {
    fn set(&self, step: String, progress: Option<f32>) {
        let mut s = self.status.lock().unwrap();
        s.step = step;
        s.progress = progress;
        drop(s);
        self.ctx.request_repaint();
    }
}

fn install(p: &Progress, ytdlp: bool, ffmpeg: bool, deno: bool) -> Result<()> {
    let bin = bin_dir();
    fs::create_dir_all(&bin).with_context(|| format!("can't create {}", bin.display()))?;

    if ytdlp {
        let dest = bin.join(exe_name("yt-dlp"));
        download(p, "yt-dlp", YTDLP_URL, &dest)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
        }
    }
    if ffmpeg {
        let Some(url) = FFMPEG_ZIP_URL else {
            bail!("Install ffmpeg with your package manager (e.g. `brew install ffmpeg`).");
        };
        let zip = bin.join("ffmpeg-download.zip");
        download(p, "ffmpeg", url, &zip)?;
        p.set("Unpacking ffmpeg…".into(), None);
        extract(&zip, &bin, &[&exe_name("ffmpeg"), &exe_name("ffprobe")])?;
        let _ = fs::remove_file(&zip);
    }
    if deno && let Some(url) = DENO_ZIP_URL {
        let zip = bin.join("deno-download.zip");
        download(p, "Deno", url, &zip)?;
        p.set("Unpacking Deno…".into(), None);
        extract(&zip, &bin, &[&exe_name("deno")])?;
        let _ = fs::remove_file(&zip);
    }
    Ok(())
}

fn download(p: &Progress, label: &str, url: &str, dest: &Path) -> Result<()> {
    p.set(format!("Connecting to download {label}…"), None);
    let resp = ureq::get(url)
        .call()
        .with_context(|| format!("couldn't download {label}"))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = resp.into_body().into_reader();

    let part = dest.with_extension("part");
    let mut file = File::create(&part).with_context(|| format!("can't write {}", part.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    let mut last_update = Instant::now() - Duration::from_secs(1);
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("download of {label} was interrupted"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if last_update.elapsed() > Duration::from_millis(100) {
            last_update = Instant::now();
            let mb = |b: u64| b as f64 / 1_048_576.0;
            let (frac, text) = match total {
                Some(t) if t > 0 => (
                    Some(done as f32 / t as f32),
                    format!("Downloading {label} · {:.1} / {:.1} MB", mb(done), mb(t)),
                ),
                _ => (None, format!("Downloading {label} · {:.1} MB", mb(done))),
            };
            p.set(text, frac);
        }
    }
    file.flush()?;
    drop(file);
    if let Some(t) = total
        && done != t
    {
        let _ = fs::remove_file(&part);
        bail!("download of {label} was incomplete ({done} of {t} bytes)");
    }
    fs::rename(&part, dest).with_context(|| format!("can't move {label} into place"))?;
    Ok(())
}

/// Copies the named files (matched by file name anywhere in the archive) into `dest_dir`.
fn extract(zip_path: &Path, dest_dir: &Path, names: &[&str]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?).context("corrupt archive")?;
    let mut found = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(file_name) = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
        else {
            continue;
        };
        if entry.is_file() && names.contains(&file_name.as_str()) {
            let dest = dest_dir.join(&file_name);
            let mut out = File::create(&dest)
                .with_context(|| format!("can't write {} (is it running?)", dest.display()))?;
            std::io::copy(&mut entry, &mut out)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
            }
            found += 1;
        }
    }
    if found < names.len() {
        bail!("archive didn't contain {}", names.join(", "));
    }
    Ok(())
}

/// Runs `yt-dlp -U` and returns its last line of output.
pub fn update_ytdlp(tools: &ToolPaths) -> Result<String> {
    let Some(ytdlp) = &tools.ytdlp else {
        bail!("yt-dlp is not installed");
    };
    let out = super::command(ytdlp).arg("-U").output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let last = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("Done")
        .trim()
        .to_owned();
    if out.status.success() {
        Ok(last)
    } else {
        bail!("{last}")
    }
}

/// `yt-dlp --version`, for the settings page.
pub fn ytdlp_version(tools: &ToolPaths) -> Option<String> {
    let out = super::command(tools.ytdlp.as_ref()?).arg("--version").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|s| !s.is_empty())
}
