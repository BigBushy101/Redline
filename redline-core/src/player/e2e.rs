//! Player checks against the real ffmpeg (and, for links, yt-dlp and the network).
//! Opt-in like the other end-to-end tests: `cargo test -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use super::output::Capture;
use super::*;
use crate::tools::command;
use crate::tools::e2e::{ensure_tools, scratch};

const RATE: u32 = 48_000;

fn wait_for(p: &Player, what: &str, secs: u64, cond: impl Fn(&PlayerState) -> bool) -> PlayerState {
    let start = Instant::now();
    loop {
        let s = p.state();
        if cond(&s) {
            return s;
        }
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}: {s:?}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn loud_frames(c: &Capture) -> usize {
    c.samples
        .lock()
        .unwrap()
        .chunks(2)
        .filter(|f| f[0].abs() > 1e-3 || f[1].abs() > 1e-3)
        .count()
}

#[test]
#[ignore]
fn plays_a_local_tone_to_the_end() {
    let tools = ensure_tools();
    let dir = scratch("player-local");
    let tone = dir.join("tone.m4a");
    let status = command(tools.ffmpeg.as_ref().unwrap())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"])
        .args(["-metadata", "title=Test tone", "-metadata", "artist=Redline"])
        .arg(&tone)
        .status()
        .unwrap();
    assert!(status.success());

    let capture = Capture::new(RATE, 4.0);
    let p = Player::spawn(tools, Arc::new(Ffmpeg), capture.opener(), crate::no_wake());
    p.send(Command::Load(Source::Local(tone), 0.0));
    let s = wait_for(&p, "playing", 10, |s| s.status == Status::Playing);
    let track = s.track.unwrap();
    assert_eq!(track.title, "Test tone");
    assert_eq!(track.artist.as_deref(), Some("Redline"));
    assert!((s.duration.unwrap() - 3.0).abs() < 0.1, "{:?}", s.duration);
    wait_for(&p, "end", 10, |s| s.status == Status::Ended);
    let secs = loud_frames(&capture) as f64 / RATE as f64;
    println!("{secs:.3} s of sound");
    assert!((2.85..=3.1).contains(&secs), "{secs:.3} s of sound");
}

#[test]
#[ignore]
fn streams_a_youtube_link_and_seeks() {
    let tools = ensure_tools();
    let capture = Capture::new(RATE, 1.0);
    let p = Player::spawn(tools.clone(), Arc::new(Ffmpeg), capture.opener(), crate::no_wake());
    p.send(Command::Load(Source::Link("https://www.youtube.com/watch?v=jNQXAC9IVRw".into()), 0.0));
    let s = wait_for(&p, "playing", 60, |s| s.status == Status::Playing && !s.buffering);
    let track = s.track.clone().unwrap();
    println!("{track:?}");
    assert_eq!(track.title, "Me at the zoo");
    assert!(track.thumbnail.is_some());
    assert!((s.duration.unwrap() - 19.0).abs() < 1.5, "{:?}", s.duration);

    let started = capture.frames();
    wait_for(&p, "2 s in", 20, |s| s.position >= 2.0);
    let played = (capture.frames() - started) as f64 / RATE as f64;
    println!("2 s took {played:.2} s of output");
    assert!(loud_frames(&capture) > RATE as usize, "sound came out");

    p.send(Command::Seek(10.0));
    let s = wait_for(&p, "seeked", 20, |s| s.position >= 10.0 && !s.buffering);
    assert!(s.position < 10.6, "{}", s.position);
    let before = loud_frames(&capture);
    let s = wait_for(&p, "1 s more", 20, |s| s.position >= 11.0);
    assert_eq!(s.status, Status::Playing);
    let after = loud_frames(&capture);
    println!("{} loud frames after the seek", after - before);
    assert!(after - before > RATE as usize / 2, "sound after the seek");
}

/// With browser cookies, YouTube hands out stream URLs that refuse the first few seconds
/// of requests. Set `REDLINE_COOKIES_BROWSER` (e.g. `firefox`) to run it.
#[test]
#[ignore]
fn streams_a_youtube_link_with_browser_cookies() {
    let Ok(browser) = std::env::var("REDLINE_COOKIES_BROWSER") else {
        println!("REDLINE_COOKIES_BROWSER not set; skipped");
        return;
    };
    let tools = ensure_tools();
    let capture = Capture::new(RATE, 1.0);
    let p = Player::spawn(tools.clone(), Arc::new(Ffmpeg), capture.opener(), crate::no_wake());
    p.send(Command::SetNetwork(ytdlp::NetworkOptions {
        cookies_browser: Some(browser),
        ..Default::default()
    }));
    p.send(Command::Load(Source::Link("https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()), 0.0));
    wait_for(&p, "playing", 90, |s| {
        assert!(!matches!(s.status, Status::Error(_)), "{:?}", s.status);
        s.status == Status::Playing && !s.buffering && s.position >= 1.0
    });
    assert!(loud_frames(&capture) > RATE as usize / 2, "sound came out");
}

#[test]
#[ignore]
fn double_speed_and_eq_on_a_real_track() {
    let tools = ensure_tools();
    let dir = scratch("player-speed");
    let tone = dir.join("tone.wav");
    let status = command(tools.ffmpeg.as_ref().unwrap())
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=1000:duration=3"])
        .arg(&tone)
        .status()
        .unwrap();
    assert!(status.success());

    let capture = Capture::new(RATE, 4.0);
    let p = Player::spawn(tools, Arc::new(Ffmpeg), capture.opener(), crate::no_wake());
    p.send(Command::SetSpeed(2.0));
    let mut eq = dsp::EqSettings::default();
    eq.bands_db[9] = 12.0; // 16 kHz: shouldn't touch a 2 kHz tone much
    p.send(Command::SetEq(eq));
    p.send(Command::Load(Source::Local(tone), 0.0));
    wait_for(&p, "end", 10, |s| s.status == Status::Ended);
    let samples = capture.samples.lock().unwrap().clone();
    let left: Vec<f32> = samples.chunks(2).map(|f| f[0]).filter(|v| v.abs() > 1e-3).collect();
    let secs = left.len() as f64 / RATE as f64;
    // Count rising zero crossings for the pitch.
    let crossings = left.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    let freq = crossings as f64 / secs;
    println!("{secs:.3} s at about {freq:.0} Hz");
    assert!((1.4..=1.55).contains(&secs), "{secs:.3} s");
    assert!((freq - 2000.0).abs() < 60.0, "{freq:.0} Hz");
}
