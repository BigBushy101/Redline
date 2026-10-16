//! Locating and running the external tools (yt-dlp, ffmpeg, ffprobe, deno).

pub mod convert;
#[cfg(test)]
mod e2e;
pub mod ffmpeg;
pub mod filters;
pub mod hwaccel;
pub mod image_conv;
pub mod install;
pub mod ytdlp;

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Result, anyhow};

use crate::jobs::{CancelHandle, Cancelled};
use crate::settings::{self, Settings};

pub fn bin_dir() -> PathBuf {
    settings::data_dir().join("bin")
}

pub fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

#[derive(Clone, Default)]
pub struct ToolPaths {
    pub ytdlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub deno: Option<PathBuf>,
}

impl ToolPaths {
    pub fn locate(s: &Settings) -> Self {
        let ffmpeg = find("ffmpeg", s.ffmpeg_path.as_deref());
        // Prefer the ffprobe that ships next to whichever ffmpeg we picked.
        let ffprobe = s
            .ffprobe_path
            .clone()
            .filter(|p| p.is_file())
            .or_else(|| {
                ffmpeg
                    .as_ref()
                    .and_then(|f| f.parent())
                    .map(|d| d.join(exe_name("ffprobe")))
                    .filter(|p| p.is_file())
            })
            .or_else(|| find("ffprobe", None));
        Self {
            ytdlp: find("yt-dlp", s.ytdlp_path.as_deref()),
            ffmpeg,
            ffprobe,
            deno: find("deno", s.deno_path.as_deref()),
        }
    }

    pub fn ready_for_download(&self) -> bool {
        self.ytdlp.is_some() && self.ffmpeg.is_some()
    }

    pub fn ready_for_convert(&self) -> bool {
        self.ffmpeg.is_some() && self.ffprobe.is_some()
    }

    pub fn missing_any(&self) -> bool {
        self.ytdlp.is_none() || self.ffmpeg.is_none() || self.ffprobe.is_none() || self.deno.is_none()
    }
}

fn find(name: &str, override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_path.filter(|p| p.is_file()) {
        return Some(p.to_path_buf());
    }
    let exe = exe_name(name);
    let local = bin_dir().join(&exe);
    if local.is_file() {
        return Some(local);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(&exe))
            .find(|p| p.is_file())
    })
}

/// A `Command` that won't flash a console window on Windows.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Kills a process and its children. yt-dlp.exe is a PyInstaller bundle that re-spawns
/// itself and launches ffmpeg, so killing only the top process leaves orphans.
pub fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = command(Path::new("taskkill"))
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Runs a tool to completion, feeding each stdout line to `on_line`.
/// Fails with [`Cancelled`] if the user cancelled, or with the tool's last error output.
pub fn run_streaming(
    mut cmd: Command,
    cancel: &CancelHandle,
    mut on_line: impl FnMut(&str),
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(Cancelled.into());
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("failed to start {:?}: {e}", cmd.get_program()))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    cancel.attach(child);

    let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
    let tail_writer = tail.clone();
    let stderr_thread = thread::spawn(move || {
        for line in lines(stderr) {
            let mut t = tail_writer.lock().unwrap();
            if t.len() == 40 {
                t.pop_front();
            }
            t.push_back(line);
        }
    });

    for line in lines(stdout) {
        on_line(&line);
    }
    let _ = stderr_thread.join();
    let status = cancel.wait()?;

    if cancel.is_cancelled() {
        return Err(Cancelled.into());
    }
    if !status.success() {
        let tail = tail.lock().unwrap();
        let msg = tail
            .iter()
            .rev()
            .find(|l| l.starts_with("ERROR:"))
            .map(|l| l.trim_start_matches("ERROR:").trim().to_owned())
            .or_else(|| {
                let last: Vec<_> = tail.iter().rev().take(3).cloned().collect();
                (!last.is_empty()).then(|| last.into_iter().rev().collect::<Vec<_>>().join("\n"))
            })
            .unwrap_or_else(|| format!("exited with {status}"));
        return Err(anyhow!(msg));
    }
    Ok(())
}

/// Lossy UTF-8 line iterator that tolerates `\r\n` and invalid bytes.
fn lines(r: impl std::io::Read) -> impl Iterator<Item = String> {
    let mut reader = BufReader::new(r);
    std::iter::from_fn(move || {
        let mut buf = Vec::new();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(
                String::from_utf8_lossy(&buf)
                    .trim_end_matches(['\r', '\n'])
                    .to_owned(),
            ),
        }
    })
}

/// Returns `dir/stem.ext`, or `dir/stem (n).ext` if that already exists.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

/// Opens Explorer with the file selected (or just the folder elsewhere).
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    if path.is_file() {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
        return;
    }
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let _ = open::that_detached(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_path_appends_counter() {
        let dir = std::env::temp_dir().join(format!("redline-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a", "png"), dir.join("a.png"));
        std::fs::write(dir.join("a.png"), b"").unwrap();
        assert_eq!(unique_path(&dir, "a", "png"), dir.join("a (1).png"));
        std::fs::write(dir.join("a (1).png"), b"").unwrap();
        assert_eq!(unique_path(&dir, "a", "png"), dir.join("a (2).png"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
