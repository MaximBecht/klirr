//! Safety limiter at the end of the chain.
//!
//! It is a guard, not a tone tool: gain drops instantly to whatever keeps the sample under
//! the ceiling and recovers smoothly. In normal use it never engages.

use crate::util::{db_to_gain, one_pole_coeff};

/// Output ceiling in dBFS.
pub const CEILING_DB: f32 = -1.0;
const RELEASE_MS: f32 = 80.0;

/// The safety limiter.
#[derive(Clone, Debug)]
pub struct Limiter {
    ceiling: f32,
    release: f32,
    gain: f32,
    min_gain: f32,
}

impl Limiter {
    /// Creates the limiter.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            ceiling: db_to_gain(CEILING_DB),
            release: one_pole_coeff(RELEASE_MS, sample_rate),
            gain: 1.0,
            min_gain: 1.0,
        }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.min_gain = 1.0;
    }

    /// Lowest gain applied since the last call, as a linear factor; resets the tracker.
    pub fn take_min_gain(&mut self) -> f32 {
        std::mem::replace(&mut self.min_gain, self.gain)
    }

    /// Limits one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let level = x.abs();
        let needed = if level > self.ceiling {
            self.ceiling / level
        } else {
            1.0
        };
        self.gain = (self.gain + (1.0 - self.gain) * self.release).min(needed);
        self.min_gain = self.min_gain.min(self.gain);
        x * self.gain
    }
}
