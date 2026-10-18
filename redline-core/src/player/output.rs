//! Where rendered audio goes: the default sound device (cpal on desktops, AAudio on Android),
//! or a capture buffer for tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
#[cfg(not(target_os = "android"))]
use anyhow::{Context, anyhow};
#[cfg(not(target_os = "android"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
#[cfg(not(target_os = "android"))]
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample};

use super::render::Renderer;

/// An opened audio output. Created and dropped on the engine thread.
pub trait Output {
    fn sample_rate(&self) -> u32;
    /// Starts calling the renderer for audio.
    fn start(&mut self, renderer: Renderer) -> Result<()>;
    /// The device went away; the engine drops this output and opens the default again.
    fn lost(&self) -> bool;
}

/// Opens an output; called again after the device is lost.
pub type OpenOutput = Box<dyn FnMut() -> Result<Box<dyn Output>> + Send>;

/// The system's default output device.
#[cfg(not(target_os = "android"))]
pub fn system() -> OpenOutput {
    Box::new(|| Ok(Box::new(CpalOutput::open()?) as Box<dyn Output>))
}

/// The system's default output device.
#[cfg(target_os = "android")]
pub fn system() -> OpenOutput {
    Box::new(|| Ok(Box::new(super::aaudio::AAudioOutput::open()?) as Box<dyn Output>))
}

#[cfg(not(target_os = "android"))]
pub struct CpalOutput {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    stream: Option<cpal::Stream>,
    lost: Arc<AtomicBool>,
}

#[cfg(not(target_os = "android"))]
impl CpalOutput {
    pub fn open() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("No audio output device found"))?;
        let config = device
            .default_output_config()
            .context("The audio device didn't report a usable format")?;
        Ok(Self {
            device,
            config,
            stream: None,
            lost: Arc::default(),
        })
    }

    fn build<T: SizedSample + FromSample<f32>>(&self, mut renderer: Renderer) -> Result<cpal::Stream> {
        let channels = self.config.channels() as usize;
        let lost = self.lost.clone();
        // Scratch buffer for formats other than f32; grown outside the steady state only.
        let mut scratch: Vec<f32> = Vec::new();
        let stream = self.device.build_output_stream::<T, _, _>(
            self.config.config(),
            move |out: &mut [T], _| {
                if scratch.len() < out.len() {
                    scratch.resize(out.len(), 0.0);
                }
                let buf = &mut scratch[..out.len()];
                renderer.render(buf, channels);
                for (o, s) in out.iter_mut().zip(buf.iter()) {
                    *o = T::from_sample(*s);
                }
            },
            move |err: cpal::Error| {
                if matches!(err.kind(), ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated) {
                    lost.store(true, Ordering::Relaxed);
                }
            },
            None,
        )?;
        Ok(stream)
    }
}

#[cfg(not(target_os = "android"))]
impl Output for CpalOutput {
    fn sample_rate(&self) -> u32 {
        self.config.sample_rate()
    }

    fn start(&mut self, renderer: Renderer) -> Result<()> {
        let stream = match self.config.sample_format() {
            SampleFormat::F32 => self.build::<f32>(renderer),
            SampleFormat::I16 => self.build::<i16>(renderer),
            SampleFormat::I32 => self.build::<i32>(renderer),
            SampleFormat::U16 => self.build::<u16>(renderer),
            SampleFormat::F64 => self.build::<f64>(renderer),
            f => Err(anyhow!("The audio device uses an unsupported sample format ({f})")),
        }
        .context("Couldn't open the audio device")?;
        stream.play().context("Couldn't start the audio device")?;
        self.stream = Some(stream);
        Ok(())
    }

    fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
}

/// Collects rendered audio in memory instead of playing it.
#[derive(Clone)]
pub struct Capture {
    pub samples: Arc<Mutex<Vec<f32>>>,
    pub rate: u32,
    /// How many times faster than real time to pull audio.
    pub speedup: f32,
    /// Set to simulate the device going away.
    pub lose: Arc<AtomicBool>,
}

impl Capture {
    pub fn new(rate: u32, speedup: f32) -> Self {
        Self {
            samples: Arc::default(),
            rate,
            speedup,
            lose: Arc::default(),
        }
    }

    /// An [`OpenOutput`] that renders into this capture.
    pub fn opener(&self) -> OpenOutput {
        let c = self.clone();
        Box::new(move || {
            c.lose.store(false, Ordering::Relaxed);
            Ok(Box::new(CaptureOutput {
                capture: c.clone(),
                stop: Arc::default(),
            }) as Box<dyn Output>)
        })
    }

    /// Captured stereo frames so far.
    pub fn frames(&self) -> usize {
        self.samples.lock().unwrap().len() / 2
    }
}

struct CaptureOutput {
    capture: Capture,
    stop: Arc<AtomicBool>,
}

/// Device period the capture pretends to have.
const PERIOD_FRAMES: usize = 480;

impl Output for CaptureOutput {
    fn sample_rate(&self) -> u32 {
        self.capture.rate
    }

    fn start(&mut self, mut renderer: Renderer) -> Result<()> {
        let c = self.capture.clone();
        let stop = self.stop.clone();
        thread::spawn(move || {
            let period = Duration::from_secs_f64(PERIOD_FRAMES as f64 / c.rate as f64 / c.speedup as f64);
            let mut buf = vec![0.0f32; PERIOD_FRAMES * 2];
            let start = Instant::now();
            let mut n = 0u32;
            while !stop.load(Ordering::Relaxed) {
                renderer.render(&mut buf, 2);
                c.samples.lock().unwrap().extend_from_slice(&buf);
                n += 1;
                let due = start + period * n;
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    thread::sleep(wait);
                }
            }
        });
        Ok(())
    }

    fn lost(&self) -> bool {
        self.capture.lose.load(Ordering::Relaxed)
    }
}

impl Drop for CaptureOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
