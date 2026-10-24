//! Engine tests with a fake decoder and the capture output, so no ffmpeg or sound card.

use std::io::{Cursor, Read};

use super::output::Capture;
use super::*;

/// Decodes to a 440 Hz tone of `seconds`, recording each offset it's asked to start from.
struct Tone {
    seconds: f64,
    starts: Mutex<Vec<f64>>,
    fail_decode: bool,
}

impl Tone {
    fn new(seconds: f64) -> Arc<Self> {
        Arc::new(Self {
            seconds,
            starts: Mutex::default(),
            fail_decode: false,
        })
    }
}

impl Media for Tone {
    fn resolve(&self, _: &Env, source: &Source) -> Result<Resolved> {
        Ok(Resolved {
            input: Input {
                location: "tone".into(),
                headers: None,
            },
            info: TrackInfo {
                title: format!("Tone {}", source.label()),
                duration: Some(self.seconds),
                ..Default::default()
            },
        })
    }

    fn decode(&self, _: &Env, _: &Input, start: f64, rate: u32) -> Result<Pcm> {
        if self.fail_decode {
            return Err(anyhow!("can't decode"));
        }
        self.starts.lock().unwrap().push(start);
        let first = (start * rate as f64) as usize;
        let last = (self.seconds * rate as f64) as usize;
        let mut bytes = Vec::with_capacity((last - first) * 8);
        for i in first..last {
            let v = 0.5 * (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin();
            bytes.extend_from_slice(&v.to_le_bytes());
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        Ok(Pcm {
            reader: Box::new(Cursor::new(bytes)),
            kill: Box::new(|| {}),
            finish: Box::new(|| Ok(())),
        })
    }
}

const RATE: u32 = 8000;

fn player(media: Arc<dyn Media>, capture: &Capture) -> Player {
    Player::spawn(ToolPaths::default(), media, capture.opener(), crate::no_wake())
}

fn wait_for(p: &Player, what: &str, cond: impl Fn(&PlayerState) -> bool) -> PlayerState {
    let start = Instant::now();
    loop {
        let s = p.state();
        if cond(&s) {
            return s;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}: {s:?}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn loud_frames(c: &Capture) -> usize {
    c.samples
        .lock()
        .unwrap()
        .chunks(2)
        .filter(|f| f[0].abs() > 1e-4)
        .count()
}

fn track() -> Source {
    Source::Local("a.mp3".into())
}

#[test]
fn loads_plays_to_the_end() {
    let capture = Capture::new(RATE, 8.0);
    let p = player(Tone::new(1.0), &capture);
    assert_eq!(p.state().status, Status::Idle);
    p.send(Command::Load(track(), 0.0));
    assert!(wait_for(&p, "loading", |s| s.is_active()).track.is_some());
    let s = wait_for(&p, "playing", |s| s.status == Status::Playing);
    assert_eq!(s.track.unwrap().title, "Tone a");
    assert_eq!(s.duration, Some(1.0));
    let s = wait_for(&p, "end", |s| s.status == Status::Ended);
    assert_eq!(s.position, 1.0);
    let loud = loud_frames(&capture);
    // Fades and zero crossings shave a little off.
    assert!((RATE as usize * 95 / 100..=RATE as usize).contains(&loud), "{loud} loud frames");
}

#[test]
fn seek_restarts_the_decoder_at_the_offset() {
    let capture = Capture::new(RATE, 4.0);
    let tone = Tone::new(3.0);
    let p = player(tone.clone(), &capture);
    p.send(Command::Load(track(), 0.5));
    wait_for(&p, "playing", |s| s.status == Status::Playing && s.position > 0.6);
    p.send(Command::Seek(2.5));
    let s = wait_for(&p, "seeked", |s| s.position >= 2.5);
    assert!(s.position < 2.9, "{}", s.position);
    assert_eq!(*tone.starts.lock().unwrap(), vec![0.5, 2.5]);
    wait_for(&p, "end", |s| s.status == Status::Ended);

    // Play after the end starts over.
    p.send(Command::Toggle);
    wait_for(&p, "replay", |s| s.status == Status::Playing);
    assert_eq!(tone.starts.lock().unwrap().last(), Some(&0.0));

    // Seeking past the end clamps to the duration.
    p.send(Command::Seek(99.0));
    wait_for(&p, "end again", |s| s.status == Status::Ended);
}

#[test]
fn pause_holds_the_position() {
    let capture = Capture::new(RATE, 2.0);
    let p = player(Tone::new(5.0), &capture);
    p.send(Command::Load(track(), 0.0));
    wait_for(&p, "playing", |s| s.status == Status::Playing && s.position > 0.2);
    p.send(Command::Pause);
    let paused = wait_for(&p, "paused", |s| s.status == Status::Paused);
    thread::sleep(Duration::from_millis(300));
    let later = p.state();
    // The fade-out may use a few more milliseconds.
    assert!((later.position - paused.position).abs() < 0.05, "{} vs {}", later.position, paused.position);
    p.send(Command::Toggle);
    wait_for(&p, "resumed", |s| s.status == Status::Playing && s.position > later.position + 0.1);
    p.send(Command::Stop);
    let s = wait_for(&p, "stopped", |s| s.status == Status::Idle);
    assert!(!s.is_active());
}

#[test]
fn reopens_a_lost_device_and_carries_on() {
    let capture = Capture::new(RATE, 4.0);
    let tone = Tone::new(2.0);
    let p = player(tone.clone(), &capture);
    p.send(Command::Load(track(), 0.0));
    wait_for(&p, "playing", |s| s.status == Status::Playing && s.position > 0.5);
    capture.lose.store(true, Ordering::Relaxed);
    wait_for(&p, "restarted", |_| tone.starts.lock().unwrap().len() == 2);
    let resumed_at = tone.starts.lock().unwrap()[1];
    assert!(resumed_at > 0.4, "picked up near where it was: {resumed_at}");
    wait_for(&p, "end", |s| s.status == Status::Ended);
}

#[test]
fn decode_failure_is_an_error_state() {
    let capture = Capture::new(RATE, 4.0);
    let p = player(
        Arc::new(Tone {
            seconds: 1.0,
            starts: Mutex::default(),
            fail_decode: true,
        }),
        &capture,
    );
    p.send(Command::Load(track(), 0.0));
    let s = wait_for(&p, "error", |s| matches!(s.status, Status::Error(_)));
    assert_eq!(s.status, Status::Error("can't decode".into()));
    assert!(!s.buffering);
}

#[test]
fn missing_local_file_fails_to_resolve() {
    let err = Ffmpeg
        .resolve(&Env::default(), &Source::Local("Z:/nope/gone.mp3".into()))
        .unwrap_err();
    assert!(err.to_string().contains("moved or deleted"), "{err}");
}

/// Holds back the end of a stream for a moment, like a connection that stalls then fails.
struct SlowEnd(Cursor<Vec<u8>>);

impl Read for SlowEnd {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read(out)?;
        if n == 0 {
            thread::sleep(Duration::from_millis(300));
        }
        Ok(n)
    }
}

/// A link whose first stream URL dies with a 403 partway through.
struct ExpiringLink {
    resolves: Mutex<u32>,
    starts: Mutex<Vec<(String, f64)>>,
}

impl Media for ExpiringLink {
    fn resolve(&self, _: &Env, _: &Source) -> Result<Resolved> {
        let mut n = self.resolves.lock().unwrap();
        *n += 1;
        Ok(Resolved {
            input: Input {
                location: format!("https://cdn/{n}"),
                headers: Some("User-Agent: x\r\n".into()),
            },
            info: TrackInfo {
                title: "Link".into(),
                duration: Some(2.0),
                ..Default::default()
            },
        })
    }

    fn decode(&self, env: &Env, input: &Input, start: f64, rate: u32) -> Result<Pcm> {
        self.starts.lock().unwrap().push((input.location.clone(), start));
        let mut pcm = Tone::new(2.0).decode(env, input, start, rate)?;
        if input.location.ends_with("/1") {
            // Half a second of audio, then the server says no.
            let bytes = (rate as usize / 2) * 8;
            let mut half = vec![0u8; bytes];
            let _ = pcm.reader.read_exact(&mut half);
            pcm.reader = Box::new(SlowEnd(Cursor::new(half)));
            pcm.finish = Box::new(|| Err(anyhow!("Server returned 403 Forbidden (access denied)")));
        }
        Ok(pcm)
    }
}

#[test]
fn expired_stream_url_is_resolved_again_and_resumes() {
    let capture = Capture::new(RATE, 4.0);
    let media = Arc::new(ExpiringLink {
        resolves: Mutex::default(),
        starts: Mutex::default(),
    });
    let p = player(media.clone(), &capture);
    p.send(Command::Load(Source::Link("https://youtu.be/x".into()), 0.0));
    let s = wait_for(&p, "end", |s| s.status == Status::Ended || matches!(s.status, Status::Error(_)));
    assert_eq!(s.status, Status::Ended);
    assert_eq!(*media.resolves.lock().unwrap(), 2);
    let starts = media.starts.lock().unwrap().clone();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_eq!(starts[1].0, "https://cdn/2");
    assert!((0.4..=0.55).contains(&starts[1].1), "resumed where it died: {starts:?}");

    // Playing it again uses the cached URL.
    p.send(Command::Load(Source::Link("https://youtu.be/x".into()), 1.5));
    wait_for(&p, "replay", |s| s.status == Status::Ended && s.load_id == 2);
    assert_eq!(*media.resolves.lock().unwrap(), 2);
}

/// A link whose fresh stream URLs refuse the first few opens, like YouTube's do.
struct ColdLink {
    resolves: Mutex<u32>,
    opens: Mutex<Vec<(String, f64)>>,
    refusals: usize,
}

impl Media for ColdLink {
    fn resolve(&self, env: &Env, source: &Source) -> Result<Resolved> {
        *self.resolves.lock().unwrap() += 1;
        Tone::new(1.0).resolve(env, source)
    }

    fn decode(&self, env: &Env, input: &Input, start: f64, rate: u32) -> Result<Pcm> {
        let mut opens = self.opens.lock().unwrap();
        opens.push((input.location.clone(), start));
        let mut pcm = Tone::new(1.0).decode(env, input, start, rate)?;
        if opens.len() <= self.refusals {
            pcm.reader = Box::new(Cursor::new(Vec::new()));
            pcm.finish = Box::new(|| Err(anyhow!("Error opening input: Server returned 403 Forbidden (access denied)")));
        }
        Ok(pcm)
    }
}

#[test]
fn refused_fresh_stream_url_is_tried_again_without_a_new_lookup() {
    let capture = Capture::new(RATE, 4.0);
    let media = Arc::new(ColdLink {
        resolves: Mutex::default(),
        opens: Mutex::default(),
        refusals: 2,
    });
    let p = player(media.clone(), &capture);
    p.send(Command::Load(Source::Link("https://youtu.be/x".into()), 0.25));
    let s = wait_for(&p, "end", |s| s.status == Status::Ended || matches!(s.status, Status::Error(_)));
    assert_eq!(s.status, Status::Ended);
    assert_eq!(*media.resolves.lock().unwrap(), 1);
    let opens = media.opens.lock().unwrap().clone();
    assert_eq!(opens, vec![("tone".to_string(), 0.25); 3]);
}

#[test]
fn stream_url_that_keeps_refusing_fails() {
    let capture = Capture::new(RATE, 4.0);
    let media = Arc::new(ColdLink {
        resolves: Mutex::default(),
        opens: Mutex::default(),
        refusals: usize::MAX,
    });
    let p = player(media.clone(), &capture);
    p.send(Command::Load(Source::Link("https://youtu.be/x".into()), 0.0));
    let start = Instant::now();
    loop {
        if matches!(p.state().status, Status::Error(_)) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "never gave up");
        thread::sleep(Duration::from_millis(20));
    }
    // Waits out the refusals once, then looks it up again once more.
    assert_eq!(*media.resolves.lock().unwrap(), 2);
}

#[test]
fn recognises_expired_urls() {
    assert!(is_expired("[https @ 0x1] HTTP error 403 Forbidden"));
    assert!(is_expired("Server returned 403 Forbidden (access denied)"));
    assert!(is_expired("https://x: Server returned 410 Gone"));
    assert!(!is_expired("Invalid data found when processing input"));
    assert!(!is_expired("C:/music/403.mp3: No such file"));
}

#[test]
fn double_speed_plays_in_half_the_time_in_source_time() {
    let capture = Capture::new(RATE, 8.0);
    let p = player(Tone::new(2.0), &capture);
    p.send(Command::SetSpeed(2.0));
    p.send(Command::Load(track(), 0.0));
    let s = wait_for(&p, "end", |s| s.status == Status::Ended);
    // Position and duration stay in the track's own time.
    assert_eq!(s.position, 2.0);
    assert_eq!(s.duration, Some(2.0));
    let secs = loud_frames(&capture) as f64 / RATE as f64;
    assert!((0.9..=1.05).contains(&secs), "{secs:.3} s of output for 2 s at 2×");

    // Out-of-range speeds are clamped.
    p.send(Command::SetSpeed(10.0));
    p.send(Command::Load(track(), 0.0));
    wait_for(&p, "end again", |s| s.status == Status::Ended && s.load_id == 2);
}

#[test]
fn moves_on_to_the_next_source_by_itself() {
    let capture = Capture::new(RATE, 8.0);
    let p = player(Tone::new(0.5), &capture);
    p.send(Command::Load(Source::Local("a.mp3".into()), 0.0));
    p.send(Command::SetNext(Some(Source::Local("b.mp3".into()))));
    let s = wait_for(&p, "advanced", |s| s.advances == 1);
    assert_eq!(s.source, Some(Source::Local("b.mp3".into())));
    assert_ne!(s.status, Status::Ended, "the next track started instead of ending");
    // The next one is used up: b plays to its end and stops there.
    let s = wait_for(&p, "end of b", |s| s.status == Status::Ended);
    assert_eq!(s.advances, 1);
    assert_eq!(s.track.unwrap().title, "Tone b");
}

/// Local files play as a 3 s tone, links as a 1 s one. Links count their lookups, and their
/// stream URLs refuse the first few opens like YouTube's.
struct QueuedLink {
    resolves: Mutex<u32>,
    link_opens: Mutex<u32>,
    refusals: u32,
}

impl Media for QueuedLink {
    fn resolve(&self, env: &Env, source: &Source) -> Result<Resolved> {
        let link = matches!(source, Source::Link(_));
        let mut r = Tone::new(if link { 1.0 } else { 3.0 }).resolve(env, source)?;
        if let Source::Link(url) = source {
            *self.resolves.lock().unwrap() += 1;
            r.input.location = url.clone();
        }
        Ok(r)
    }

    fn decode(&self, env: &Env, input: &Input, start: f64, rate: u32) -> Result<Pcm> {
        let link = input.location.starts_with("https");
        let mut pcm = Tone::new(if link { 1.0 } else { 3.0 }).decode(env, input, start, rate)?;
        if link {
            let mut opens = self.link_opens.lock().unwrap();
            *opens += 1;
            if *opens <= self.refusals {
                pcm.reader = Box::new(Cursor::new(Vec::new()));
                pcm.finish = Box::new(|| Err(anyhow!("Server returned 403 Forbidden (access denied)")));
            }
        }
        Ok(pcm)
    }
}

#[test]
fn next_link_is_looked_up_and_warmed_while_the_current_track_plays() {
    // Real time, so the warming's waits fit inside the first track.
    let capture = Capture::new(RATE, 1.0);
    let media = Arc::new(QueuedLink {
        resolves: Mutex::default(),
        link_opens: Mutex::default(),
        refusals: 2,
    });
    let p = player(media.clone(), &capture);
    let next = Source::Link("https://youtu.be/b".into());
    p.send(Command::Load(track(), 0.0));
    p.send(Command::SetNext(Some(next.clone())));
    // Looked up once and opened until it stopped refusing, all before the first track ends.
    let s = wait_for(&p, "warmed", |_| *media.link_opens.lock().unwrap() == 3);
    assert_eq!(s.advances, 0, "{s:?}");
    assert_eq!(*media.resolves.lock().unwrap(), 1);

    let s = wait_for(&p, "advanced", |s| s.advances == 1);
    assert_eq!(s.source, Some(next));
    wait_for(&p, "end of b", |s| s.status == Status::Ended);
    // Played from the cache, and its URL opened first time.
    assert_eq!(*media.resolves.lock().unwrap(), 1);
    assert_eq!(*media.link_opens.lock().unwrap(), 4);
}

#[test]
fn prefetch_for_a_replaced_next_is_dropped() {
    let capture = Capture::new(RATE, 8.0);
    let media = Arc::new(QueuedLink {
        resolves: Mutex::default(),
        link_opens: Mutex::default(),
        refusals: 0,
    });
    let p = player(media.clone(), &capture);
    p.send(Command::Load(track(), 0.0));
    p.send(Command::SetNext(Some(Source::Link("https://youtu.be/b".into()))));
    wait_for(&p, "prefetched b", |_| *media.resolves.lock().unwrap() == 1);
    p.send(Command::SetNext(Some(Source::Link("https://youtu.be/c".into()))));
    wait_for(&p, "prefetched c", |_| *media.resolves.lock().unwrap() == 2);
    wait_for(&p, "advanced", |s| s.advances == 1);
    wait_for(&p, "end of c", |s| s.status == Status::Ended);
    assert_eq!(*media.resolves.lock().unwrap(), 2, "c came from the cache");
}

#[test]
fn ab_loop_goes_back_to_a_until_cleared() {
    let capture = Capture::new(RATE, 8.0);
    let tone = Tone::new(3.0);
    let p = player(tone.clone(), &capture);
    p.send(Command::Load(track(), 0.0));
    wait_for(&p, "playing", |s| s.status == Status::Playing && s.position > 0.6);
    p.send(Command::SetLoopA(0.5));
    p.send(Command::SetLoopB(1.0));
    let s = wait_for(&p, "markers", |s| s.ab_loop.is_active());
    assert_eq!((s.ab_loop.a, s.ab_loop.b), (Some(0.5), Some(1.0)));
    // Already past B when it was set: back to A straight away, then round again.
    let start = Instant::now();
    loop {
        let s = p.state();
        assert_ne!(s.status, Status::Ended, "looped instead of ending");
        assert!(s.position <= 1.1, "stayed inside the loop: {}", s.position);
        if tone.starts.lock().unwrap().iter().filter(|t| **t == 0.5).count() >= 3 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "didn't loop: {:?}", tone.starts.lock().unwrap());
        thread::sleep(Duration::from_millis(5));
    }
    p.send(Command::ClearLoop);
    let s = wait_for(&p, "end", |s| s.status == Status::Ended);
    assert!(!s.ab_loop.is_set());

    // A new track clears the markers.
    p.send(Command::Load(track(), 0.0));
    p.send(Command::SetLoopB(2.0));
    wait_for(&p, "B", |s| s.ab_loop.is_active());
    p.send(Command::Load(track(), 0.0));
    wait_for(&p, "cleared", |s| s.load_id == 3 && !s.ab_loop.is_set());
}

#[test]
fn ab_loop_with_b_at_the_end_loops_instead_of_ending() {
    let capture = Capture::new(RATE, 8.0);
    let tone = Tone::new(1.0);
    let p = player(tone.clone(), &capture);
    p.send(Command::Load(track(), 0.0));
    wait_for(&p, "playing", |s| s.status == Status::Playing);
    p.send(Command::SetLoopA(0.2));
    p.send(Command::SetLoopB(5.0));
    let s = wait_for(&p, "B", |s| s.ab_loop.b.is_some());
    assert_eq!(s.ab_loop.b, Some(1.0), "clamped to the end");
    wait_for(&p, "looped", |s| s.status != Status::Ended && tone.starts.lock().unwrap().contains(&0.2));
    assert_eq!(p.state().advances, 0);
}

/// Files play as a short tone; links fail to look up, with YouTube's error for an upload
/// that's there but won't play here. Counts the look-ups.
struct BlockedLinks {
    resolves: Mutex<u32>,
}

const BLOCKED: &str = "Video unavailable. This video is not available";

impl Media for BlockedLinks {
    fn resolve(&self, env: &Env, source: &Source) -> Result<Resolved> {
        if matches!(source, Source::Link(_)) {
            *self.resolves.lock().unwrap() += 1;
            return Err(anyhow!(BLOCKED));
        }
        Tone::new(0.5).resolve(env, source)
    }

    fn decode(&self, env: &Env, input: &Input, start: f64, rate: u32) -> Result<Pcm> {
        Tone::new(0.5).decode(env, input, start, rate)
    }
}

#[test]
fn next_link_that_failed_ahead_of_time_says_so_and_fails_at_once() {
    let capture = Capture::new(RATE, 2.0);
    let media = Arc::new(BlockedLinks { resolves: Mutex::default() });
    let p = player(media.clone(), &capture);
    let url = "https://www.youtube.com/watch?v=blocked";
    p.send(Command::Load(track(), 0.0));
    p.send(Command::SetNext(Some(Source::Link(url.into()))));
    // Told before its turn, so the app can mark the row.
    let s = wait_for(&p, "the look-ahead failure", |s| s.next_failed.is_some());
    assert_eq!(s.next_failed, Some((url.to_owned(), BLOCKED.to_owned())));
    assert_eq!(s.status, Status::Playing, "the current track carries on");
    // Its turn: the same error straight away, without asking yt-dlp again.
    let s = wait_for(&p, "advanced", |s| s.advances == 1 && matches!(s.status, Status::Error(_)));
    assert_eq!(s.status, Status::Error(BLOCKED.into()));
    assert_eq!(*media.resolves.lock().unwrap(), 1);
    // Playing it again does ask.
    p.send(Command::Play);
    wait_for(&p, "asked again", |_| *media.resolves.lock().unwrap() == 2);
}

/// Files play as a long tone; links take a while to look up, like yt-dlp does, and count
/// their look-ups.
struct SlowLookup {
    resolves: Mutex<u32>,
}

impl Media for SlowLookup {
    fn resolve(&self, env: &Env, source: &Source) -> Result<Resolved> {
        if matches!(source, Source::Link(_)) {
            *self.resolves.lock().unwrap() += 1;
            thread::sleep(Duration::from_millis(400));
            return Tone::new(0.5).resolve(env, source);
        }
        Tone::new(5.0).resolve(env, source)
    }

    fn decode(&self, env: &Env, input: &Input, start: f64, rate: u32) -> Result<Pcm> {
        Tone::new(5.0).decode(env, input, start, rate)
    }
}

#[test]
fn skipping_to_a_link_being_looked_up_ahead_waits_for_that_look_up() {
    let capture = Capture::new(RATE, 1.0);
    let media = Arc::new(SlowLookup { resolves: Mutex::default() });
    let p = player(media.clone(), &capture);
    let next = Source::Link("https://youtu.be/b".into());
    p.send(Command::Load(track(), 0.0));
    p.send(Command::SetNext(Some(next.clone())));
    wait_for(&p, "look-up ahead started", |_| *media.resolves.lock().unwrap() == 1);
    // Next pressed while yt-dlp is still busy with it.
    p.send(Command::Load(next.clone(), 0.0));
    let s = wait_for(&p, "playing b", |s| s.status == Status::Playing && s.source.as_ref() == Some(&next));
    assert_eq!(s.track.unwrap().title, "Tone https://youtu.be/b");
    assert_eq!(*media.resolves.lock().unwrap(), 1, "looked up once");
}
