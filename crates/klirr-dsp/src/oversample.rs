//! 4× oversampling around non-linear stages.
//!
//! Distortion creates harmonics far above the audible range. At the base sample rate those
//! harmonics fold back ("alias") into the audible band as inharmonic, fizzy noise. Running the
//! waveshaper at 4× the rate and filtering before returning to the base rate removes most of it.
//!
//! Both directions use the same linear-phase, Kaiser-windowed sinc low-pass filter, implemented
//! polyphase so no work is spent on the zeros inserted when upsampling.

use std::f64::consts::PI;

/// Oversampling factor.
pub const FACTOR: usize = 4;
/// Filter length at the oversampled rate.
pub const TAPS: usize = 96;
const TAPS_PER_PHASE: usize = TAPS / FACTOR;
/// Cut-off as a fraction of the oversampled rate (≈ 20.6 kHz at a 48 kHz base rate).
const CUTOFF: f64 = 0.5 / FACTOR as f64 * 0.86;
const KAISER_BETA: f64 = 8.6;

/// Round-trip latency (upsample + downsample) in base-rate samples, rounded.
pub const LATENCY: usize = (TAPS - 1) / FACTOR;

fn bessel_i0(x: f64) -> f64 {
    let y = x * x / 4.0;
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..64 {
        term *= y / f64::from(k * k);
        sum += term;
        if term < 1e-14 * sum {
            break;
        }
    }
    sum
}

fn design_lowpass() -> [f64; TAPS] {
    let mut h = [0.0; TAPS];
    let centre = (TAPS - 1) as f64 / 2.0;
    let norm = bessel_i0(KAISER_BETA);
    for (n, tap) in h.iter_mut().enumerate() {
        let t = n as f64 - centre;
        let sinc = if t == 0.0 {
            2.0 * CUTOFF
        } else {
            (2.0 * PI * CUTOFF * t).sin() / (PI * t)
        };
        let r = t / centre;
        let window = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / norm;
        *tap = sinc * window;
    }
    let sum: f64 = h.iter().sum();
    for tap in &mut h {
        *tap /= sum;
    }
    h
}

/// A 4× up/down sampler for one mono signal.
#[derive(Clone, Debug)]
pub struct Oversampler4x {
    up_phases: [[f32; TAPS_PER_PHASE]; FACTOR],
    down: [f32; TAPS],
    up_hist: [f32; 2 * TAPS_PER_PHASE],
    up_pos: usize,
    down_hist: [f32; 2 * TAPS],
    down_pos: usize,
}

impl Default for Oversampler4x {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler4x {
    /// Creates an oversampler with silent state.
    pub fn new() -> Self {
        let h = design_lowpass();
        let mut up_phases = [[0.0; TAPS_PER_PHASE]; FACTOR];
        for (p, phase) in up_phases.iter_mut().enumerate() {
            for (k, tap) in phase.iter_mut().enumerate() {
                // Zero-stuffing divides the level by FACTOR; the polyphase taps restore it.
                *tap = (h[k * FACTOR + p] * FACTOR as f64) as f32;
            }
        }
        let mut down = [0.0; TAPS];
        for (d, &v) in down.iter_mut().zip(h.iter()) {
            *d = v as f32;
        }
        Self {
            up_phases,
            down,
            up_hist: [0.0; 2 * TAPS_PER_PHASE],
            up_pos: 0,
            down_hist: [0.0; 2 * TAPS],
            down_pos: 0,
        }
    }

    /// Clears the filter state.
    pub fn reset(&mut self) {
        self.up_hist = [0.0; 2 * TAPS_PER_PHASE];
        self.down_hist = [0.0; 2 * TAPS];
        self.up_pos = 0;
        self.down_pos = 0;
    }

    /// Turns one base-rate sample into [`FACTOR`] oversampled samples.
    #[inline]
    pub fn upsample(&mut self, x: f32) -> [f32; FACTOR] {
        self.up_pos = (self.up_pos + TAPS_PER_PHASE - 1) % TAPS_PER_PHASE;
        self.up_hist[self.up_pos] = x;
        self.up_hist[self.up_pos + TAPS_PER_PHASE] = x;
        // window[k] is the input from k samples ago.
        let window = &self.up_hist[self.up_pos..self.up_pos + TAPS_PER_PHASE];
        let mut out = [0.0; FACTOR];
        for (o, phase) in out.iter_mut().zip(self.up_phases.iter()) {
            *o = phase.iter().zip(window).map(|(h, x)| h * x).sum();
        }
        out
    }

    /// Turns [`FACTOR`] oversampled samples back into one base-rate sample.
    #[inline]
    pub fn downsample(&mut self, block: [f32; FACTOR]) -> f32 {
        for v in block {
            self.down_pos = (self.down_pos + TAPS - 1) % TAPS;
            self.down_hist[self.down_pos] = v;
            self.down_hist[self.down_pos + TAPS] = v;
        }
        let window = &self.down_hist[self.down_pos..self.down_pos + TAPS];
        self.down.iter().zip(window).map(|(h, x)| h * x).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn roundtrip_peak(freq: f32) -> f32 {
        let mut os = Oversampler4x::new();
        let mut peak = 0.0_f32;
        for n in 0..9600 {
            let x = (2.0 * std::f32::consts::PI * freq * n as f32 / SR).sin();
            let up = os.upsample(x);
            let y = os.downsample(up);
            if n > 4800 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn passband_is_flat() {
        for f in [100.0, 1000.0, 5000.0, 12_000.0] {
            let p = roundtrip_peak(f);
            assert!((p - 1.0).abs() < 0.01, "{f} Hz: {p}");
        }
    }

    #[test]
    fn latency_matches_constant() {
        let mut os = Oversampler4x::new();
        let mut out = Vec::new();
        for n in 0..64 {
            let x = if n == 0 { 1.0 } else { 0.0 };
            let up = os.upsample(x);
            out.push(os.downsample(up));
        }
        let peak_at = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert!(peak_at.abs_diff(LATENCY) <= 1, "peak at {peak_at}");
    }

    #[test]
    fn upsampled_images_are_suppressed() {
        // A 10 kHz tone upsampled to 192 kHz has images at 38, 58, 86 ... kHz.
        let mut os = Oversampler4x::new();
        let n_out = 4 * 4096;
        let mut up = Vec::with_capacity(n_out);
        for n in 0..(n_out / 4 + 200) {
            let x = (2.0 * std::f32::consts::PI * 10_000.0 * n as f32 / SR).sin();
            let block = os.upsample(x);
            if n >= 200 {
                up.extend_from_slice(&block);
            }
        }
        // Single-bin DFT at the first image (38 kHz) vs the tone (10 kHz), Hann-windowed.
        let dft = |f: f32| {
            let (mut re, mut im) = (0.0_f64, 0.0_f64);
            let len = up.len() as f64;
            for (i, &v) in up.iter().enumerate() {
                let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / len).cos();
                let ph =
                    2.0 * std::f64::consts::PI * f64::from(f) * i as f64 / (4.0 * f64::from(SR));
                re += f64::from(v) * w * ph.cos();
                im -= f64::from(v) * w * ph.sin();
            }
            (re * re + im * im).sqrt()
        };
        let ratio_db = 20.0 * (dft(38_000.0) / dft(10_000.0)).log10();
        assert!(ratio_db < -70.0, "image at {ratio_db:.1} dB");
    }
}
