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
            ..Default::default()
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
        ..Default::default()
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
            ..Default::default()
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
        ..Default::default()
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

    let info = ytdlp::fetch_info(&tools, &Default::default(), url).expect("fetch_info");
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
            ..Default::default()
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
        ..Default::default()
    };
    let out = ytdlp::download(&tools, &req, &CancelHandle::default(), &Reporter::for_test())
        .unwrap_or_else(|e| panic!("trimmed: {e:#}"))
        .expect("yt-dlp reported no output file");
    let d = ffmpeg::probe_duration(tools.ffprobe.as_ref().unwrap(), &out);
    println!("trimmed -> {} ({d:?}s)", out.display());
    assert!(d.is_some_and(|d| (d - 6.0).abs() < 1.0), "trimmed duration {d:?}");
}

#[test]
#[ignore]
fn downloads_with_formats_tags_subs_and_archive() {
    let tools = ensure_tools();
    let dir = scratch("dl-extras");
    let url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    let info = ytdlp::fetch_info(&tools, &Default::default(), url).expect("fetch_info");
    let choices = ytdlp::height_choices(&info.formats, info.duration);
    println!("heights: {choices:?}");
    assert!(!choices.is_empty(), "no video heights parsed");
    let audio = info
        .formats
        .iter()
        .find(|f| f.has_audio() && !f.has_video())
        .expect("an audio-only format");

    // Exact audio format, custom tags, archive.
    let archive = dir.join("archive.txt");
    let req = DownloadRequest {
        url: url.into(),
        options: DownloadOptions {
            mode: Mode::Audio,
            audio_format: "m4a".into(),
            square_cover: true,
            ..Default::default()
        },
        out_dir: dir.clone(),
        archive: Some(archive.clone()),
        format: Some(audio.id.clone()),
        tags: Some(ytdlp::Tags {
            title: "Zoo Test".into(),
            artist: "Redline".into(),
            album: "E2E".into(),
        }),
        ..Default::default()
    };
    let out = ytdlp::download(&tools, &req, &CancelHandle::default(), &Reporter::for_test())
        .unwrap_or_else(|e| panic!("tagged audio: {e:#}"))
        .expect("no output file");
    let probe = command(tools.ffprobe.as_ref().unwrap())
        .args(["-v", "error", "-show_entries", "format_tags=title,artist,album", "-of", "default=nw=1"])
        .arg(&out)
        .output()
        .unwrap();
    let tags = String::from_utf8_lossy(&probe.stdout).to_lowercase();
    println!("tags: {tags}");
    assert!(tags.contains("title=zoo test") && tags.contains("artist=redline") && tags.contains("album=e2e"));
    assert!(std::fs::read_to_string(&archive).unwrap().contains("jNQXAC9IVRw"));

    // Second run is skipped by the archive.
    let again = ytdlp::download(&tools, &req, &CancelHandle::default(), &Reporter::for_test());
    assert!(again.as_ref().is_err_and(|e| e.is::<crate::jobs::AlreadyDownloaded>()), "{:?}", again.err());

    // Auto captions saved as .srt.
    let req = DownloadRequest {
        url: url.into(),
        options: DownloadOptions {
            max_height: 240,
            embed_subs: true,
            auto_subs: true,
            subs_mode: ytdlp::SubsMode::Srt,
            ..Default::default()
        },
        out_dir: dir.join("subs"),
        ..Default::default()
    };
    ytdlp::download(&tools, &req, &CancelHandle::default(), &Reporter::for_test())
        .unwrap_or_else(|e| panic!("subs: {e:#}"));
    let srts: Vec<_> = std::fs::read_dir(dir.join("subs"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "srt"))
        .collect();
    println!("srt files: {}", srts.len());
    assert!(!srts.is_empty(), "no .srt written");
}
