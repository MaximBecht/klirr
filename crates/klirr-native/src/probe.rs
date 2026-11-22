//! Loopback latency measurement: the same probe as the web app (`web/src/audio/probe.ts`), so
//! the two figures compare directly.
//!
//! With a cable from the interface's output to its input, the probe plays short clicks and
//! counts the frames until each click comes back. That round trip covers everything outside
//! the engine: output buffers, driver, DAC, cable, ADC, input buffers and the buffer between
//! the input and output streams. Added to the engine's own latency it is the real
//! string-to-ear figure.

/// Number of clicks played.
pub const PINGS: usize = 6;
/// Level of each click (a single sample), linear.
const CLICK_LEVEL: f32 = 0.5;
/// Silence before the first click, so the amp's tail has died away.
const SETTLE_S: f32 = 0.4;
/// The last part of the settle time measures the noise floor.
const NOISE_S: f32 = 0.2;
/// A click that has not come back after this long is counted as lost.
const LISTEN_S: f32 = 1.0;
/// Pause after each click, so its ringing has decayed before the next one.
const GAP_S: f32 = 0.25;
/// The input must stay below the threshold for this long before each click; otherwise a
/// ringing string would "answer" the click at once.
const QUIET_S: f32 = 0.1;
const MIN_THRESHOLD: f32 = 0.01;
/// An input louder than this before the first click is too noisy to measure.
const MAX_NOISE: f32 = 0.08;

/// The raw outcome of one probe run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProbeResult {
    /// Round trips in frames; only the first `count` entries are used.
    Done {
        /// Round trip of each click that came back, in frames.
        round_trips: [u32; PINGS],
        /// Number of clicks that came back.
        count: usize,
    },
    /// The input was too loud to hear the clicks.
    Noisy,
    /// Nothing came back.
    NoSignal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Settle,
    Listen,
    Gap,
}

/// Runs one measurement, one sample at a time. Never allocates, so it can live on the audio
/// thread.
#[derive(Debug)]
pub struct LoopbackProbe {
    sample_rate: f32,
    settle_frames: u32,
    noise_frames: u32,
    listen_frames: u32,
    gap_frames: u32,
    quiet_frames: u32,
    hits: [u32; PINGS],
    hit_count: usize,
    pings: usize,
    misses: usize,
    phase: Phase,
    /// Frames since the current phase started.
    t: u32,
    noise_peak: f32,
    threshold: f32,
    outcome: Option<ProbeResult>,
}

impl LoopbackProbe {
    /// A probe for the given sample rate, ready to run.
    pub fn new(sample_rate: f32) -> Self {
        let frames = |s: f32| (s * sample_rate).round() as u32;
        Self {
            sample_rate,
            settle_frames: frames(SETTLE_S),
            noise_frames: frames(NOISE_S),
            listen_frames: frames(LISTEN_S),
            gap_frames: frames(GAP_S),
            quiet_frames: frames(QUIET_S),
            hits: [0; PINGS],
            hit_count: 0,
            pings: 0,
            misses: 0,
            phase: Phase::Settle,
            t: 0,
            noise_peak: 0.0,
            threshold: MIN_THRESHOLD,
            outcome: None,
        }
    }

    /// Starts a new measurement.
    pub fn restart(&mut self) {
        *self = Self::new(self.sample_rate);
    }

    /// The result, once the measurement is over.
    pub fn result(&self) -> Option<ProbeResult> {
        self.outcome
    }

    /// Reads one input sample and returns the probe's output sample (silence or a click).
    /// Once the measurement is over it returns silence.
    pub fn tick(&mut self, input: f32) -> f32 {
        if self.outcome.is_some() {
            return 0.0;
        }
        let x = input.abs();
        match self.phase {
            Phase::Settle => {
                if self.t >= self.settle_frames - self.noise_frames {
                    self.noise_peak = self.noise_peak.max(x);
                }
                self.t += 1;
                if self.t >= self.settle_frames {
                    if self.noise_peak > MAX_NOISE {
                        self.outcome = Some(ProbeResult::Noisy);
                        return 0.0;
                    }
                    self.threshold = MIN_THRESHOLD.max(self.noise_peak * 4.0);
                    return self.click();
                }
            }
            Phase::Listen => {
                // `t` counts frames since the click, which can come back one frame later at
                // the earliest.
                self.t += 1;
                if x >= self.threshold {
                    self.hits[self.hit_count] = self.t;
                    self.hit_count += 1;
                    self.next(true);
                } else if self.t > self.listen_frames {
                    self.next(false);
                }
            }
            Phase::Gap => {
                if self.t >= self.gap_frames - self.quiet_frames && x >= self.threshold {
                    self.outcome = Some(ProbeResult::Noisy);
                    return 0.0;
                }
                self.t += 1;
                if self.t >= self.gap_frames {
                    return self.click();
                }
            }
        }
        0.0
    }

    fn click(&mut self) -> f32 {
        self.pings += 1;
        self.phase = Phase::Listen;
        self.t = 0;
        CLICK_LEVEL
    }

    fn next(&mut self, hit: bool) {
        if !hit {
            self.misses += 1;
        }
        // Without a loopback nothing comes back: stop early instead of waiting for every click.
        if (self.misses >= 2 && self.hit_count == 0) || self.pings >= PINGS {
            self.outcome = Some(if self.hit_count == 0 {
                ProbeResult::NoSignal
            } else {
                ProbeResult::Done {
                    round_trips: self.hits,
                    count: self.hit_count,
                }
            });
            return;
        }
        self.phase = Phase::Gap;
        self.t = 0;
    }
}

/// Why a measurement gave no figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The input was too loud to hear the clicks.
    Noisy,
    /// Nothing came back: no loopback cable, or the wrong input.
    NoSignal,
    /// The clicks came back with scattered delays.
    Inconsistent,
}

/// A finished measurement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    /// Output → input round trip outside the engine, in ms.
    pub round_trip_ms: f64,
    /// Round trip plus the engine's own latency: string to ear, in ms.
    pub total_ms: f64,
    /// Clicks that came back with a consistent delay, out of [`PINGS`].
    pub hits: usize,
}

/// Clicks must agree to within this much to count as one measurement.
const TOLERANCE_MS: f64 = 3.0;
const MIN_HITS: usize = 3;

/// Turns the probe's raw round trips (in frames) into a measurement.
pub fn summarise(
    result: ProbeResult,
    sample_rate: f64,
    engine_latency_samples: usize,
) -> Result<Measurement, Failure> {
    let (round_trips, count) = match result {
        ProbeResult::Done { round_trips, count } => (round_trips, count),
        ProbeResult::Noisy => return Err(Failure::Noisy),
        ProbeResult::NoSignal => return Err(Failure::NoSignal),
    };
    let mut ms: Vec<f64> = round_trips[..count]
        .iter()
        .map(|&f| f64::from(f) / sample_rate * 1000.0)
        .collect();
    ms.sort_by(f64::total_cmp);
    let median = ms.get(ms.len() / 2).copied().unwrap_or(0.0);
    let agreeing: Vec<f64> = ms
        .into_iter()
        .filter(|v| (v - median).abs() <= TOLERANCE_MS)
        .collect();
    if agreeing.len() < MIN_HITS {
        return Err(Failure::Inconsistent);
    }
    let round_trip_ms = agreeing.iter().sum::<f64>() / agreeing.len() as f64;
    Ok(Measurement {
        round_trip_ms,
        total_ms: round_trip_ms + engine_latency_samples as f64 / sample_rate * 1000.0,
        hits: agreeing.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 128;

    /// Runs the probe against a simulated cable: what it outputs comes back `delay` frames
    /// later, scaled by `gain`, plus `noise(t)`. Mirrors `web/src/audio/probe.test.ts`.
    fn run(delay: usize, gain: f32, noise: impl Fn(usize) -> f32) -> ProbeResult {
        let mut probe = LoopbackProbe::new(SR);
        let mut line = vec![0.0_f32; delay + BLOCK];
        let n = line.len();
        let mut t = 0;
        for _ in 0..10 * (SR as usize / BLOCK) {
            let input: Vec<f32> = (0..BLOCK)
                .map(|i| line[(t + i) % n] + noise(t + i))
                .collect();
            for (i, &x) in input.iter().enumerate() {
                let y = probe.tick(x);
                line[(t + i) % n] = 0.0;
                line[(t + i + delay) % n] = y * gain;
            }
            t += BLOCK;
            if probe.result().is_some() {
                break;
            }
        }
        probe.result().expect("the probe did not finish")
    }

    #[test]
    fn measures_the_round_trip_to_the_frame() {
        for delay in [128, 129, 255, 1000, 2411] {
            assert_eq!(
                run(delay, 0.3, |_| 0.0),
                ProbeResult::Done {
                    round_trips: [delay as u32; PINGS],
                    count: PINGS
                }
            );
        }
    }

    #[test]
    fn stops_early_when_nothing_comes_back() {
        assert_eq!(run(500, 0.0, |_| 0.0), ProbeResult::NoSignal);
    }

    #[test]
    fn refuses_a_noisy_input() {
        assert_eq!(run(500, 0.3, |_| 0.2), ProbeResult::Noisy);
    }

    #[test]
    fn refuses_a_note_that_starts_ringing_after_the_first_click() {
        let late_note = |t: usize| {
            if t > SR as usize {
                0.3 * (std::f32::consts::TAU * 110.0 * t as f32 / SR).sin()
            } else {
                0.0
            }
        };
        assert_eq!(run(500, 0.3, late_note), ProbeResult::Noisy);
    }

    #[test]
    fn hears_a_quiet_loopback_above_a_low_noise_floor() {
        let hum = |t: usize| 0.004 * (std::f32::consts::TAU * 50.0 * t as f32 / SR).sin();
        let ProbeResult::Done { round_trips, count } = run(700, 0.05, hum) else {
            panic!("expected a result");
        };
        assert!(round_trips[..count].iter().all(|&f| f == 700));
    }

    #[test]
    fn restart_runs_a_second_measurement() {
        let mut probe = LoopbackProbe::new(SR);
        while probe.result().is_none() {
            probe.tick(0.0);
        }
        assert_eq!(probe.result(), Some(ProbeResult::NoSignal));
        probe.restart();
        assert_eq!(probe.result(), None);
    }

    fn done(frames: &[u32]) -> ProbeResult {
        let mut round_trips = [0; PINGS];
        round_trips[..frames.len()].copy_from_slice(frames);
        ProbeResult::Done {
            round_trips,
            count: frames.len(),
        }
    }

    #[test]
    fn summarise_adds_the_engine_latency() {
        let m = summarise(done(&[960, 960, 961, 960]), 48_000.0, 144).unwrap();
        assert!((m.round_trip_ms - 20.005).abs() < 1e-3);
        assert!((m.total_ms - 23.005).abs() < 1e-3);
        assert_eq!(m.hits, 4);
    }

    #[test]
    fn summarise_drops_outliers_and_rejects_scattered_results() {
        let ok = summarise(done(&[960, 962, 5000, 961]), 48_000.0, 0).unwrap();
        assert_eq!(ok.hits, 3);
        let bad = summarise(done(&[500, 2000, 9000, 20000]), 48_000.0, 0);
        assert_eq!(bad, Err(Failure::Inconsistent));
    }

    #[test]
    fn summarise_passes_failures_through() {
        assert_eq!(
            summarise(ProbeResult::Noisy, 48_000.0, 0),
            Err(Failure::Noisy)
        );
    }
}
