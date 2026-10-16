//! End-to-end checks against the real tools. They download ~200 MB on first run and hit the
//! network, so they're opt-in: `cargo test -- --ignored --nocapture`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use super::convert::{Category, ConvertOptions, ConvertRequest, GifDirection, GifOptions};
use super::install::Installer;
use super::ytdlp::{DownloadOptions, DownloadRequest, Mode, Trim};
use super::*;
use crate::jobs::{CancelHandle, Cancelled, Reporter};
use crate::settings::Settings;

fn ensure_tools() -> ToolPaths {
    let tools = ToolPaths::locate(&Settings::default());
    if tools.missing_any() {
        let installer = Installer::default();
        installer.start(&tools, &egui::Context::default());
        let start = Instant::now();
        while installer.status().running {
            assert!(start.elapsed() < Duration::from_secs(900), "install timed out");
            std::thread::sleep(Duration::from_millis(500));
        }
        if let Some(e) = installer.status().error {
            panic!("install failed: {e}");
        }
    }
    let tools = ToolPaths::locate(&Settings::default());
    assert!(tools.ready_for_download() && tools.ready_for_convert(), "tools missing after install");
    tools
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("redline-e2e-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_clip(tools: &ToolPaths, dir: &std::path::Path, secs: u32) -> PathBuf {
    let clip = dir.join(format!("clip{secs}.mp4"));
    let status = command(tools.ffmpeg.as_ref().unwrap())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=duration={secs}:size=640x360:rate=30"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={secs}"))
        .args(["-shortest", "-c:v", "libx264", "-c:a", "aac"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success());
    clip
}

#[test]
#[ignore]
fn converts_video_and_audio_with_ffmpeg() {
    let tools = ensure_tools();
    let dir = scratch("convert");
    let clip = make_clip(&tools, &dir, 3);
    let rep = Reporter::for_test();

    for (category, input, target) in [
        (Category::Video, clip.clone(), "webm"),
        (Category::Video, clip.clone(), "mp3"),
        (Category::Video, clip.clone(), "gif"),
        (Category::Video, clip.clone(), "mkv"),
    ] {
        let req = ConvertRequest {
            input,
            category,
            target: target.into(),
            options: ConvertOptions {
                max_height: 240,
                preset: "ultrafast".into(),
                ..Default::default()
            },
            out_dir: Some(dir.clone()),
        };
        let out = convert::run(&tools, &req, &CancelHandle::default(), &rep)
            .unwrap_or_else(|e| panic!("{target}: {e:#}"))
            .unwrap();
        assert!(out.extension().unwrap() == target);
        let d = ffmpeg::probe_duration(tools.ffprobe.as_ref().unwrap(), &out);
        println!("{target}: {} ({:?}s)", out.display(), d);
        assert!(d.is_some_and(|d| (d - 3.0).abs() < 0.6), "{target} duration {d:?}");
    }

    // mp3 → flac (audio-to-audio)
    let mp3 = dir.join("clip3.mp3");
    let req = ConvertRequest {
        input: mp3,
        category: Category::Audio,
        target: "flac".into(),
        options: ConvertOptions::default(),
        out_dir: None,
    };
    let out = convert::run(&tools, &req, &CancelHandle::default(), &rep).unwrap().unwrap();
    assert!(out.is_file());
    println!("flac: {}", out.display());
}

#[test]
#[ignore]
fn converts_video_to_custom_gifs() {
    let tools = ensure_tools();
    let dir = scratch("gif");
    let clip = make_clip(&tools, &dir, 6);
    let rep = Reporter::for_test();

    let cases = [
        // (options, expected output seconds)
        (GifOptions::default(), 6.0),
        (
            GifOptions {
                start: 1.0,
                length: 2.0,
                speed: 2.0,
                direction: GifDirection::Boomerang,
                fps: 10,
                width: 0,
                colors: 32,
                dither: "bayer".into(),
                diff_only: true,
                plays: 2,
            },
            2.0,
        ),
        (
            GifOptions {
                start: 4.0,
                speed: 0.5,
                direction: GifDirection::Reverse,
                width: 240,
                dither: "none".into(),
                plays: 1,
                ..Default::default()
            },
            4.0,
        ),
    ];
    for (i, (gif, expected)) in cases.into_iter().enumerate() {
        let req = ConvertRequest {
            input: clip.clone(),
            category: Category::Video,
            target: "gif".into(),
            options: ConvertOptions { gif, ..Default::default() },
            out_dir: Some(dir.join(i.to_string())),
        };
        let out = convert::run(&tools, &req, &CancelHandle::default(), &rep)
            .unwrap_or_else(|e| panic!("case {i}: {e:#}"))
            .unwrap();
        let d = ffmpeg::probe_duration(tools.ffprobe.as_ref().unwrap(), &out);
        let size = std::fs::metadata(&out).unwrap().len();
        println!("case {i}: {} ({d:?}s, {size} bytes)", out.display());
        assert!(d.is_some_and(|d| (d - expected).abs() < 0.4), "case {i} duration {d:?}");
    }
}

#[test]
#[ignore]
fn cancelling_kills_ffmpeg() {
    let tools = ensure_tools();
    let dir = scratch("cancel");
    let clip = make_clip(&tools, &dir, 30);
    let cancel = Arc::new(CancelHandle::default());
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        c2.cancel();
    });
    let req = ConvertRequest {
        input: clip,
        category: Category::Video,
        target: "webm".into(),
        options: ConvertOptions {
            crf: 10,
            ..Default::default()
        },
        out_dir: Some(dir.clone()),
    };
    let start = Instant::now();
    let err = convert::run(&tools, &req, &cancel, &Reporter::for_test()).unwrap_err();
    assert!(err.is::<Cancelled>(), "expected Cancelled, got {err:#}");
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(!dir.join("clip30.webm").exists(), "partial output left behind");
}

#[test]
#[ignore]
fn downloads_with_ytdlp() {
    let tools = ensure_tools();
    let dir = scratch("download");
    // "Me at the zoo": 19 seconds and unlikely to ever go away.
    let url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

    let info = ytdlp::fetch_info(&tools, url).expect("fetch_info");
    println!("info: {} by {:?}, {:?}s", info.title, info.uploader, info.duration);
    assert!(!info.title.is_empty());

    for options in [
        DownloadOptions {
            mode: Mode::Audio,
            audio_format: "mp3".into(),
            audio_bitrate: "128K".into(),
            ..Default::default()
        },
        DownloadOptions {
            mode: Mode::Video,
            max_height: 360,
            container: "mp4".into(),
            ..Default::default()
        },
    ] {
        let req = DownloadRequest {
            url: url.into(),
            options: options.clone(),
            out_dir: dir.clone(),
            trim: None,
        };
        let out = ytdlp::download(&tools, &req, &CancelHandle::default(), &Reporter::for_test())
            .unwrap_or_else(|e| panic!("{}: {e:#}", options.summary()));
        let out = out.expect("yt-dlp reported no output file");
        println!("{} -> {}", options.summary(), out.display());
        assert!(out.is_file());
        assert!(std::fs::metadata(&out).unwrap().len() > 10_000);
    }

    let req = DownloadRequest {
        url: url.into(),
        options: DownloadOptions {
            max_height: 360,
            ..Default::default()
        },
        out_dir: dir.clone(),
        trim: Some(Trim {
            start: 3.0,
            end: Some(9.0),
            precise: true,
        }),
    };
    let out = ytdlp::download(&tools, &req, &CancelHandle::default(), &Reporter::for_test())
        .unwrap_or_else(|e| panic!("trimmed: {e:#}"))
        .expect("yt-dlp reported no output file");
    let d = ffmpeg::probe_duration(tools.ffprobe.as_ref().unwrap(), &out);
    println!("trimmed -> {} ({d:?}s)", out.display());
    assert!(d.is_some_and(|d| (d - 6.0).abs() < 1.0), "trimmed duration {d:?}");
}
