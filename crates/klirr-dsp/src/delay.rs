//! Echo delay with a fractional, gliding delay time and a darkening feedback loop.

use crate::smooth::Smoothed;
use crate::util::{flush_denormal, one_pole_coeff};

/// Longest supported delay time, in milliseconds.
pub const MAX_TIME_MS: f32 = 2000.0;
/// Highest feedback amount (kept below 1 so repeats always die out).
pub const MAX_FEEDBACK: f32 = 0.95;
/// Delay-time changes glide like a tape echo instead of jumping (which would click).
const TIME_GLIDE_MS: f32 = 120.0;
const LEVEL_SMOOTH_MS: f32 = 20.0;
/// Each repeat passes a gentle low-pass, so echoes get darker as they fade.
const FEEDBACK_LOWPASS_MS: f32 = 0.035; // one-pole time constant ≈ 4.5 kHz

/// The delay effect. Output is `dry + mix · echoes`.
#[derive(Clone, Debug)]
pub struct Delay {
    sample_rate: f32,
    buf: Vec<f32>,
    mask: usize,
    write: usize,
    time: Smoothed,
    feedback: Smoothed,
    mix: Smoothed,
    lp: f32,
    lp_coeff: f32,
}

impl Delay {
    /// Creates the delay. Allocates the delay line for [`MAX_TIME_MS`].
    pub fn new(sample_rate: f32) -> Self {
        let needed = (MAX_TIME_MS / 1000.0 * sample_rate) as usize + 4;
        let len = needed.next_power_of_two();
        Self {
            sample_rate,
            buf: vec![0.0; len],
            mask: len - 1,
            write: 0,
            time: Smoothed::new(
                Self::ms_to_samples(300.0, sample_rate),
                TIME_GLIDE_MS,
                sample_rate,
            ),
            feedback: Smoothed::new(0.35, LEVEL_SMOOTH_MS, sample_rate),
            mix: Smoothed::new(0.0, LEVEL_SMOOTH_MS, sample_rate),
            lp: 0.0,
            lp_coeff: one_pole_coeff(FEEDBACK_LOWPASS_MS, sample_rate),
        }
    }

    fn ms_to_samples(ms: f32, sample_rate: f32) -> f32 {
        ms.clamp(1.0, MAX_TIME_MS) / 1000.0 * sample_rate
    }

    /// Sets delay time (ms), feedback (0 – [`MAX_FEEDBACK`]) and mix (0 – 1).
    pub fn set(&mut self, time_ms: f32, feedback: f32, mix: f32) {
        self.time
            .set_target(Self::ms_to_samples(time_ms, self.sample_rate));
        self.feedback.set_target(feedback.clamp(0.0, MAX_FEEDBACK));
        self.mix.set_target(mix.clamp(0.0, 1.0));
    }

    /// Clears the delay line and jumps to the targets.
    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.lp = 0.0;
        for s in [&mut self.time, &mut self.feedback, &mut self.mix] {
            let t = s.target();
            s.set_immediate(t);
        }
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let delay = f64::from(self.time.tick());
        let feedback = self.feedback.tick();
        let mix = self.mix.tick();

        // Linear interpolation between the two samples around `write - delay`.
        let pos = self.write as f64 - delay;
        let base = pos.floor();
        let frac = (pos - base) as f32;
        let i = (base as i64).rem_euclid(self.buf.len() as i64) as usize;
        let a = self.buf[i];
        let b = self.buf[(i + 1) & self.mask];
        let echo = a + (b - a) * frac;

        self.lp += (echo - self.lp) * self.lp_coeff;
        self.lp = flush_denormal(self.lp);
        self.buf[self.write] = flush_denormal(x + self.lp * feedback);
        self.write = (self.write + 1) & self.mask;

        x + echo * mix
    }
}
