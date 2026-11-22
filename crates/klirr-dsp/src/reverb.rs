//! Algorithmic reverb (Freeverb-style: 8 parallel damped combs into 4 series allpasses).
//!
//! A comb feedback of 1 or more makes a reverb blow up. Here the feedback is capped at 0.98, and the damping low-pass in each comb only lowers the loop gain further,
//! so the reverb always decays.

use crate::smooth::Smoothed;
use crate::util::flush_denormal;

const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const TUNING_RATE: f32 = 44_100.0;
const INPUT_GAIN: f32 = 0.015;
const WET_GAIN: f32 = 3.0;
const ALLPASS_FEEDBACK: f32 = 0.5;
/// Comb feedback at size 0 and at size 1.
const FEEDBACK_RANGE: (f32, f32) = (0.70, 0.98);
const SMOOTH_MS: f32 = 50.0;

#[derive(Clone, Debug)]
struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
}

impl Comb {
    #[inline]
    fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.idx];
        self.store = flush_denormal(out * (1.0 - damp) + self.store * damp);
        self.buf[self.idx] = x + self.store * feedback;
        self.idx += 1;
        if self.idx == self.buf.len() {
            self.idx = 0;
        }
        out
    }
}

#[derive(Clone, Debug)]
struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.buf[self.idx];
        self.buf[self.idx] = flush_denormal(x + delayed * ALLPASS_FEEDBACK);
        self.idx += 1;
        if self.idx == self.buf.len() {
            self.idx = 0;
        }
        delayed - x
    }
}

/// The reverb effect. Output is `dry + mix · reverb`.
#[derive(Clone, Debug)]
pub struct Reverb {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
    size: Smoothed,
    damping: Smoothed,
    mix: Smoothed,
}

impl Reverb {
    /// Creates the reverb. Allocates its delay lines.
    pub fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / TUNING_RATE;
        let len = |d: usize| ((d as f32 * scale) as usize).max(1);
        Self {
            combs: COMB_TUNING
                .iter()
                .map(|&d| Comb {
                    buf: vec![0.0; len(d)],
                    idx: 0,
                    store: 0.0,
                })
                .collect(),
            allpasses: ALLPASS_TUNING
                .iter()
                .map(|&d| Allpass {
                    buf: vec![0.0; len(d)],
                    idx: 0,
                })
                .collect(),
            size: Smoothed::new(0.5, SMOOTH_MS, sample_rate),
            damping: Smoothed::new(0.5, SMOOTH_MS, sample_rate),
            mix: Smoothed::new(0.0, SMOOTH_MS, sample_rate),
        }
    }

    /// Sets room size, damping and mix (each 0 – 1).
    pub fn set(&mut self, size: f32, damping: f32, mix: f32) {
        self.size.set_target(size.clamp(0.0, 1.0));
        self.damping.set_target(damping.clamp(0.0, 1.0));
        self.mix.set_target(mix.clamp(0.0, 1.0));
    }

    /// Clears the reverb tail and jumps to the targets.
    pub fn reset(&mut self) {
        for c in &mut self.combs {
            c.buf.fill(0.0);
            c.store = 0.0;
        }
        for a in &mut self.allpasses {
            a.buf.fill(0.0);
        }
        for s in [&mut self.size, &mut self.damping, &mut self.mix] {
            let t = s.target();
            s.set_immediate(t);
        }
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let size = self.size.tick();
        let damp = self.damping.tick() * 0.4;
        let mix = self.mix.tick();
        let feedback = FEEDBACK_RANGE.0 + (FEEDBACK_RANGE.1 - FEEDBACK_RANGE.0) * size;

        let input = x * INPUT_GAIN;
        let mut wet: f32 = self
            .combs
            .iter_mut()
            .map(|c| c.process(input, feedback, damp))
            .sum();
        for a in &mut self.allpasses {
            wet = a.process(wet);
        }
        x + wet * WET_GAIN * mix
    }
}
