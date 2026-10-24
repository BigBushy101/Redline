//! Starting Redline inside the Android app. The APK ships the tools as native libraries
//! (`libpython.so`, `libffmpeg.so`, `libffprobe.so`, `libqjs.so`) and youtubedl-android unpacks
//! Python, ffmpeg's shared libraries and the yt-dlp script into the app's storage; the Kotlin
//! side initialises that library and then hands everything to [`init`].

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::settings::{self, AppDirs, Settings};
use crate::tools::{self, JsRuntime, ToolPaths, exe_name};

/// Where things are on the device, as youtubedl-android lays them out.
#[derive(Clone, Debug)]
pub struct AndroidPaths {
    /// Settings, history, the queue and `bin/` (e.g. `Context.filesDir`).
    pub data_dir: PathBuf,
    /// Where downloads go until the user picks another folder.
    pub download_dir: PathBuf,
    /// `<nativeLibraryDir>/libpython.so`, which runs the yt-dlp script.
    pub python: PathBuf,
    /// `<noBackupFilesDir>/youtubedl-android/yt-dlp/yt-dlp`. In writable storage, so
    /// `yt-dlp -U` can replace it.
    pub ytdlp_script: PathBuf,
    /// `quickjs` at `<nativeLibraryDir>/libqjs.so`.
    pub js_runtime: JsRuntime,
    /// `<nativeLibraryDir>/libffmpeg.so`.
    pub ffmpeg: PathBuf,
    /// `<nativeLibraryDir>/libffprobe.so`.
    pub ffprobe: PathBuf,
    /// What every tool runs with: `LD_LIBRARY_PATH`, `PYTHONHOME`, `HOME`, `SSL_CERT_FILE`,
    /// `PATH` and `TMPDIR`, as youtubedl-android sets them.
    pub env: Vec<(String, String)>,
}

/// Points Redline at the app's folders and tools, and returns the tools it will use. Call it
/// once the Kotlin side has initialised youtubedl-android and before anything else in this
/// crate. Calling it again with the same paths is fine (a recreated activity in the same
/// process does).
pub fn init(paths: AndroidPaths) -> Result<ToolPaths> {
    settings::set_app_dirs(AppDirs {
        data: paths.data_dir.clone(),
        downloads: paths.download_dir.clone(),
    })?;
    tools::set_child_env(paths.env.clone())?;

    // yt-dlp's `--ffmpeg-location` wants a folder holding `ffmpeg` and `ffprobe`, and the
    // APK's are `libffmpeg.so`/`libffprobe.so`, so give them those names in `bin/`. Android
    // lets an app run a native library through a symlink.
    let bin = tools::bin_dir();
    fs::create_dir_all(&bin).with_context(|| format!("can't create {}", bin.display()))?;
    link_tool(&paths.ffmpeg, &bin.join(exe_name("ffmpeg")))?;
    link_tool(&paths.ffprobe, &bin.join(exe_name("ffprobe")))?;

    tools::set_bundled(bundled_tools(&paths, &bin))?;
    let tools = ToolPaths::locate(&Settings::load());
    // Nothing has run yt-dlp yet, so copies of older scripts can go now.
    tools::install::remove_stale_unpacked(&tools);
    let unpack = tools.clone();
    std::thread::spawn(move || tools::install::unpack_ytdlp(&unpack));
    Ok(tools)
}

/// The APK's tools as [`ToolPaths::locate`] should see them, with ffmpeg and ffprobe through
/// their links in `bin`.
fn bundled_tools(paths: &AndroidPaths, bin: &Path) -> ToolPaths {
    ToolPaths {
        ytdlp: Some(paths.ytdlp_script.clone()),
        ytdlp_launcher: Some(paths.python.clone()),
        ffmpeg: Some(bin.join(exe_name("ffmpeg"))),
        ffprobe: Some(bin.join(exe_name("ffprobe"))),
        js_runtime: Some(paths.js_runtime.clone()),
    }
}

/// Points `link` at `target`, replacing a link to an older install's copy (the APK's native
/// library folder moves with every app update).
#[cfg(unix)]
fn link_tool(target: &Path, link: &Path) -> Result<()> {
    if fs::read_link(link).is_ok_and(|t| t == target) {
        return Ok(());
    }
    match fs::remove_file(link) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("can't replace {}", link.display()));
        }
        _ => {}
    }
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("can't link {} to {}", link.display(), target.display()))
}

/// Only so the module builds for desktop tests; [`init`] never runs there.
#[cfg(not(unix))]
fn link_tool(_target: &Path, _link: &Path) -> Result<()> {
    anyhow::bail!("tool links need a unix host")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> AndroidPaths {
        let lib = PathBuf::from("/data/app/~~x/app-y/lib/x86_64");
        AndroidPaths {
            data_dir: PathBuf::from("/data/user/0/app/files"),
            download_dir: PathBuf::from("/storage/emulated/0/Download/Redline"),
            python: lib.join("libpython.so"),
            ytdlp_script: PathBuf::from("/data/user/0/app/no_backup/youtubedl-android/yt-dlp/yt-dlp"),
            js_runtime: JsRuntime { name: "quickjs".into(), path: lib.join("libqjs.so") },
            ffmpeg: lib.join("libffmpeg.so"),
            ffprobe: lib.join("libffprobe.so"),
            env: vec![("PYTHONHOME".into(), "/data/user/0/app/no_backup/youtubedl-android/packages/python/usr".into())],
        }
    }

    #[test]
    fn bundled_tools_run_the_script_through_python() {
        let p = paths();
        let bin = p.data_dir.join("bin");
        let t = bundled_tools(&p, &bin);
        assert_eq!(t.ytdlp.as_ref(), Some(&p.ytdlp_script));
        assert_eq!(t.ytdlp_launcher.as_ref(), Some(&p.python));
        assert_eq!(t.ffmpeg.as_deref().and_then(Path::parent), Some(bin.as_path()));
        assert_eq!(t.ffprobe.as_deref().and_then(Path::parent), Some(bin.as_path()));
        assert_eq!(t.js_runtime, Some(p.js_runtime));
        assert!(t.ready_for_download() && t.ready_for_convert() && !t.missing_any());
    }

    #[cfg(unix)]
    #[test]
    fn links_follow_the_library_folder() {
        let dir = std::env::temp_dir().join(format!("redline-android-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (old, new, link) = (dir.join("old.so"), dir.join("new.so"), dir.join("ffmpeg"));
        fs::write(&old, b"").unwrap();
        fs::write(&new, b"").unwrap();
        link_tool(&old, &link).unwrap();
        link_tool(&old, &link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), old);
        link_tool(&new, &link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), new);
        fs::remove_dir_all(&dir).unwrap();
    }
}
