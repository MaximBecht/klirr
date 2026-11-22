//! Second-order IIR filters (RBJ "Audio EQ Cookbook" designs).
//!
//! State and coefficients are `f64`: low-frequency filters at oversampled rates lose too much
//! precision in `f32`. The filter is transposed direct form II, which tolerates coefficient
//! changes while running, so EQ changes never need a state reset.

use std::f64::consts::PI;

/// Normalised biquad coefficients (`a0 = 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    /// Passes the signal through unchanged.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalise(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Returns `(cos w0, sin w0)` with the frequency kept safely below Nyquist.
    fn omega(freq: f32, sample_rate: f32) -> (f64, f64) {
        let f = f64::from(freq).clamp(1.0, 0.49 * f64::from(sample_rate));
        let w0 = 2.0 * PI * f / f64::from(sample_rate);
        (w0.cos(), w0.sin())
    }

    /// Low-pass filter.
    pub fn lowpass(freq: f32, q: f32, sample_rate: f32) -> Self {
        let (cos, sin) = Self::omega(freq, sample_rate);
        let alpha = sin / (2.0 * f64::from(q));
        let b1 = 1.0 - cos;
        Self::normalise(b1 / 2.0, b1, b1 / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// High-pass filter.
    pub fn highpass(freq: f32, q: f32, sample_rate: f32) -> Self {
        let (cos, sin) = Self::omega(freq, sample_rate);
        let alpha = sin / (2.0 * f64::from(q));
        let b1 = -(1.0 + cos);
        Self::normalise(
            -b1 / 2.0,
            b1,
            -b1 / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// Peaking (bell) EQ.
    pub fn peaking(freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> Self {
        let (cos, sin) = Self::omega(freq, sample_rate);
        let a = 10.0_f64.powf(f64::from(gain_db) / 40.0);
        let alpha = sin / (2.0 * f64::from(q));
        Self::normalise(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        )
    }

    /// Low shelf with a shelf slope of 1 (the steepest slope without overshoot).
    pub fn low_shelf(freq: f32, gain_db: f32, sample_rate: f32) -> Self {
        let (cos, sin) = Self::omega(freq, sample_rate);
        let a = 10.0_f64.powf(f64::from(gain_db) / 40.0);
        let alpha = sin / 2.0 * 2.0_f64.sqrt();
        let sq = 2.0 * a.sqrt() * alpha;
        Self::normalise(
            a * ((a + 1.0) - (a - 1.0) * cos + sq),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - sq),
            (a + 1.0) + (a - 1.0) * cos + sq,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - sq,
        )
    }

    /// High shelf with a shelf slope of 1.
    pub fn high_shelf(freq: f32, gain_db: f32, sample_rate: f32) -> Self {
        let (cos, sin) = Self::omega(freq, sample_rate);
        let a = 10.0_f64.powf(f64::from(gain_db) / 40.0);
        let alpha = sin / 2.0 * 2.0_f64.sqrt();
        let sq = 2.0 * a.sqrt() * alpha;
        Self::normalise(
            a * ((a + 1.0) + (a - 1.0) * cos + sq),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - sq),
            (a + 1.0) - (a - 1.0) * cos + sq,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - sq,
        )
    }

    /// Magnitude response at `freq`, as a linear factor.
    pub fn magnitude(&self, freq: f32, sample_rate: f32) -> f32 {
        let w = 2.0 * PI * f64::from(freq) / f64::from(sample_rate);
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).sqrt() as f32
    }
}

/// A running biquad filter.
#[derive(Clone, Debug)]
pub struct Biquad {
    c: Coeffs,
    s1: f64,
    s2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new(Coeffs::IDENTITY)
    }
}

impl Biquad {
    /// Creates a filter with the given coefficients and silent state.
    pub fn new(coeffs: Coeffs) -> Self {
        Self {
            c: coeffs,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Replaces the coefficients, keeping the state (no click).
    pub fn set_coeffs(&mut self, coeffs: Coeffs) {
        self.c = coeffs;
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let c = &self.c;
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        if self.s1.abs() < 1e-30 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-30 {
            self.s2 = 0.0;
        }
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn zero_db_eq_is_identity() {
        for c in [
            Coeffs::peaking(800.0, 0.7, 0.0, SR),
            Coeffs::low_shelf(200.0, 0.0, SR),
            Coeffs::high_shelf(3500.0, 0.0, SR),
        ] {
            for f in [50.0, 500.0, 5000.0, 15000.0] {
                assert!((c.magnitude(f, SR) - 1.0).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn shelves_reach_their_gain() {
        let low = Coeffs::low_shelf(200.0, 12.0, SR);
        assert!((crate::util::gain_to_db(low.magnitude(20.0, SR)) - 12.0).abs() < 0.3);
        let high = Coeffs::high_shelf(3500.0, -12.0, SR);
        assert!((crate::util::gain_to_db(high.magnitude(20_000.0, SR)) + 12.0).abs() < 0.5);
    }

    #[test]
    fn lowpass_and_highpass_shapes() {
        let lp = Coeffs::lowpass(1000.0, 0.707, SR);
        assert!((lp.magnitude(10.0, SR) - 1.0).abs() < 1e-3);
        assert!(lp.magnitude(10_000.0, SR) < 0.02);
        let hp = Coeffs::highpass(100.0, 0.707, SR);
        assert!(hp.magnitude(10.0, SR) < 0.02);
        assert!((hp.magnitude(5_000.0, SR) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn running_filter_matches_magnitude() {
        let c = Coeffs::peaking(1000.0, 1.0, 6.0, SR);
        let mut f = Biquad::new(c);
        let mut peak = 0.0_f32;
        for n in 0..48_000 {
            let x = (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin();
            let y = f.process(x);
            if n > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - c.magnitude(1000.0, SR)).abs() < 0.01);
    }
}
