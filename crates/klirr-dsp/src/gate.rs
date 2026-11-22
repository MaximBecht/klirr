//! Noise gate with smooth attack / hold / release and hysteresis.
//!
//! Switching whole audio blocks on or off would chop note tails and click. This gate follows the signal envelope per sample, opens above the threshold, only starts
//! closing after the level has fallen 6 dB *below* the threshold for the hold time, and fades
//! its gain instead of switching it.

use crate::util::{db_to_gain, one_pole_coeff};

/// At or below this threshold the gate is disabled (always open).
pub const OFF_DB: f32 = -96.0;
const HYSTERESIS: f32 = 0.5; // close at threshold − 6 dB
const ENVELOPE_RELEASE_MS: f32 = 10.0;
const ATTACK_MS: f32 = 0.5;
const HOLD_MS: f32 = 40.0;
const RELEASE_MS: f32 = 60.0;

/// The noise gate.
#[derive(Clone, Debug)]
pub struct Gate {
    open_level: f32,
    enabled: bool,
    envelope: f32,
    env_release: f32,
    attack: f32,
    release: f32,
    hold_samples: u32,
    hold_left: u32,
    open: bool,
    gain: f32,
}

impl Gate {
    /// Creates a gate at `threshold_db`.
    pub fn new(threshold_db: f32, sample_rate: f32) -> Self {
        let mut gate = Self {
            open_level: 0.0,
            enabled: false,
            envelope: 0.0,
            env_release: 1.0 - one_pole_coeff(ENVELOPE_RELEASE_MS, sample_rate),
            attack: one_pole_coeff(ATTACK_MS, sample_rate),
            release: one_pole_coeff(RELEASE_MS, sample_rate),
            hold_samples: (HOLD_MS / 1000.0 * sample_rate) as u32,
            hold_left: (HOLD_MS / 1000.0 * sample_rate) as u32,
            open: true,
            gain: 1.0,
        };
        gate.set_threshold_db(threshold_db);
        gate
    }

    /// Sets the opening threshold in dBFS. [`OFF_DB`] or lower disables the gate.
    pub fn set_threshold_db(&mut self, threshold_db: f32) {
        self.enabled = threshold_db > OFF_DB;
        self.open_level = db_to_gain(threshold_db);
    }

    /// Clears the state (gate open).
    pub fn reset(&mut self) {
        self.envelope = 0.0;
        // Start open and holding, so the first note is never clipped while the envelope rises.
        self.hold_left = self.hold_samples;
        self.open = true;
        self.gain = 1.0;
    }

    /// `true` while the gate is open.
    pub fn is_open(&self) -> bool {
        self.open || !self.enabled
    }

    /// The gain currently applied (0 – 1).
    pub fn gain(&self) -> f32 {
        if self.enabled { self.gain } else { 1.0 }
    }

    /// Gates one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let level = x.abs();
        self.envelope = if level > self.envelope {
            level
        } else {
            self.envelope * self.env_release
        };

        if !self.enabled {
            return x;
        }

        if self.envelope >= self.open_level {
            self.open = true;
            self.hold_left = self.hold_samples;
        } else if self.envelope < self.open_level * HYSTERESIS {
            if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.open = false;
            }
        }

        let (target, coeff) = if self.open {
            (1.0, self.attack)
        } else {
            (0.0, self.release)
        };
        self.gain += (target - self.gain) * coeff;
        if self.gain < 1e-6 {
            self.gain = 0.0;
        }
        x * self.gain
    }
}
