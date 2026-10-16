//! The audio callback side of the player: pulls decoded samples from the current stream's
//! ring buffer and writes the device's buffers. Nothing here blocks or locks; the engine talks
//! to it through atomics and a small message queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

/// An `f32` that can be shared without a lock.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// State shared between the engine thread and the audio callback.
pub struct Shared {
    pub playing: AtomicBool,
    /// 0..=1, as set by the user; the callback applies a curve.
    pub volume: AtomicF32,
    /// Id of the stream the callback is playing, and how many source frames it has used.
    pub stream_id: AtomicU64,
    pub frames: AtomicU64,
    /// Highest stream id that has had samples ready to play.
    pub ready_id: AtomicU64,
    /// Highest stream id that ended and was played to the last sample.
    pub drained_id: AtomicU64,
    /// The stream ran dry before its end: waiting on the decoder or the network.
    pub underrun: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            playing: AtomicBool::new(false),
            volume: AtomicF32::new(1.0),
            stream_id: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            ready_id: AtomicU64::new(0),
            drained_id: AtomicU64::new(0),
            underrun: AtomicBool::new(false),
        }
    }
}

/// Decoded audio for one load or seek: interleaved stereo samples at the device rate.
pub struct Stream {
    pub id: u64,
    pub pcm: Consumer<f32>,
    /// Set by the reader once the decoder has nothing more to give.
    pub eof: Arc<AtomicBool>,
}

pub enum RenderMsg {
    Play(Stream),
    /// Drop the current stream and go silent.
    Clear,
}

/// Seconds of decoded audio buffered ahead of the device.
pub const BUFFER_SECS: usize = 2;

/// A ring buffer sized for [`BUFFER_SECS`] of stereo audio at `rate`.
pub fn stream_buffer(rate: u32) -> (Producer<f32>, Consumer<f32>) {
    RingBuffer::new(rate as usize * 2 * BUFFER_SECS)
}

/// Gain ramp length for pause, resume and volume changes, so they don't click.
const RAMP_SECS: f32 = 0.008;

pub struct Renderer {
    shared: Arc<Shared>,
    inbox: Consumer<RenderMsg>,
    stream: Option<Stream>,
    frames: u64,
    /// 0..=1 fade for pause/resume.
    play_gain: f32,
    volume_gain: f32,
    /// Per-sample step for the ramps.
    ramp_step: f32,
}

impl Renderer {
    /// A renderer for a device running at `rate`, and the queue for handing it streams.
    pub fn new(shared: Arc<Shared>, rate: u32) -> (Producer<RenderMsg>, Self) {
        let (tx, inbox) = RingBuffer::new(16);
        let volume_gain = volume_curve(shared.volume.get());
        (
            tx,
            Self {
                shared,
                inbox,
                stream: None,
                frames: 0,
                play_gain: 0.0,
                volume_gain,
                ramp_step: 1.0 / (RAMP_SECS * rate.max(1) as f32),
            },
        )
    }

    fn take_messages(&mut self) {
        while let Ok(msg) = self.inbox.pop() {
            match msg {
                RenderMsg::Play(s) => {
                    self.shared.stream_id.store(s.id, Ordering::Relaxed);
                    self.frames = 0;
                    self.shared.frames.store(0, Ordering::Relaxed);
                    self.stream = Some(s);
                }
                RenderMsg::Clear => self.stream = None,
            }
        }
    }

    /// Fills `out` (interleaved, `channels` per frame) with the next audio.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        self.take_messages();
        let channels = channels.max(1);
        let playing = self.shared.playing.load(Ordering::Relaxed);
        let target_volume = volume_curve(self.shared.volume.get());
        let mut underrun = false;

        if let Some(s) = &self.stream
            && s.pcm.slots() > 0
        {
            self.shared.ready_id.fetch_max(s.id, Ordering::Relaxed);
        }

        for frame in out.chunks_mut(channels) {
            let play_target = if playing { 1.0 } else { 0.0 };
            self.play_gain = approach(self.play_gain, play_target, self.ramp_step);
            self.volume_gain = approach(self.volume_gain, target_volume, self.ramp_step);

            // Only pull while audible, so pausing holds the position.
            let (l, r) = match &mut self.stream {
                Some(s) if self.play_gain > 0.0 => {
                    if s.pcm.slots() >= 2 {
                        let l = s.pcm.pop().unwrap_or(0.0);
                        let r = s.pcm.pop().unwrap_or(0.0);
                        self.frames += 1;
                        (l, r)
                    } else {
                        if s.eof.load(Ordering::Acquire) {
                            self.shared.drained_id.fetch_max(s.id, Ordering::Relaxed);
                        } else {
                            underrun = true;
                        }
                        (0.0, 0.0)
                    }
                }
                _ => (0.0, 0.0),
            };
            let g = self.play_gain * self.volume_gain;
            write_frame(frame, l * g, r * g);
        }
        // A stream that ended while paused still counts as played out once it's empty.
        if let Some(s) = &self.stream
            && s.eof.load(Ordering::Acquire)
            && s.pcm.slots() < 2
        {
            self.shared.drained_id.fetch_max(s.id, Ordering::Relaxed);
        }
        self.shared.frames.store(self.frames, Ordering::Relaxed);
        self.shared.underrun.store(underrun, Ordering::Relaxed);
    }
}

/// Maps the volume slider to gain; squared feels closer to even steps than linear.
fn volume_curve(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    v * v
}

fn approach(current: f32, target: f32, step: f32) -> f32 {
    if (target - current).abs() <= step {
        target
    } else if target > current {
        current + step
    } else {
        current - step
    }
}

/// Writes a stereo frame to a device frame of any width.
fn write_frame(frame: &mut [f32], l: f32, r: f32) {
    match frame {
        [m] => *m = 0.5 * (l + r),
        [a, b, rest @ ..] => {
            *a = l;
            *b = r;
            rest.fill(0.0);
        }
        [] => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(rate: u32) -> (Arc<Shared>, Producer<RenderMsg>, Renderer) {
        let shared = Arc::new(Shared::default());
        let (tx, r) = Renderer::new(shared.clone(), rate);
        (shared, tx, r)
    }

    #[test]
    fn plays_a_stream_then_reports_it_drained() {
        let (shared, mut tx, mut r) = setup(1000);
        let (mut p, c) = RingBuffer::new(64);
        for i in 0..20 {
            p.push(i as f32 / 100.0).unwrap();
        }
        let eof = Arc::new(AtomicBool::new(false));
        tx.push(RenderMsg::Play(Stream { id: 7, pcm: c, eof: eof.clone() })).unwrap();

        // Paused: nothing is consumed, but the stream is ready.
        let mut out = [1.0f32; 8];
        r.render(&mut out, 2);
        assert_eq!(out, [0.0; 8]);
        assert_eq!(shared.ready_id.load(Ordering::Relaxed), 7);
        assert_eq!(shared.frames.load(Ordering::Relaxed), 0);

        shared.playing.store(true, Ordering::Relaxed);
        let mut out = [0.0f32; 40];
        r.render(&mut out, 2);
        assert_eq!(shared.frames.load(Ordering::Relaxed), 10, "all 10 frames used");
        assert!(shared.underrun.load(Ordering::Relaxed), "ran dry before eof");
        assert_eq!(shared.drained_id.load(Ordering::Relaxed), 0);

        eof.store(true, Ordering::Release);
        r.render(&mut out, 2);
        assert!(!shared.underrun.load(Ordering::Relaxed));
        assert_eq!(shared.drained_id.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn maps_stereo_to_mono_and_surround() {
        let mut mono = [0.0];
        write_frame(&mut mono, 1.0, 0.0);
        assert_eq!(mono, [0.5]);
        let mut six = [9.0; 6];
        write_frame(&mut six, 0.25, -0.25);
        assert_eq!(six, [0.25, -0.25, 0.0, 0.0, 0.0, 0.0]);
    }
}
