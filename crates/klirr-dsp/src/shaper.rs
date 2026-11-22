//! Soft clipper with first-order antiderivative anti-aliasing (ADAA).
//!
//! Instead of evaluating the curve `f` at each sample, ADAA outputs the average of `f` over
//! the straight line between the previous and the current input:
//! `(F(x[n]) − F(x[n−1])) / (x[n] − x[n−1])`, where `F` is the antiderivative of `f`.
//! That suppresses the harmonics that would alias, on top of the 4× oversampling, at the cost
//! of half a sample of delay at the oversampled rate.
//!
//! The curve is [`crate::util::fast_tanh`]: `x(27 + x²) / (27 + 9x²)` for `|x| < 3`, else ±1.
//! Its antiderivative is `x²/18 + (4/3)·ln(9x² + 27)`, continued linearly beyond the knee.
//! Computation is in `f64` because the difference quotient cancels digits.

const KNEE: f64 = 3.0;
/// `F(3) = 9/18 + (4/3)·ln(108)`.
const F_AT_KNEE: f64 = 6.742_841_636_165_63;
/// Below this input step, the quotient is ill-conditioned; evaluate the curve directly.
const MIN_STEP: f64 = 1e-6;

#[inline]
fn curve(x: f64) -> f64 {
    if x.abs() >= KNEE {
        x.signum()
    } else {
        let x2 = x * x;
        x * (27.0 + x2) / (27.0 + 9.0 * x2)
    }
}

#[inline]
fn antiderivative(x: f64) -> f64 {
    let a = x.abs();
    if a >= KNEE {
        a - KNEE + F_AT_KNEE
    } else {
        x * x / 18.0 + (4.0 / 3.0) * (9.0 * x * x + 27.0).ln()
    }
}

/// An anti-aliased `tanh`-like soft clipper.
#[derive(Clone, Debug)]
pub struct AdaaTanh {
    rest: f64,
    x1: f64,
    f1: f64,
}

impl Default for AdaaTanh {
    fn default() -> Self {
        Self::new()
    }
}

/// The clipper curve evaluated directly (no anti-aliasing), e.g. to compute a bias offset
/// that exactly matches what [`AdaaTanh`] outputs for a constant input.
pub fn curve_value(x: f32) -> f32 {
    curve(f64::from(x)) as f32
}

impl AdaaTanh {
    /// Creates the clipper with silent state.
    pub fn new() -> Self {
        Self::at_rest(0.0)
    }

    /// Creates the clipper as if its input had been `rest` forever (e.g. a bias voltage), so
    /// a constant input produces no start-up transient.
    pub fn at_rest(rest: f32) -> Self {
        let rest = f64::from(rest);
        Self {
            rest,
            x1: rest,
            f1: antiderivative(rest),
        }
    }

    /// Clears the state back to the resting input.
    pub fn reset(&mut self) {
        self.x1 = self.rest;
        self.f1 = antiderivative(self.rest);
    }

    /// Clips one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let fx = antiderivative(x);
        let step = x - self.x1;
        let y = if step.abs() > MIN_STEP {
            (fx - self.f1) / step
        } else {
            curve(0.5 * (x + self.x1))
        };
        self.x1 = x;
        self.f1 = fx;
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn antiderivative_is_continuous_at_the_knee() {
        for k in [KNEE, -KNEE] {
            let inside = antiderivative(k * (1.0 - 1e-9));
            let outside = antiderivative(k);
            assert!((inside - outside).abs() < 1e-6);
        }
    }

    #[test]
    fn antiderivative_matches_the_curve() {
        for i in -500..500 {
            let x = f64::from(i) * 0.01;
            let h = 1e-5;
            let slope = (antiderivative(x + h) - antiderivative(x - h)) / (2.0 * h);
            assert!((slope - curve(x)).abs() < 1e-6, "x = {x}");
        }
    }

    #[test]
    fn constant_input_gives_the_curve_value() {
        let mut s = AdaaTanh::new();
        let mut y = 0.0;
        for _ in 0..4 {
            y = s.process(0.7);
        }
        assert!((f64::from(y) - curve(0.7)).abs() < 1e-6);
    }

    #[test]
    fn output_stays_bounded() {
        let mut s = AdaaTanh::new();
        for i in 0..10_000 {
            let x = ((i as f32) * 0.37).sin() * 50.0;
            assert!(s.process(x).abs() <= 1.0 + 1e-6);
        }
    }
}
