//! The current link's decoded audio, kept on disk so looping, seeking back and playing it
//! again read it from there instead of opening the stream URL again.
//!
//! One decoder (the filler) at a time decodes the track as fast as it can into a file, and
//! each playback stream reads from that file at its own position. A stream that reaches audio
//! nobody has decoded yet waits for the filler if it's nearly there, or starts a new filler at
//! its own position (which replaces the old one). The file goes when the cache is dropped.

use std::fs::File;
use std::io::Read;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};

use super::Source;
use super::decode::Pcm;

/// Starts decoding the track from these seconds.
pub type Decode = Arc<dyn Fn(f64) -> Result<Pcm> + Send + Sync>;

/// Tracks longer than this aren't kept (an hour of audio is about 700 MB on disk).
pub const MAX_SECS: f64 = 30.0 * 60.0;

/// A stream this close behind the filler waits for it instead of starting another.
const CATCH_UP_SECS: u64 = 4;

/// Bytes per stereo frame on disk: two `i16`s.
const FRAME_BYTES: u64 = 4;

/// Samples are stored as `i16` over ±2, so decoder overshoot past full scale survives.
const SCALE: f32 = 16383.0;

pub struct TrackCache {
    pub source: Source,
    pub rate: u32,
    inner: Arc<Inner>,
}

struct Inner {
    file: File,
    rate: u32,
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// Decoded frames: sorted, apart and not touching.
    have: Vec<Range<u64>>,
    /// The track's length in frames, once a filler reached its end.
    total: Option<u64>,
    filler: Option<Filler>,
    /// The last filler that failed, and why.
    failed: Option<(u64, String)>,
    /// Writing to the file failed, so the cache can't be trusted to grow.
    broken: bool,
    closed: bool,
    next_gen: u64,
}

struct Filler {
    generation: u64,
    /// The frame it started from, and the next one it will write.
    from: u64,
    at: u64,
    kill: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl TrackCache {
    /// An empty cache for `source` decoded at `rate`, in a file under `dir`. None if the file
    /// can't be made.
    pub fn new(source: Source, rate: u32, dir: &Path) -> Option<Self> {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("redline-play-{}-{n}.pcm", std::process::id()));
        let file = create_scratch(&path).ok()?;
        Some(Self {
            source,
            rate,
            inner: Arc::new(Inner {
                file,
                rate,
                state: Mutex::default(),
                changed: Condvar::new(),
            }),
        })
    }

    pub fn is_broken(&self) -> bool {
        self.inner.state.lock().unwrap().broken
    }

    /// Playback from `start` seconds. It decodes with `decode` whatever isn't kept yet.
    pub fn reader(&self, start: f64, decode: Decode) -> Pcm {
        let stop = Arc::new(AtomicBool::new(false));
        let error: Arc<Mutex<Option<String>>> = Arc::default();
        let reader = Reader {
            inner: self.inner.clone(),
            at: (start.max(0.0) * self.rate as f64).round() as u64 * 2,
            decode,
            mine: None,
            stop: stop.clone(),
            error: error.clone(),
        };
        let inner = self.inner.clone();
        Pcm {
            reader: Box::new(reader),
            kill: Box::new(move || {
                stop.store(true, Ordering::Relaxed);
                // Taken so a waiting read sees the flag.
                let _guard = inner.state.lock().unwrap();
                inner.changed.notify_all();
            }),
            finish: Box::new(move || match error.lock().unwrap().take() {
                Some(e) => Err(anyhow!(e)),
                None => Ok(()),
            }),
        }
    }
}

impl Drop for TrackCache {
    fn drop(&mut self) {
        let kill = {
            let mut st = self.inner.state.lock().unwrap();
            st.closed = true;
            st.filler.take().and_then(|f| f.kill)
        };
        self.inner.changed.notify_all();
        if let Some(kill) = kill {
            thread::spawn(move || kill());
        }
    }
}

impl State {
    fn covering(&self, frame: u64) -> Option<&Range<u64>> {
        self.have.iter().find(|r| r.contains(&frame))
    }

    fn add(&mut self, new: Range<u64>) {
        if new.is_empty() {
            return;
        }
        let (mut start, mut end) = (new.start, new.end);
        self.have.retain(|r| {
            let touches = r.start <= end && start <= r.end;
            if touches {
                start = start.min(r.start);
                end = end.max(r.end);
            }
            !touches
        });
        let at = self.have.partition_point(|r| r.start < start);
        self.have.insert(at, start..end);
    }
}

impl Inner {
    /// Replaces the filler with one from `frame` (or a little before). Returns its generation.
    fn start_filler(self: &Arc<Self>, frame: u64, decode: &Decode) -> u64 {
        // Decoders start on a whole millisecond; the frames before `frame` are the same audio.
        let secs = (frame as f64 * 1000.0 / self.rate as f64).floor() / 1000.0;
        let first = ((secs * self.rate as f64).round() as u64).min(frame);
        let (generation, old) = {
            let mut st = self.state.lock().unwrap();
            st.next_gen += 1;
            let generation = st.next_gen;
            let old = st.filler.replace(Filler { generation, from: first, at: first, kill: None });
            (generation, old.and_then(|f| f.kill))
        };
        if let Some(kill) = old {
            thread::spawn(move || kill());
        }
        match decode(secs) {
            Ok(pcm) => {
                let kill: Arc<dyn Fn() + Send + Sync> = Arc::from(pcm.kill);
                let current = {
                    let mut st = self.state.lock().unwrap();
                    let closed = st.closed;
                    match st.filler.as_mut().filter(|f| f.generation == generation) {
                        Some(f) if !closed => {
                            f.kill = Some(kill.clone());
                            true
                        }
                        _ => false,
                    }
                };
                if !current {
                    kill();
                }
                let inner = self.clone();
                thread::spawn(move || inner.fill(generation, first, pcm.reader, pcm.finish));
            }
            Err(e) => self.filler_ended(generation, Err(format!("{e:#}"))),
        }
        generation
    }

    /// The filler's thread: copies decoded audio into the file until the decoder ends or
    /// another filler takes over.
    fn fill(&self, generation: u64, mut at: u64, mut reader: Box<dyn Read + Send>, finish: Box<dyn FnOnce() -> Result<()> + Send>) {
        let mut buf = vec![0u8; 64 * 1024];
        let mut carry = 0;
        let mut ended: Result<(), String> = Ok(());
        loop {
            let n = match reader.read(&mut buf[carry..]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    ended = Err(e.to_string());
                    break;
                }
            };
            let filled = carry + n;
            let whole = filled / 8 * 8;
            let frames = whole as u64 / 8;
            let disk: Vec<u8> = buf[..whole]
                .chunks_exact(4)
                .flat_map(|b| to_disk(f32::from_le_bytes([b[0], b[1], b[2], b[3]])).to_le_bytes())
                .collect();
            if let Err(e) = write_at(&self.file, &disk, at * FRAME_BYTES) {
                self.state.lock().unwrap().broken = true;
                ended = Err(format!("can't keep the stream's audio: {e}"));
                break;
            }
            {
                let mut st = self.state.lock().unwrap();
                let closed = st.closed;
                let Some(f) = st.filler.as_mut().filter(|f| f.generation == generation && !closed) else {
                    // Taken over: this decoder was killed, and only needs reaping.
                    drop(st);
                    let _ = finish();
                    return;
                };
                f.at = at + frames;
                st.add(at..at + frames);
            }
            self.changed.notify_all();
            at += frames;
            buf.copy_within(whole..filled, 0);
            carry = filled - whole;
        }
        let result = finish().map_err(|e| format!("{e:#}")).and(ended);
        self.filler_ended(generation, result.map(|()| at));
    }

    /// Records how filler `generation` ended: at the track's end (in frames), or failed.
    fn filler_ended(&self, generation: u64, result: Result<u64, String>) {
        {
            let mut st = self.state.lock().unwrap();
            if !st.filler.as_ref().is_some_and(|f| f.generation == generation) {
                return;
            }
            st.filler = None;
            match result {
                Ok(end) => st.total = Some(end),
                Err(e) => st.failed = Some((generation, e)),
            }
        }
        self.changed.notify_all();
    }
}

/// One playback stream's view of the cache.
struct Reader {
    inner: Arc<Inner>,
    /// The next sample to hand out (two per frame).
    at: u64,
    decode: Decode,
    /// The filler this reader is waiting on, so its failure ends the stream.
    mine: Option<u64>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

impl Read for Reader {
    /// Hands out whole samples as `f32le`, waiting for them to be decoded.
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let inner = self.inner.clone();
        let catch_up = CATCH_UP_SECS * inner.rate as u64;
        let mut st = inner.state.lock().unwrap();
        loop {
            if self.stop.load(Ordering::Relaxed) || st.closed {
                return Ok(0);
            }
            let frame = self.at / 2;
            if let Some(end) = st.covering(frame).map(|r| r.end) {
                let samples = (end * 2 - self.at).min(out.len() as u64 / 4);
                drop(st);
                let mut disk = vec![0u8; samples as usize * 2];
                read_at(&inner.file, &mut disk, self.at * 2)?;
                for (o, d) in out.chunks_exact_mut(4).zip(disk.chunks_exact(2)) {
                    o.copy_from_slice(&from_disk(i16::from_le_bytes([d[0], d[1]])).to_le_bytes());
                }
                self.at += samples;
                return Ok(samples as usize * 4);
            }
            if st.total.is_some_and(|t| frame >= t) {
                return Ok(0);
            }
            let nearly_there = st
                .filler
                .as_ref()
                .filter(|f| f.from <= frame && frame <= f.at + catch_up)
                .map(|f| f.generation);
            if nearly_there.is_some() {
                self.mine = nearly_there;
                st = inner.changed.wait_timeout(st, Duration::from_millis(100)).unwrap().0;
                continue;
            }
            if let Some((_, e)) = st.failed.as_ref().filter(|(g, _)| Some(*g) == self.mine) {
                *self.error.lock().unwrap() = Some(e.clone());
                return Ok(0);
            }
            drop(st);
            self.mine = Some(inner.start_filler(frame, &self.decode));
            st = inner.state.lock().unwrap();
        }
    }
}

fn to_disk(s: f32) -> i16 {
    (s * SCALE).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

fn from_disk(s: i16) -> f32 {
    s as f32 / SCALE
}

/// A file that's gone from the folder once it's closed, even if the app is killed.
#[cfg(unix)]
fn create_scratch(path: &Path) -> std::io::Result<File> {
    let file = File::options().read(true).write(true).create_new(true).open(path)?;
    // Open files outlive their name.
    let _ = std::fs::remove_file(path);
    Ok(file)
}

#[cfg(windows)]
fn create_scratch(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(path)
}

#[cfg(unix)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(windows)]
fn write_at(file: &File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(windows)]
fn read_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset)? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_ranges_merge() {
        let mut st = State::default();
        st.add(10..20);
        st.add(30..40);
        st.add(0..5);
        assert_eq!(st.have, vec![0..5, 10..20, 30..40]);
        st.add(18..30);
        assert_eq!(st.have, vec![0..5, 10..40]);
        st.add(5..10);
        assert_eq!(st.have, vec![0..40]);
        assert!(st.covering(39).is_some() && st.covering(40).is_none());
    }

    /// Frame `i` of the fake track holds `i / 10000` on both channels.
    fn sample(frame: u64) -> f32 {
        frame as f32 / 10_000.0
    }

    /// Hands out its bytes a few frames at a time, like a decoder on a slow network.
    struct Trickle(std::io::Cursor<Vec<u8>>);

    impl Read for Trickle {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            thread::sleep(Duration::from_millis(2));
            let n = out.len().min(8 * 50 + 3);
            self.0.read(&mut out[..n])
        }
    }

    /// A 10 s track at 1000 Hz, recording where each decoder started.
    fn track(starts: Arc<Mutex<Vec<f64>>>) -> Decode {
        Arc::new(move |start| {
            starts.lock().unwrap().push(start);
            let first = (start * RATE as f64).round() as u64;
            let bytes = (first..10 * RATE as u64)
                .flat_map(|i| [sample(i), sample(i)])
                .flat_map(f32::to_le_bytes)
                .collect();
            Ok(Pcm {
                reader: Box::new(Trickle(std::io::Cursor::new(bytes))),
                kill: Box::new(|| {}),
                finish: Box::new(|| Ok(())),
            })
        })
    }

    const RATE: u32 = 1000;

    fn read_frames(pcm: &mut Pcm, frames: usize) -> Vec<f32> {
        let mut bytes = vec![0u8; frames * 8];
        pcm.reader.read_exact(&mut bytes).unwrap();
        bytes.chunks_exact(8).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }

    fn assert_frames(got: &[f32], first: u64) {
        for (i, s) in got.iter().enumerate() {
            let want = sample(first + i as u64);
            assert!((s - want).abs() < 1e-3, "frame {}: {s} != {want}", first + i as u64);
        }
    }

    #[test]
    fn streams_read_the_kept_audio_and_restart_the_decoder_only_past_it() {
        let starts: Arc<Mutex<Vec<f64>>> = Arc::default();
        let decode = track(starts.clone());
        let cache = TrackCache::new(Source::Link("x".into()), RATE, &std::env::temp_dir()).unwrap();

        // Waits for the decoder as it goes.
        let mut first = cache.reader(0.0, decode.clone());
        assert_frames(&read_frames(&mut first, 1500), 0);
        (first.kill)();

        // Back to the start: already there.
        let mut again = cache.reader(0.2, decode.clone());
        assert_frames(&read_frames(&mut again, 1000), 200);
        (again.kill)();
        assert_eq!(*starts.lock().unwrap(), vec![0.0]);

        // Far past the decoder: a new one there.
        let mut far = cache.reader(8.5, decode.clone());
        assert_frames(&read_frames(&mut far, 1500), 8500);
        let mut rest = Vec::new();
        far.reader.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty(), "ended with the track");
        assert!((far.finish)().is_ok());
        assert_eq!(*starts.lock().unwrap(), vec![0.0, 8.5]);

        // Into the gap between the two: decoded from there, and the rest is kept.
        let mut gap = cache.reader(5.0, decode.clone());
        let mut all = Vec::new();
        gap.reader.read_to_end(&mut all).unwrap();
        let got: Vec<f32> = all.chunks_exact(8).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        assert_eq!(got.len(), 5000);
        assert_frames(&got, 5000);
        assert_eq!(starts.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_failed_decoder_ends_the_streams_waiting_on_it() {
        let cache = TrackCache::new(Source::Link("x".into()), RATE, &std::env::temp_dir()).unwrap();
        let decode: Decode = Arc::new(|_| Err(anyhow!("Server returned 403 Forbidden")));
        let mut pcm = cache.reader(1.0, decode);
        assert_eq!(pcm.reader.read(&mut [0u8; 64]).unwrap(), 0);
        assert!((pcm.finish)().unwrap_err().to_string().contains("403"));
    }

    #[test]
    fn samples_survive_the_disk() {
        for s in [0.0, 0.5, -1.0, 1.3, -1.9] {
            assert!((from_disk(to_disk(s)) - s).abs() < 1e-4, "{s}");
        }
        assert_eq!(from_disk(to_disk(5.0)), i16::MAX as f32 / SCALE, "clamped");
    }
}
