//! The test tone: a quiet sine that takes the amp's place on the output, to check that the
//! output device and routing are the right ones without playing a note.
//!
//! It fades in and out over a few milliseconds, so switching it never clicks, and while it
//! sounds the amp is faded out: the tone stays the same whatever the knobs are set to.

/// Frequency of the tone, in Hz (concert A).
pub const FREQUENCY: f32 = 440.0;
/// Peak level of the tone: −20 dBFS, clearly audible without being loud.
pub const LEVEL: f32 = 0.1;
/// How long the fade in and out takes, in seconds.
const FADE_S: f32 = 0.02;

/// The tone generator. Real-time safe: no allocation, no locks.
#[derive(Debug)]
pub struct TestTone {
    /// Position in the cycle, 0..1.
    phase: f32,
    /// Phase advance per sample.
    step: f32,
    /// How much of the output is tone (0: all amp, 1: all tone).
    mix: f32,
    /// Where `mix` is heading.
    target: f32,
    /// Change of `mix` per sample while fading.
    fade_step: f32,
}

impl TestTone {
    /// A silent tone generator for `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            step: FREQUENCY / sample_rate,
            mix: 0.0,
            target: 0.0,
            fade_step: 1.0 / (FADE_S * sample_rate).max(1.0),
        }
    }

    /// Fades the tone in or out.
    pub fn set(&mut self, on: bool) {
        self.target = if on { 1.0 } else { 0.0 };
    }

    /// Whether the tone is off and fully faded out.
    pub fn is_silent(&self) -> bool {
        self.mix == 0.0 && self.target == 0.0
    }

    /// Mixes the tone into `out` (one channel of the amp's output), fading the amp out as
    /// the tone fades in.
    pub fn apply(&mut self, out: &mut [f32]) {
        if self.is_silent() {
            return;
        }
        for y in out {
            if self.mix < self.target {
                self.mix = (self.mix + self.fade_step).min(self.target);
            } else if self.mix > self.target {
                self.mix = (self.mix - self.fade_step).max(self.target);
            }
            let tone = (self.phase * std::f32::consts::TAU).sin() * LEVEL;
            self.phase += self.step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            *y = *y * (1.0 - self.mix) + tone * self.mix;
        }
        if self.is_silent() {
            self.phase = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    #[test]
    fn off_leaves_the_output_alone() {
        let mut tone = TestTone::new(SR);
        let mut out = [0.5_f32; 256];
        tone.apply(&mut out);
        assert!(out.iter().all(|&v| v == 0.5));
    }

    #[test]
    fn on_replaces_the_amp_after_the_fade() {
        let mut tone = TestTone::new(SR);
        tone.set(true);
        // Past the fade, then one second of steady tone over a loud amp.
        let mut fade = vec![0.9_f32; (FADE_S * SR) as usize + 1];
        tone.apply(&mut fade);
        let mut out = vec![0.9_f32; SR as usize];
        tone.apply(&mut out);
        assert!((peak(&out) - LEVEL).abs() < 1e-3, "peak {}", peak(&out));
        // 440 cycles: two zero crossings each.
        let crossings = out
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((878..=882).contains(&crossings), "{crossings} crossings");
    }

    #[test]
    fn switching_never_jumps() {
        let mut tone = TestTone::new(SR);
        let mut out = vec![0.0_f32; 4800];
        tone.set(true);
        tone.apply(&mut out[..2400]);
        tone.set(false);
        tone.apply(&mut out[2400..]);
        // A sine at this level moves at most 2π·f/sr·LEVEL per sample.
        let max_step = std::f32::consts::TAU * FREQUENCY / SR * LEVEL * 1.1;
        assert!(out.windows(2).all(|w| (w[1] - w[0]).abs() <= max_step));
        assert!(tone.is_silent());
    }
}
