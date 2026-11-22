//! Small numeric helpers shared by the DSP modules.

/// Converts decibels to a linear gain factor.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Converts a linear gain factor to decibels. Gains at or below zero map to −240 dB.
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-12).log10()
}

/// Smooth, cheap `tanh` approximation (rational Padé form).
///
/// It is monotonic, exactly ±1 for `|x| ≥ 3`, and its slope reaches zero at ±3, so the curve
/// has no kink. Much cheaper than `f32::tanh` in WebAssembly.
#[inline]
pub fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    // Rounding can overshoot ±1 by an ulp near ±3.
    (x * (27.0 + x2) / (27.0 + 9.0 * x2)).clamp(-1.0, 1.0)
}

/// Replaces values so small they would become subnormal with zero.
///
/// Subnormal floats are very slow on many CPUs; feedback loops decaying towards silence
/// produce them unless flushed.
#[inline]
pub fn flush_denormal(x: f32) -> f32 {
    if x.abs() < 1e-15 { 0.0 } else { x }
}

/// Coefficient for a one-pole smoother reaching ~63 % of a step after `time_ms`.
///
/// Returns `1.0` (no smoothing) for non-positive times.
#[inline]
pub fn one_pole_coeff(time_ms: f32, sample_rate: f32) -> f32 {
    if time_ms <= 0.0 {
        1.0
    } else {
        1.0 - (-1000.0 / (time_ms * sample_rate)).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_roundtrip() {
        for db in [-60.0, -6.0, 0.0, 6.0, 24.0] {
            assert!((gain_to_db(db_to_gain(db)) - db).abs() < 1e-3);
        }
    }

    #[test]
    fn fast_tanh_is_monotonic_and_bounded() {
        let mut prev = -2.0;
        for i in -1000..=1000 {
            let y = fast_tanh(i as f32 * 0.01);
            assert!(y >= prev - 1e-6, "not monotonic at {i}");
            assert!(y.abs() <= 1.0);
            prev = y;
        }
        assert_eq!(fast_tanh(10.0), 1.0);
        assert_eq!(fast_tanh(-10.0), -1.0);
    }

    #[test]
    fn fast_tanh_is_close_to_tanh() {
        for i in -300..=300 {
            let x = i as f32 * 0.01;
            assert!((fast_tanh(x) - x.tanh()).abs() < 0.025, "x = {x}");
        }
    }
}
