//! Parameter smoothing, so knob changes glide instead of clicking.

use crate::util::one_pole_coeff;

/// A value that follows its target with a one-pole (exponential) glide.
#[derive(Clone, Debug)]
pub struct Smoothed {
    current: f32,
    target: f32,
    coeff: f32,
}

impl Smoothed {
    /// Creates a smoother starting (and settled) at `value`.
    ///
    /// `time_ms` is the time to cover ~63 % of a change.
    pub fn new(value: f32, time_ms: f32, sample_rate: f32) -> Self {
        Self {
            current: value,
            target: value,
            coeff: one_pole_coeff(time_ms, sample_rate),
        }
    }

    /// Sets a new target; the value glides towards it.
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jumps to `value` without gliding.
    pub fn set_immediate(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    /// Advances one sample and returns the new value.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.current != self.target {
            let next = self.current + (self.target - self.current) * self.coeff;
            self.current = self.snap(next);
        }
        self.current
    }

    /// Advances `n` samples at once (used for control-rate parameters) and returns the value.
    pub fn advance(&mut self, n: usize) -> f32 {
        if self.current != self.target {
            let remaining = (1.0 - self.coeff).powi(n as i32);
            let next = self.target + (self.current - self.target) * remaining;
            self.current = self.snap(next);
        }
        self.current
    }

    /// Lands exactly on the target once close enough, or once `f32` rounding stops progress.
    #[inline]
    fn snap(&self, next: f32) -> f32 {
        if next == self.current || (self.target - next).abs() <= 1e-6 * (1.0 + self.target.abs()) {
            self.target
        } else {
            next
        }
    }

    /// The current value.
    pub fn value(&self) -> f32 {
        self.current
    }

    /// The value being approached.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// `true` once the value has reached its target.
    pub fn is_settled(&self) -> bool {
        self.current == self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glides_and_settles() {
        let mut s = Smoothed::new(0.0, 5.0, 48_000.0);
        s.set_target(1.0);
        let first = s.tick();
        assert!(first > 0.0 && first < 0.01);
        for _ in 0..48_000 {
            s.tick();
        }
        assert!(s.is_settled());
        assert_eq!(s.value(), 1.0);
    }

    #[test]
    fn advance_matches_repeated_ticks() {
        let mut a = Smoothed::new(0.0, 10.0, 48_000.0);
        let mut b = a.clone();
        a.set_target(1.0);
        b.set_target(1.0);
        for _ in 0..32 {
            a.tick();
        }
        b.advance(32);
        assert!((a.value() - b.value()).abs() < 1e-5);
    }
}
