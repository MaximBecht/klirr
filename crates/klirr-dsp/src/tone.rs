//! Tone stack: bass (low shelf), mid (bell) and treble (high shelf).
//!
//! The filters are trapezoidal state-variable filters (Andrew Simper / Cytomic design). Unlike
//! a biquad, an SVF stays well-behaved when its gain changes on every sample, so knob moves
//! glide with no ticks. The state is never reset while playing (resetting it every block is
//! a classic cause of block-rate buzz).

use crate::smooth::Smoothed;

/// Bass shelf frequency in Hz.
pub const BASS_HZ: f32 = 200.0;
/// Mid bell frequency in Hz.
pub const MID_HZ: f32 = 800.0;
/// Mid bell Q.
pub const MID_Q: f32 = 0.7;
/// Treble shelf frequency in Hz.
pub const TREBLE_HZ: f32 = 3500.0;
const SHELF_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
const SMOOTH_MS: f32 = 30.0;

#[derive(Clone, Copy, Debug)]
enum Shape {
    LowShelf,
    Bell,
    HighShelf,
}

#[derive(Clone, Copy, Debug, Default)]
struct SvfCoeffs {
    a1: f64,
    a2: f64,
    a3: f64,
    m0: f64,
    m1: f64,
    m2: f64,
}

/// One EQ band as a state-variable filter.
#[derive(Clone, Debug)]
struct Band {
    shape: Shape,
    /// `tan(π·f/fs)`, fixed because the frequency never changes.
    g: f64,
    q: f64,
    gain_db: Smoothed,
    c: SvfCoeffs,
    ic1: f64,
    ic2: f64,
}

impl Band {
    fn new(shape: Shape, freq: f32, q: f64, sample_rate: f32) -> Self {
        let g = (std::f64::consts::PI * f64::from(freq) / f64::from(sample_rate)).tan();
        let mut band = Self {
            shape,
            g,
            q,
            gain_db: Smoothed::new(0.0, SMOOTH_MS, sample_rate),
            c: SvfCoeffs::default(),
            ic1: 0.0,
            ic2: 0.0,
        };
        band.update_coeffs();
        band
    }

    fn update_coeffs(&mut self) {
        let a = 10.0_f64.powf(f64::from(self.gain_db.value()) / 40.0);
        let (g, k, m0, m1, m2) = match self.shape {
            Shape::Bell => {
                let k = 1.0 / (self.q * a);
                (self.g, k, 1.0, k * (a * a - 1.0), 0.0)
            }
            Shape::LowShelf => {
                let k = 1.0 / self.q;
                (self.g / a.sqrt(), k, 1.0, k * (a - 1.0), a * a - 1.0)
            }
            Shape::HighShelf => {
                let k = 1.0 / self.q;
                (self.g * a.sqrt(), k, a * a, k * (1.0 - a) * a, 1.0 - a * a)
            }
        };
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        self.c = SvfCoeffs {
            a1,
            a2,
            a3: g * a2,
            m0,
            m1,
            m2,
        };
    }

    fn reset(&mut self) {
        let t = self.gain_db.target();
        self.gain_db.set_immediate(t);
        self.update_coeffs();
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        if !self.gain_db.is_settled() {
            self.gain_db.tick();
            self.update_coeffs();
        }
        let c = &self.c;
        let v0 = f64::from(x);
        let v3 = v0 - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        if self.ic1.abs() < 1e-30 {
            self.ic1 = 0.0;
        }
        if self.ic2.abs() < 1e-30 {
            self.ic2 = 0.0;
        }
        (c.m0 * v0 + c.m1 * v1 + c.m2 * v2) as f32
    }
}

/// The tone stack.
#[derive(Clone, Debug)]
pub struct ToneStack {
    bands: [Band; 3],
}

impl ToneStack {
    /// Creates a flat tone stack.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            bands: [
                Band::new(Shape::LowShelf, BASS_HZ, SHELF_Q, sample_rate),
                Band::new(Shape::Bell, MID_HZ, f64::from(MID_Q), sample_rate),
                Band::new(Shape::HighShelf, TREBLE_HZ, SHELF_Q, sample_rate),
            ],
        }
    }

    /// Sets the targets in dB (the caller clamps them to ±12 dB). Changes glide.
    pub fn set_gains_db(&mut self, bass: f32, mid: f32, treble: f32) {
        for (band, db) in self.bands.iter_mut().zip([bass, mid, treble]) {
            band.gain_db.set_target(db);
        }
    }

    /// Clears the filter state and jumps to the targets.
    pub fn reset(&mut self) {
        for band in &mut self.bands {
            band.reset();
        }
    }

    /// Filters one sample. At 0 dB every band is an exact identity.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.bands.iter_mut().fold(x, |v, band| band.process(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn steady_gain_db(stack: &mut ToneStack, freq: f32) -> f32 {
        let mut peak = 0.0_f32;
        for n in 0..96_000 {
            let x = (2.0 * std::f32::consts::PI * freq * n as f32 / SR).sin();
            let y = stack.process(x);
            if n > 48_000 {
                peak = peak.max(y.abs());
            }
        }
        crate::util::gain_to_db(peak)
    }

    #[test]
    fn flat_is_identity() {
        let mut stack = ToneStack::new(SR);
        for n in 0..1000 {
            let x = (n as f32 * 0.123).sin();
            assert_eq!(stack.process(x), x);
        }
    }

    #[test]
    fn bands_reach_their_gain() {
        let cases = [
            ((12.0, 0.0, 0.0), 30.0, 12.0),
            ((-12.0, 0.0, 0.0), 30.0, -12.0),
            ((0.0, 12.0, 0.0), MID_HZ, 12.0),
            ((0.0, 0.0, 12.0), 18_000.0, 12.0),
            ((0.0, 0.0, -12.0), 18_000.0, -12.0),
            ((12.0, 0.0, 12.0), 1000.0, 0.0),
        ];
        for ((b, m, t), freq, expected) in cases {
            let mut stack = ToneStack::new(SR);
            stack.set_gains_db(b, m, t);
            stack.reset();
            let got = steady_gain_db(&mut stack, freq);
            let tolerance = if expected == 0.0 { 3.0 } else { 0.6 };
            assert!(
                (got - expected).abs() < tolerance,
                "{b}/{m}/{t} at {freq} Hz: {got:.2} dB"
            );
        }
    }
}
