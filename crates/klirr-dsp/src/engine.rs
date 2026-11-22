//! The complete amp: all stages chained, with parameters and meters.

use crate::amp::{AmpChannel, Channel};
use crate::biquad::{Biquad, Coeffs};
use crate::cabinet::{Cabinet, CabinetStage, IrError, prepare_custom_ir};
use crate::delay::Delay;
use crate::gate::Gate;
use crate::limiter::Limiter;
use crate::params::Param;
use crate::reverb::Reverb;
use crate::smooth::Smoothed;
use crate::tone::ToneStack;
use crate::util::{db_to_gain, gain_to_db};

const GAIN_SMOOTH_MS: f32 = 20.0;
const CHANNEL_CROSSFADE_MS: f32 = 10.0;
/// A newly selected channel first runs silently for this long, so its filters have settled
/// before it becomes audible (otherwise the switch would click).
const CHANNEL_WARMUP_MS: f32 = 10.0;
const BYPASS_FADE_MS: f32 = 10.0;
/// High-pass before the amp; individual channels add tighter filters of their own.
const INPUT_HIGHPASS_HZ: f32 = 70.0;

/// Levels measured since the last [`Engine::take_meters`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Meters {
    /// Peak input level after the trim, linear (1.0 = 0 dBFS).
    pub input_peak: f32,
    /// Peak output level, linear.
    pub output_peak: f32,
    /// Whether the noise gate is currently open.
    pub gate_open: bool,
    /// Largest gain reduction applied by the safety limiter, in dB (0 = not engaged).
    pub limiter_reduction_db: f32,
}

/// The Klirr amp engine (mono in, mono out).
#[derive(Debug)]
pub struct Engine {
    sample_rate: f32,
    values: [f32; Param::COUNT],
    input_gain: Smoothed,
    gate: Gate,
    highpass: Biquad,
    channels: [AmpChannel; 3],
    channel_weights: [Smoothed; 3],
    active_channel: usize,
    /// Channel being warmed up before the crossfade, with the samples left.
    pending_channel: Option<(usize, u32)>,
    warmup_samples: u32,
    gain: Smoothed,
    tone: ToneStack,
    cabinet: CabinetStage,
    master: Smoothed,
    delay: Delay,
    reverb: Reverb,
    limiter: Limiter,
    bypass: Smoothed,
    input_peak: f32,
    output_peak: f32,
}

impl Engine {
    /// Creates an engine with default settings. Allocates.
    ///
    /// # Panics
    ///
    /// If `sample_rate` is not a finite number of at least 8 kHz.
    pub fn new(sample_rate: f32) -> Self {
        assert!(
            sample_rate.is_finite() && sample_rate >= 8000.0,
            "unsupported sample rate {sample_rate}"
        );
        let sr = sample_rate;
        let mut engine = Self {
            sample_rate: sr,
            values: [0.0; Param::COUNT],
            input_gain: Smoothed::new(1.0, GAIN_SMOOTH_MS, sr),
            gate: Gate::new(crate::gate::OFF_DB, sr),
            highpass: Biquad::new(Coeffs::highpass(INPUT_HIGHPASS_HZ, 0.6, sr)),
            channels: Channel::ALL.map(|c| AmpChannel::new(c, sr)),
            channel_weights: std::array::from_fn(|_| Smoothed::new(0.0, CHANNEL_CROSSFADE_MS, sr)),
            active_channel: 0,
            pending_channel: None,
            warmup_samples: (CHANNEL_WARMUP_MS / 1000.0 * sr) as u32,
            gain: Smoothed::new(0.5, GAIN_SMOOTH_MS, sr),
            tone: ToneStack::new(sr),
            cabinet: CabinetStage::new(Cabinet::Off, sr),
            master: Smoothed::new(1.0, GAIN_SMOOTH_MS, sr),
            delay: Delay::new(sr),
            reverb: Reverb::new(sr),
            limiter: Limiter::new(sr),
            bypass: Smoothed::new(0.0, BYPASS_FADE_MS, sr),
            input_peak: 0.0,
            output_peak: 0.0,
        };
        engine.activate_channel(0);
        for p in Param::ALL {
            engine.set_param(p, p.info().default);
        }
        engine.reset();
        engine
    }

    /// The sample rate the engine was created for.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Processing latency in samples (oversampling filters + cabinet convolution).
    pub fn latency_samples(&self) -> usize {
        crate::oversample::LATENCY + CabinetStage::LATENCY
    }

    /// Sets a parameter. Values are clamped to the parameter's range; NaN is ignored.
    /// The change glides in over a few milliseconds.
    pub fn set_param(&mut self, param: Param, value: f32) {
        let Some(v) = param.sanitise(value) else {
            return;
        };
        self.values[param.index()] = v;
        let val = |p: Param| self.values[p.index()];
        match param {
            Param::InputGainDb => self.input_gain.set_target(db_to_gain(v)),
            Param::GateThresholdDb => self.gate.set_threshold_db(v),
            Param::Channel => self.select_channel(Channel::from_value(v) as usize),
            Param::Gain => self.gain.set_target(v),
            Param::BassDb | Param::MidDb | Param::TrebleDb => {
                let (b, m, t) = (val(Param::BassDb), val(Param::MidDb), val(Param::TrebleDb));
                self.tone.set_gains_db(b, m, t);
            }
            Param::Cabinet => self.cabinet.select(Cabinet::from_value(v)),
            Param::MasterDb => self.master.set_target(db_to_gain(v)),
            Param::DelayTimeMs | Param::DelayFeedback | Param::DelayMix => {
                let (t, f, m) = (
                    val(Param::DelayTimeMs),
                    val(Param::DelayFeedback),
                    val(Param::DelayMix),
                );
                self.delay.set(t, f, m);
            }
            Param::ReverbSize | Param::ReverbDamping | Param::ReverbMix => {
                let (s, d, m) = (
                    val(Param::ReverbSize),
                    val(Param::ReverbDamping),
                    val(Param::ReverbMix),
                );
                self.reverb.set(s, d, m);
            }
            Param::Bypass => self.bypass.set_target(v),
        }
    }

    fn select_channel(&mut self, channel: usize) {
        if channel == self.active_channel {
            self.pending_channel = None;
        } else if self.channel_weights[channel].value() > 0.0 {
            // Still audible from a recent switch: its state is warm, crossfade right away.
            self.pending_channel = None;
            self.activate_channel(channel);
        } else {
            self.channels[channel].reset();
            self.pending_channel = Some((channel, self.warmup_samples));
        }
    }

    fn activate_channel(&mut self, channel: usize) {
        self.active_channel = channel;
        for (i, w) in self.channel_weights.iter_mut().enumerate() {
            w.set_target(if i == channel { 1.0 } else { 0.0 });
        }
    }

    /// The current (target) value of a parameter.
    pub fn param(&self, param: Param) -> f32 {
        self.values[param.index()]
    }

    /// Loads a custom cabinet impulse response (mono samples at `ir_sample_rate`).
    ///
    /// The IR is resampled, truncated to [`crate::cabinet::MAX_IR_SECONDS`] and level-matched.
    /// Select it with [`Param::Cabinet`] = 3. **Allocates**: call outside the audio callback.
    pub fn load_custom_ir(&mut self, samples: &[f32], ir_sample_rate: f32) -> Result<(), IrError> {
        let ir = prepare_custom_ir(samples, ir_sample_rate, self.sample_rate)?;
        self.cabinet.set_custom_ir(&ir);
        Ok(())
    }

    /// Clears all audio state (tails, filters) and jumps every parameter to its target.
    pub fn reset(&mut self) {
        if let Some((channel, _)) = self.pending_channel.take() {
            self.activate_channel(channel);
        }
        for s in [
            &mut self.input_gain,
            &mut self.gain,
            &mut self.master,
            &mut self.bypass,
        ]
        .into_iter()
        .chain(self.channel_weights.iter_mut())
        {
            let t = s.target();
            s.set_immediate(t);
        }
        self.gate.reset();
        self.highpass.reset();
        for c in &mut self.channels {
            c.reset();
        }
        self.tone.reset();
        self.cabinet.reset();
        self.delay.reset();
        self.reverb.reset();
        self.limiter.reset();
        self.input_peak = 0.0;
        self.output_peak = 0.0;
    }

    /// Returns the meters accumulated since the previous call and starts a new measurement.
    pub fn take_meters(&mut self) -> Meters {
        let meters = Meters {
            input_peak: self.input_peak,
            output_peak: self.output_peak,
            gate_open: self.gate.is_open(),
            limiter_reduction_db: 0.0 - gain_to_db(self.limiter.take_min_gain()).min(0.0),
        };
        self.input_peak = 0.0;
        self.output_peak = 0.0;
        meters
    }

    /// Processes `input` into `output`. Both must have the same length; if they differ, only
    /// the common length is processed. Real-time safe: no allocation, no locks.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        for (o, &x) in output.iter_mut().zip(input) {
            *o = self.tick(x);
        }
    }

    /// Processes `buffer` in place. Real-time safe.
    pub fn process_in_place(&mut self, buffer: &mut [f32]) {
        for s in buffer.iter_mut() {
            *s = self.tick(*s);
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };

        let trimmed = x * self.input_gain.tick();
        self.input_peak = self.input_peak.max(trimmed.abs());
        let gated = self.gate.process(trimmed);
        let filtered = self.highpass.process(gated);

        let gain = self.gain.tick();
        let mut amp = 0.0;
        for (channel, weight) in self
            .channels
            .iter_mut()
            .zip(self.channel_weights.iter_mut())
        {
            if weight.value() == 0.0 && weight.target() == 0.0 {
                continue;
            }
            let w = weight.tick();
            let (drive, makeup) = channel.drive_and_makeup(gain);
            amp += w * channel.process(filtered, drive, makeup);
        }
        if let Some((index, left)) = self.pending_channel {
            let channel = &mut self.channels[index];
            let (drive, makeup) = channel.drive_and_makeup(gain);
            channel.process(filtered, drive, makeup); // silent warm-up
            if left <= 1 {
                self.pending_channel = None;
                self.activate_channel(index);
            } else {
                self.pending_channel = Some((index, left - 1));
            }
        }

        let toned = self.tone.process(amp);
        let cab = self.cabinet.process(toned);
        let mastered = cab * self.master.tick();
        let echoed = self.delay.process(mastered);
        let wet = self.reverb.process(echoed);
        let limited = self.limiter.process(wet);

        let b = self.bypass.tick();
        let out = (limited * (1.0 - b) + x * b).clamp(-1.0, 1.0);
        if !out.is_finite() {
            // Should be unreachable; never let a bad value poison the state for good.
            self.reset();
            return 0.0;
        }
        self.output_peak = self.output_peak.max(out.abs());
        out
    }
}
