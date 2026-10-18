//! Locating and running the external tools (yt-dlp, ffmpeg, ffprobe, a JS runtime).

pub mod convert;
#[cfg(test)]
pub(crate) mod e2e;
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
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use anyhow::{Result, anyhow, ensure};

use crate::jobs::{CancelHandle, Cancelled};
use crate::settings::{self, Settings};

pub fn bin_dir() -> PathBuf {
    settings::data_dir().join("bin")
}

pub fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// A JavaScript runtime yt-dlp can use for YouTube's checks, passed as `--js-runtimes name:path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsRuntime {
    /// yt-dlp's name for it: `deno` on desktop, `quickjs` on Android.
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolPaths {
    /// The yt-dlp executable, or its script when [`Self::ytdlp_launcher`] is set.
    pub ytdlp: Option<PathBuf>,
    /// Interpreter that runs `ytdlp` when it's a script rather than a program (Android ships a
    /// Python launcher and the yt-dlp zipapp). `None` runs `ytdlp` itself.
    pub ytdlp_launcher: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub js_runtime: Option<JsRuntime>,
}

/// Tools that came with the app instead of being found on disk (the Android APK's), filling in
/// whatever [`ToolPaths::locate`] doesn't find. Desktop never sets it.
static BUNDLED: OnceLock<ToolPaths> = OnceLock::new();

/// Registers the app's own tools for [`ToolPaths::locate`]. Setting the same ones again is fine;
/// different ones fail, since running jobs would be holding the old paths.
pub fn set_bundled(tools: ToolPaths) -> Result<()> {
    let set = BUNDLED.get_or_init(|| tools.clone());
    ensure!(*set == tools, "bundled tools are already set");
    Ok(())
}

/// Environment every tool runs with on top of Redline's own (Android's yt-dlp and ffmpeg need
/// `LD_LIBRARY_PATH`, `PYTHONHOME` and friends). Applied in [`command`] rather than set on the
/// process: changing the environment of a process whose other threads may read it (the JVM's,
/// on Android) is a data race, and every tool already starts through [`command`].
static CHILD_ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();

/// Sets the extra environment for every tool started from now on. Same rules as [`set_bundled`].
pub fn set_child_env(vars: Vec<(String, String)>) -> Result<()> {
    let set = CHILD_ENV.get_or_init(|| vars.clone());
    ensure!(*set == vars, "the tools' environment is already set");
    Ok(())
}

impl ToolPaths {
    pub fn locate(s: &Settings) -> Self {
        Self::locate_with(s, BUNDLED.get())
    }

    fn locate_with(s: &Settings, bundled: Option<&ToolPaths>) -> Self {
        let bundled_path = |pick: fn(&ToolPaths) -> &Option<PathBuf>| bundled.and_then(|b| pick(b).clone());
        let ffmpeg = find("ffmpeg", s.ffmpeg_path.as_deref()).or_else(|| bundled_path(|b| &b.ffmpeg));
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
            .or_else(|| find("ffprobe", None))
            .or_else(|| bundled_path(|b| &b.ffprobe));
        let deno = find("deno", s.deno_path.as_deref()).map(|path| JsRuntime { name: "deno".into(), path });
        Self {
            ytdlp: find("yt-dlp", s.ytdlp_path.as_deref()).or_else(|| bundled_path(|b| &b.ytdlp)),
            ytdlp_launcher: bundled_path(|b| &b.ytdlp_launcher),
            ffmpeg,
            ffprobe,
            js_runtime: deno.or_else(|| bundled.and_then(|b| b.js_runtime.clone())),
        }
    }

    /// A command that runs yt-dlp (through its launcher, if it has one), with no arguments yet.
    pub fn ytdlp_command(&self) -> Result<Command> {
        let ytdlp = self.ytdlp.as_ref().ok_or_else(|| anyhow!("yt-dlp is not installed"))?;
        Ok(match &self.ytdlp_launcher {
            Some(launcher) => {
                let mut cmd = command(launcher);
                cmd.arg(ytdlp);
                cmd
            }
            None => command(ytdlp),
        })
    }

    pub fn ready_for_download(&self) -> bool {
        self.ytdlp.is_some() && self.ffmpeg.is_some()
    }

    pub fn ready_for_convert(&self) -> bool {
        self.ffmpeg.is_some() && self.ffprobe.is_some()
    }

    pub fn missing_any(&self) -> bool {
        self.ytdlp.is_none() || self.ffmpeg.is_none() || self.ffprobe.is_none() || self.js_runtime.is_none()
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

/// A `Command` that won't flash a console window on Windows, with the tools' extra environment
/// (see [`set_child_env`]). Elsewhere it starts in a process group of its own, so
/// [`kill_tree`] can take its children down with it.
pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    if let Some(vars) = CHILD_ENV.get() {
        cmd.envs(vars.iter().map(|(k, v)| (k, v)));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The group's id is the child's pid. Tools never read the terminal, so leaving its
        // foreground group costs nothing.
        cmd.process_group(0);
    }
    cmd
}

/// Kills a process and its children. yt-dlp.exe is a PyInstaller bundle that re-spawns
/// itself and launches ffmpeg, so killing only the top process leaves orphans. Elsewhere
/// yt-dlp's ffmpeg (and Android's python) would outlive it the same way; there the whole
/// process group [`command`] made goes.
pub fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = command(Path::new("taskkill"))
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        // The child isn't reaped yet (the caller holds it), so no other process or group can
        // have its id; one started without `command` leads no group and this just fails.
        // SAFETY: kill only sends a signal.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
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

/// Opens Explorer with the file selected (or just the folder elsewhere). Does nothing on
/// Android, where the app shows files itself.
#[cfg(not(target_os = "android"))]
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

#[cfg(target_os = "android")]
pub fn reveal(_path: &Path) {}

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

    #[test]
    fn bundled_tools_fill_in_and_overrides_win() {
        let dir = std::env::temp_dir().join(format!("redline-locate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mine = dir.join("my-yt-dlp");
        std::fs::write(&mine, b"").unwrap();
        let bundled = ToolPaths {
            ytdlp: Some(PathBuf::from("/app/yt-dlp")),
            ytdlp_launcher: Some(PathBuf::from("/app/libpython.so")),
            ..Default::default()
        };
        let s = Settings { ytdlp_path: Some(mine.clone()), ..Default::default() };
        let t = ToolPaths::locate_with(&s, Some(&bundled));
        assert_eq!(t.ytdlp, Some(mine));
        assert_eq!(t.ytdlp_launcher, bundled.ytdlp_launcher);
        // Without bundled tools (desktop), nothing runs through a launcher.
        assert_eq!(ToolPaths::locate_with(&s, None).ytdlp_launcher, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Like yt-dlp's ffmpeg: a grandchild the killed process started must go too.
    #[cfg(unix)]
    #[test]
    fn kill_tree_takes_grandchildren() {
        let mut cmd = command(Path::new("sh"));
        cmd.args(["-c", "sleep 100 & echo $!; wait"]).stdout(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let grandchild: libc::pid_t = line.trim().parse().unwrap();
        // SAFETY: signal 0 only checks that the process exists.
        let alive = || unsafe { libc::kill(grandchild, 0) } == 0;
        assert!(alive());
        kill_tree(&mut child);
        let _ = child.wait();
        // Once killed, init (or a subreaper) reaps it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive() && std::time::Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!alive(), "sleep outlived its parent");
    }
}
