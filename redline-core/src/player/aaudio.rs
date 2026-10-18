//! Android's output: an AAudio stream through the `ndk` crate.
//!
//! Not cpal: its AAudio backend asks `ndk_context` for the app's JVM after opening a stream,
//! and nothing sets that up in a library Kotlin loads through JNA, so it panics. AAudio itself
//! needs no Java at all.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow};
use ndk::audio::{
    AudioCallbackResult, AudioDirection, AudioFormat, AudioPerformanceMode, AudioSharingMode, AudioStream,
    AudioStreamBuilder,
};
use rtrb::{Consumer, Producer, RingBuffer};

use super::output::Output;
use super::render::Renderer;

/// The default output as AAudio routes it (speaker, headphones, Bluetooth).
///
/// Usage and content type stay at AAudio's defaults, which are Media and Music: the setters
/// for them are API 28 and the app runs on 26.
pub struct AAudioOutput {
    stream: AudioStream,
    /// Hands the renderer to the callback without a lock (it runs on a real-time thread).
    handoff: Producer<Renderer>,
    lost: Arc<AtomicBool>,
}

impl AAudioOutput {
    pub fn open() -> Result<Self> {
        let (handoff, mut inbox): (Producer<Renderer>, Consumer<Renderer>) = RingBuffer::new(1);
        let mut renderer: Option<Renderer> = None;
        let lost = Arc::new(AtomicBool::new(false));
        let lost_cb = lost.clone();
        let stream = AudioStreamBuilder::new()
            .map_err(|e| anyhow!("Couldn't open the audio output ({e:?})"))?
            .direction(AudioDirection::Output)
            .format(AudioFormat::PCM_Float)
            .channel_count(2)
            .sharing_mode(AudioSharingMode::Shared)
            // Bigger buffers and fewer wake-ups; a music player doesn't need low latency.
            .performance_mode(AudioPerformanceMode::PowerSaving)
            .data_callback(Box::new(move |stream: &AudioStream, data: *mut c_void, frames: i32| {
                let channels = stream.channel_count().max(1) as usize;
                let len = frames.max(0) as usize * channels;
                // SAFETY: AAudio hands over `frames` frames of the stream's format, which is
                // f32 because the builder asked for it exactly.
                let out = unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), len) };
                if renderer.is_none() {
                    renderer = inbox.pop().ok();
                }
                match renderer.as_mut() {
                    Some(r) => r.render(out, channels),
                    None => out.fill(0.0),
                }
                AudioCallbackResult::Continue
            }))
            // Disconnected (headphones out, Bluetooth gone) or any other failure: the stream is
            // dead, so let the engine open a new one on whatever is the default now.
            .error_callback(Box::new(move |_: &AudioStream, _| lost_cb.store(true, Ordering::Relaxed)))
            .open_stream()
            .map_err(|e| anyhow!("Couldn't open the audio output ({e:?})"))?;
        Ok(Self { stream, handoff, lost })
    }
}

impl Output for AAudioOutput {
    fn sample_rate(&self) -> u32 {
        self.stream.sample_rate().max(1) as u32
    }

    fn start(&mut self, renderer: Renderer) -> Result<()> {
        self.handoff
            .push(renderer)
            .map_err(|_| anyhow!("The audio output was already started"))?;
        self.stream
            .request_start()
            .map_err(|e| anyhow!("Couldn't start the audio output ({e:?})"))
    }

    fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
}

impl Drop for AAudioOutput {
    fn drop(&mut self) {
        // Closing (the stream's own drop) waits for a running callback to return.
        let _ = self.stream.request_stop();
    }
}
