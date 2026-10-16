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
