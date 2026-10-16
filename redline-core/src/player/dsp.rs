//! Real-time processing in the audio callback: variable-rate resampling for speed (pitch
//! follows, like a turntable), a 10-band peaking EQ and a soft clipper.

use serde::{Deserialize, Serialize};

/// Playback speed limits.
pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.0;

/// Plays a stream of stereo frames back at a variable rate with cubic (Catmull-Rom)
/// interpolation. At 1× it passes frames through untouched.
pub struct Resampler {
    /// The four frames around the read point: x[-1], x[0], x[1], x[2].
    hist: [[f32; 2]; 4],
    /// Read point between hist[1] and hist[2], 0..1. At 1 or more a new frame is due.
    phase: f64,
    /// Current speed, eased toward the target so changes don't jump.
    speed: f64,
    /// Per-frame easing factor for speed.
    ease: f64,
}

impl Resampler {
    pub fn new(rate: u32) -> Self {
        Self {
            hist: [[0.0; 2]; 4],
            phase: 1.0,
            speed: 1.0,
            // About 30 ms to settle on a new speed.
            ease: 1.0 - (-1.0 / (0.03 * rate.max(1) as f64)).exp(),
        }
    }

    /// Forgets the old stream, e.g. after a seek. Keeps the speed.
    pub fn reset(&mut self) {
        self.hist = [[0.0; 2]; 4];
        self.phase = 1.0;
    }

    /// The next output frame. `pull` gives the next source frame, or `None` when there's
    /// nothing buffered; then this returns `None` and keeps its place.
    pub fn next(&mut self, target_speed: f32, mut pull: impl FnMut() -> Option<[f32; 2]>) -> Option<[f32; 2]> {
        while self.phase >= 1.0 {
            let frame = pull()?;
            self.hist = [self.hist[1], self.hist[2], self.hist[3], frame];
            self.phase -= 1.0;
        }
        let t = self.phase as f32;
        let h = &self.hist;
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            *o = catmull_rom(h[0][c], h[1][c], h[2][c], h[3][c], t);
        }
        let target = target_speed.clamp(MIN_SPEED, MAX_SPEED) as f64;
        self.speed += (target - self.speed) * self.ease;
        if (self.speed - target).abs() < 1e-6 {
            self.speed = target;
        }
        self.phase += self.speed;
        Some(out)
    }
}

/// Interpolates between `x1` (t = 0) and `x2` (t = 1).
fn catmull_rom(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let a = -0.5 * x0 + 1.5 * x1 - 1.5 * x2 + 0.5 * x3;
    let b = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
    let c = -0.5 * x0 + 0.5 * x2;
    ((a * t + b) * t + c) * t + x1
}

/// Band centre frequencies.
pub const EQ_FREQS: [f32; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
pub const BAND_DB: f32 = 12.0;
pub const PREAMP_MIN_DB: f32 = -12.0;
pub const PREAMP_MAX_DB: f32 = 6.0;
/// Q of each band; about an octave wide, so neighbours overlap smoothly.
pub const EQ_Q: f32 = 1.41;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct EqSettings {
    pub enabled: bool,
    pub preamp_db: f32,
    pub bands_db: [f32; 10],
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            preamp_db: 0.0,
            bands_db: [0.0; 10],
        }
    }
}

/// Named curves for the EQ panel: (name, preamp, bands).
pub const EQ_PRESETS: &[(&str, f32, [f32; 10])] = &[
    ("Flat", 0.0, [0.0; 10]),
    ("Bass boost", -3.0, [6.0, 5.0, 4.0, 2.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble boost", -3.0, [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.5, 4.0, 5.0, 6.0]),
    ("Vocal", -2.0, [-2.0, -2.0, -1.0, 0.0, 2.0, 3.5, 3.5, 2.0, 0.0, -1.0]),
    ("Loudness", -3.0, [6.0, 4.5, 2.0, 0.0, -1.0, -1.0, 0.0, 1.5, 3.5, 5.0]),
];

impl EqSettings {
    /// The preset these settings match, if any.
    pub fn preset(&self) -> Option<&'static str> {
        EQ_PRESETS
            .iter()
            .find(|(_, pre, bands)| *pre == self.preamp_db && *bands == self.bands_db)
            .map(|(name, ..)| *name)
    }
}

/// Normalised biquad coefficients (a0 = 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Coeffs {
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// RBJ cookbook peaking EQ.
    pub fn peaking(freq: f32, gain_db: f32, q: f32, rate: f32) -> Self {
        if gain_db == 0.0 || freq >= rate * 0.45 {
            return Self::IDENTITY;
        }
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * freq / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: -2.0 * cos / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }

    /// Gain in dB at `freq`, from the transfer function.
    pub fn response_db(&self, freq: f32, rate: f32) -> f32 {
        let w = std::f64::consts::TAU * freq as f64 / rate as f64;
        let z1 = (w.cos(), -w.sin());
        let z2 = ((2.0 * w).cos(), -(2.0 * w).sin());
        let c = |k0: f32, k1: f32, k2: f32| {
            (k0 as f64 + k1 as f64 * z1.0 + k2 as f64 * z2.0, k1 as f64 * z1.1 + k2 as f64 * z2.1)
        };
        let num = c(self.b0, self.b1, self.b2);
        let den = c(1.0, self.a1, self.a2);
        let mag = (num.0.hypot(num.1)) / (den.0.hypot(den.1));
        (20.0 * mag.log10()) as f32
    }
}

/// Transposed direct form II state for one channel.
#[derive(Clone, Copy, Default)]
struct BiquadState {
    s1: f32,
    s2: f32,
}

impl BiquadState {
    fn process(&mut self, c: &Coeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }
}

/// Frames between parameter updates.
const EQ_BLOCK: u32 = 32;

pub struct Equalizer {
    rate: f32,
    /// Gains being played (dB), eased toward the targets a block at a time.
    current: [f32; 10],
    preamp: f32,
    coeffs: [Coeffs; 10],
    state: [[BiquadState; 2]; 10],
    /// Easing per block.
    ease: f32,
    countdown: u32,
}

impl Equalizer {
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1) as f32;
        Self {
            rate,
            current: [0.0; 10],
            preamp: 0.0,
            coeffs: [Coeffs::IDENTITY; 10],
            state: Default::default(),
            // About 30 ms to settle, so a slider drag doesn't zip.
            ease: 1.0 - (-(EQ_BLOCK as f32) / (0.03 * rate)).exp(),
            countdown: 0,
        }
    }

    /// Moves toward `target` (bypassed means everything at 0 dB) once per block, recomputing
    /// only the bands that are still moving.
    fn update(&mut self, target: &EqSettings) {
        let (bands, pre) = if target.enabled {
            (target.bands_db, target.preamp_db)
        } else {
            ([0.0; 10], 0.0)
        };
        for (i, want) in bands.iter().enumerate() {
            let want = want.clamp(-BAND_DB, BAND_DB);
            let cur = self.current[i];
            if cur == want {
                continue;
            }
            let next = if (want - cur).abs() < 0.01 { want } else { cur + (want - cur) * self.ease };
            self.current[i] = next;
            self.coeffs[i] = Coeffs::peaking(EQ_FREQS[i], next, EQ_Q, self.rate);
        }
        let pre = pre.clamp(PREAMP_MIN_DB, PREAMP_MAX_DB);
        self.preamp = if (pre - self.preamp).abs() < 0.01 { pre } else { self.preamp + (pre - self.preamp) * self.ease };
    }

    /// Flat with every band at rest: nothing to do.
    fn is_flat(&self) -> bool {
        self.preamp == 0.0 && self.current.iter().all(|g| *g == 0.0)
    }

    pub fn process(&mut self, target: &EqSettings, frame: [f32; 2]) -> [f32; 2] {
        if self.countdown == 0 {
            self.update(target);
            self.countdown = EQ_BLOCK;
        }
        self.countdown -= 1;
        if self.is_flat() {
            return frame;
        }
        let pre = 10f32.powf(self.preamp / 20.0);
        let mut out = [frame[0] * pre, frame[1] * pre];
        for (band, c) in self.coeffs.iter().enumerate() {
            if *c == Coeffs::IDENTITY {
                continue;
            }
            for (ch, x) in out.iter_mut().enumerate() {
                *x = self.state[band][ch].process(c, *x);
            }
        }
        out
    }
}

/// Leaves quiet signals alone and bends peaks over 0.9 smoothly toward ±1.
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let over = (a - KNEE) / (1.0 - KNEE);
        x.signum() * (KNEE + (1.0 - KNEE) * over.tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(freq: f32, frames: usize) -> Vec<[f32; 2]> {
        (0..frames)
            .map(|i| {
                let v = 0.5 * (i as f32 * freq * std::f32::consts::TAU / RATE as f32).sin();
                [v, v]
            })
            .collect()
    }

    /// Runs `input` through a resampler at `speed`, from the start.
    fn resample(input: &[[f32; 2]], speed: impl Fn(usize) -> f32) -> Vec<[f32; 2]> {
        let mut r = Resampler::new(RATE);
        r.speed = speed(0) as f64;
        let mut it = input.iter().copied();
        let mut out = Vec::new();
        while let Some(f) = r.next(speed(out.len()), || it.next()) {
            out.push(f);
        }
        out
    }

    /// Power of `freq` in `x` (Goertzel), as a fraction of the total.
    fn share_at(x: &[f32], freq: f32) -> f32 {
        let w = std::f32::consts::TAU * freq / RATE as f32;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for v in x {
            let s = v + coeff * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
        let total: f32 = x.iter().map(|v| v * v).sum::<f32>() * x.len() as f32 / 2.0;
        power / total
    }

    #[test]
    fn passes_through_untouched_at_1x() {
        let input = sine(440.0, 1000);
        let out = resample(&input, |_| 1.0);
        // One frame out per frame in, two frames behind for the interpolation's lookahead.
        assert_eq!(out.len(), input.len());
        assert_eq!(out[..2], [[0.0; 2]; 2]);
        assert_eq!(&out[2..], &input[..input.len() - 2]);
    }

    #[test]
    fn double_speed_halves_length_and_doubles_pitch() {
        let input = sine(1000.0, RATE as usize);
        let out = resample(&input, |_| 2.0);
        let expected = input.len() / 2;
        assert!(out.len().abs_diff(expected) <= 2, "{} frames, wanted {expected}", out.len());
        let left: Vec<f32> = out.iter().skip(100).map(|f| f[0]).collect();
        let at_2k = share_at(&left, 2000.0);
        let at_1k = share_at(&left, 1000.0);
        assert!(at_2k > 0.95, "2 kHz share {at_2k}");
        assert!(at_1k < 0.01, "1 kHz share {at_1k}");

        let slow = resample(&input, |_| 0.5);
        assert!(slow.len().abs_diff(input.len() * 2) <= 4);
    }

    #[test]
    fn speed_changes_are_click_free() {
        let input = sine(440.0, RATE as usize);
        // Jump from 1× to 2× and back down to 0.5× mid-tone.
        let out = resample(&input, |n| match n {
            0..4000 => 1.0,
            4000..12000 => 2.0,
            _ => 0.5,
        });
        // The steepest a 0.5-amplitude tone at 880 Hz (440 at 2×) can move per frame.
        let limit = 0.5 * std::f32::consts::TAU * 880.0 / RATE as f32 * 1.1;
        let worst = out.windows(2).map(|w| (w[1][0] - w[0][0]).abs()).fold(0.0, f32::max);
        assert!(worst <= limit, "jump of {worst} > {limit}");
    }

    #[test]
    fn peaking_band_hits_its_gain_at_the_centre() {
        for (i, f) in EQ_FREQS.iter().enumerate() {
            for gain in [-12.0, -6.0, 3.0, 12.0] {
                let c = Coeffs::peaking(*f, gain, EQ_Q, RATE as f32);
                let at = c.response_db(*f, RATE as f32);
                assert!((at - gain).abs() < 0.5, "band {i} {f} Hz {gain} dB gave {at}");
                // An octave and a half away it's much less.
                let far = c.response_db((f * 2.8).min(20_000.0), RATE as f32);
                assert!(far.abs() < gain.abs() * 0.6, "band {f} Hz leaks {far} dB");
            }
        }
    }

    #[test]
    fn bypassed_eq_is_flat_and_bit_exact() {
        let mut eq = Equalizer::new(RATE);
        let boosted = EqSettings {
            enabled: false,
            preamp_db: 6.0,
            bands_db: [12.0; 10],
        };
        let input = sine(1000.0, 4800);
        for f in &input {
            assert_eq!(eq.process(&boosted, *f), *f);
        }
        for f in [31.0, 1000.0, 16000.0] {
            assert_eq!(Coeffs::peaking(f, 0.0, EQ_Q, RATE as f32).response_db(f, RATE as f32), 0.0);
        }
    }

    #[test]
    fn eq_boost_settles_at_the_set_gain_without_jumps() {
        let mut eq = Equalizer::new(RATE);
        let mut target = EqSettings::default();
        target.bands_db[5] = 12.0; // 1 kHz
        let input = sine(1000.0, RATE as usize / 2);
        let out: Vec<[f32; 2]> = input.iter().map(|f| eq.process(&target, *f)).collect();
        let rms = |x: &[[f32; 2]]| (x.iter().map(|f| f[0] * f[0]).sum::<f32>() / x.len() as f32).sqrt();
        let tail = out.len() - 4800;
        let gain_db = 20.0 * (rms(&out[tail..]) / rms(&input[tail..])).log10();
        assert!((gain_db - 12.0).abs() < 0.5, "{gain_db} dB");
        // The ramp up has no step bigger than the boosted tone itself makes.
        let limit = 0.5 * 3.99 * std::f32::consts::TAU * 1000.0 / RATE as f32 * 1.1;
        let worst = out.windows(2).map(|w| (w[1][0] - w[0][0]).abs()).fold(0.0, f32::max);
        assert!(worst <= limit, "jump of {worst} > {limit}");
    }

    #[test]
    fn soft_clip_keeps_quiet_audio_and_bounds_peaks() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.9), -0.9);
        assert!(soft_clip(4.0) <= 1.0 && soft_clip(4.0) > 0.99);
        assert!(soft_clip(-1.2) > -1.0);
        // Continuous at the knee.
        assert!((soft_clip(0.9001) - 0.9001).abs() < 1e-3);
    }

    #[test]
    fn presets_are_in_range_and_recognised() {
        for (name, pre, bands) in EQ_PRESETS {
            assert!((PREAMP_MIN_DB..=PREAMP_MAX_DB).contains(pre));
            assert!(bands.iter().all(|b| b.abs() <= BAND_DB));
            let s = EqSettings {
                enabled: true,
                preamp_db: *pre,
                bands_db: *bands,
            };
            assert_eq!(s.preset(), Some(*name));
        }
    }
}
