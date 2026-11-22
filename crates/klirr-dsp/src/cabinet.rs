//! Speaker cabinet simulation.
//!
//! The built-in cabinets are impulse responses generated in code from a filter model of a
//! guitar speaker, so the project ships no third-party IR files. A custom IR (`.wav` decoded
//! by the host) can be loaded as well.
//!
//! Switching cabinets crossfades between them, and every path has the same latency.

use std::fmt;

use crate::biquad::{Biquad, Coeffs};
use crate::convolver::{BLOCK, Convolver};
use crate::smooth::Smoothed;

/// Length of the generated impulse responses, in seconds.
const GENERATED_IR_SECONDS: f32 = 0.045;
/// Custom impulse responses are truncated to this length, in seconds.
pub const MAX_IR_SECONDS: f32 = 0.5;
const CROSSFADE_MS: f32 = 8.0;

/// Cabinet selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Cabinet {
    /// No cabinet (for a real cab or an external IR loader).
    Off = 0,
    /// Open-back 1×12 combo: looser low end, more air.
    Combo1x12 = 1,
    /// Closed-back 4×12 stack: tight thump, strong upper-mid bite.
    Stack4x12 = 2,
    /// The impulse response loaded with [`crate::Engine::load_custom_ir`] or
    /// [`crate::Engine::swap_custom_ir`].
    Custom = 3,
}

impl Cabinet {
    /// Maps a parameter value (rounded) to a cabinet.
    pub fn from_value(value: f32) -> Self {
        match value.round() as i32 {
            i32::MIN..=0 => Self::Off,
            1 => Self::Combo1x12,
            2 => Self::Stack4x12,
            _ => Self::Custom,
        }
    }
}

/// Why a custom impulse response was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrError {
    /// The impulse response has no samples.
    Empty,
    /// It contains NaN or infinite values.
    NotFinite,
    /// The given sample rate is not a positive number.
    InvalidSampleRate,
    /// It is all (near) silence.
    Silent,
}

impl fmt::Display for IrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "the impulse response is empty",
            Self::NotFinite => "the impulse response contains invalid (NaN or infinite) samples",
            Self::InvalidSampleRate => "the impulse response sample rate is invalid",
            Self::Silent => "the impulse response is silent",
        })
    }
}

impl std::error::Error for IrError {}

/// Filter model of a cabinet, as a chain of biquads.
fn model(cab: Cabinet, sr: f32) -> Vec<Coeffs> {
    match cab {
        Cabinet::Combo1x12 => vec![
            Coeffs::highpass(75.0, 0.8, sr),
            Coeffs::peaking(120.0, 1.4, 3.0, sr),
            Coeffs::peaking(500.0, 1.0, -2.0, sr),
            Coeffs::peaking(2600.0, 1.6, 4.0, sr),
            Coeffs::lowpass(5500.0, 0.9, sr),
            Coeffs::lowpass(7000.0, 0.6, sr),
        ],
        Cabinet::Stack4x12 => vec![
            Coeffs::highpass(85.0, 1.0, sr),
            Coeffs::peaking(105.0, 1.3, 5.0, sr),
            Coeffs::peaking(400.0, 1.0, -3.0, sr),
            Coeffs::peaking(2200.0, 2.0, 5.0, sr),
            Coeffs::peaking(3600.0, 3.0, 2.5, sr),
            Coeffs::lowpass(4800.0, 1.0, sr),
            Coeffs::lowpass(5600.0, 0.7, sr),
        ],
        Cabinet::Off | Cabinet::Custom => Vec::new(),
    }
}

/// Generates the impulse response of a built-in cabinet at `sample_rate`.
///
/// Returns an empty vector for [`Cabinet::Off`] and [`Cabinet::Custom`].
pub fn generate_ir(cab: Cabinet, sample_rate: f32) -> Vec<f32> {
    let stages = model(cab, sample_rate);
    if stages.is_empty() {
        return Vec::new();
    }
    let len = (GENERATED_IR_SECONDS * sample_rate) as usize;
    let mut filters: Vec<Biquad> = stages.into_iter().map(Biquad::new).collect();
    let mut ir: Vec<f32> = (0..len)
        .map(|n| {
            let x = if n == 0 { 1.0 } else { 0.0 };
            filters.iter_mut().fold(x, |v, f| f.process(v))
        })
        .collect();
    fade_out(&mut ir, 0.25);
    normalise(&mut ir, sample_rate);
    ir
}

/// Validates, resamples, truncates and normalises a custom impulse response.
pub fn prepare_custom_ir(
    samples: &[f32],
    ir_sample_rate: f32,
    sample_rate: f32,
) -> Result<Vec<f32>, IrError> {
    if samples.is_empty() {
        return Err(IrError::Empty);
    }
    if !(ir_sample_rate.is_finite() && ir_sample_rate > 0.0) {
        return Err(IrError::InvalidSampleRate);
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err(IrError::NotFinite);
    }
    let ratio = f64::from(ir_sample_rate) / f64::from(sample_rate);
    let max_len = (MAX_IR_SECONDS * sample_rate) as usize;
    let len = ((samples.len() as f64 / ratio) as usize).clamp(1, max_len);
    let mut ir: Vec<f32> = (0..len)
        .map(|n| {
            let pos = n as f64 * ratio;
            let i = pos as usize;
            let frac = (pos - i as f64) as f32;
            let a = samples.get(i).copied().unwrap_or(0.0);
            let b = samples.get(i + 1).copied().unwrap_or(0.0);
            a + (b - a) * frac
        })
        .collect();
    if ir.iter().all(|s| s.abs() < 1e-6) {
        return Err(IrError::Silent);
    }
    fade_out(&mut ir, 0.1);
    normalise(&mut ir, sample_rate);
    Ok(ir)
}

/// A custom impulse response, prepared for an engine at one sample rate.
///
/// Preparing allocates, so a host that loads IRs while audio runs builds this off the audio
/// thread and hands it over with [`crate::Engine::swap_custom_ir`], which does not allocate.
#[derive(Debug)]
pub struct CustomIr {
    convolver: Convolver,
    sample_rate: f32,
}

impl CustomIr {
    /// Validates, resamples, truncates and normalises `samples` (at `ir_sample_rate`) for an
    /// engine running at `sample_rate`. Allocates.
    pub fn new(samples: &[f32], ir_sample_rate: f32, sample_rate: f32) -> Result<Self, IrError> {
        let ir = prepare_custom_ir(samples, ir_sample_rate, sample_rate)?;
        Ok(Self {
            convolver: Convolver::new(&ir),
            sample_rate,
        })
    }

    /// The engine sample rate this IR was prepared for.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
}

/// Applies a raised-cosine fade over the last `fraction` of the IR (no truncation click).
fn fade_out(ir: &mut [f32], fraction: f32) {
    let n = ir.len();
    let fade = ((n as f32 * fraction) as usize).max(1).min(n);
    for (i, s) in ir[n - fade..].iter_mut().enumerate() {
        let t = (i + 1) as f32 / fade as f32;
        *s *= 0.5 + 0.5 * (std::f32::consts::PI * t).cos();
    }
}

/// Scales the IR to unity average gain in the guitar's core range, so cabinets (and custom
/// IRs) sound equally loud.
fn normalise(ir: &mut [f32], sample_rate: f32) {
    let freqs = [300.0_f32, 600.0, 1200.0, 2400.0];
    let mean: f32 = freqs
        .iter()
        .map(|&f| magnitude_at(ir, f, sample_rate))
        .sum::<f32>()
        / freqs.len() as f32;
    if mean > 1e-9 {
        for s in ir.iter_mut() {
            *s /= mean;
        }
    }
}

/// Magnitude of the IR's frequency response at `freq` (single-bin DFT).
pub fn magnitude_at(ir: &[f32], freq: f32, sample_rate: f32) -> f32 {
    let w = 2.0 * std::f64::consts::PI * f64::from(freq) / f64::from(sample_rate);
    let (re, im) = ir
        .iter()
        .enumerate()
        .fold((0.0_f64, 0.0_f64), |(re, im), (n, &h)| {
            let ph = w * n as f64;
            (re + f64::from(h) * ph.cos(), im - f64::from(h) * ph.sin())
        });
    (re * re + im * im).sqrt() as f32
}

/// The cabinet stage: built-in cabinets, a custom slot and a bypass path, crossfaded.
#[derive(Debug)]
pub struct CabinetStage {
    /// Slots in [`Cabinet`] index order: Off (delay only), Combo, Stack, Custom.
    convolvers: [Convolver; 3],
    bypass_delay: [f32; BLOCK],
    bypass_pos: usize,
    weights: [Smoothed; 4],
    selected: Cabinet,
    has_custom: bool,
}

impl CabinetStage {
    /// Creates the stage with `selected` active. Allocates.
    pub fn new(selected: Cabinet, sample_rate: f32) -> Self {
        let weights = std::array::from_fn(|_| Smoothed::new(0.0, CROSSFADE_MS, sample_rate));
        let mut stage = Self {
            convolvers: [
                Convolver::new(&generate_ir(Cabinet::Combo1x12, sample_rate)),
                Convolver::new(&generate_ir(Cabinet::Stack4x12, sample_rate)),
                Convolver::new(&[]),
            ],
            bypass_delay: [0.0; BLOCK],
            bypass_pos: 0,
            weights,
            selected,
            has_custom: false,
        };
        stage.select(selected);
        let active = stage.active_slot();
        stage.weights[active].set_immediate(1.0);
        stage
    }

    /// Replaces the custom IR (already prepared with [`prepare_custom_ir`]). Allocates.
    pub fn set_custom_ir(&mut self, ir: &[f32]) {
        self.convolvers[2] = Convolver::new(ir);
        self.has_custom = true;
        self.select(self.selected);
    }

    /// Exchanges the custom IR with `ir`: afterwards `ir` holds the previous one (an empty
    /// convolver if there was none). Real-time safe: nothing is allocated or freed.
    pub fn swap_custom_ir(&mut self, ir: &mut CustomIr) {
        std::mem::swap(&mut self.convolvers[2], &mut ir.convolver);
        self.has_custom = true;
        self.select(self.selected);
    }

    /// Latency of the stage in samples (identical for all slots).
    pub const LATENCY: usize = BLOCK;

    fn active_slot(&self) -> usize {
        match self.selected {
            Cabinet::Custom if !self.has_custom => 0,
            cab => cab as usize,
        }
    }

    /// Selects a cabinet; the change is crossfaded.
    pub fn select(&mut self, cab: Cabinet) {
        self.selected = cab;
        let active = self.active_slot();
        for (i, w) in self.weights.iter_mut().enumerate() {
            w.set_target(if i == active { 1.0 } else { 0.0 });
        }
    }

    /// Clears the running state and jumps to the selected cabinet.
    pub fn reset(&mut self) {
        for c in &mut self.convolvers {
            c.reset();
        }
        self.bypass_delay = [0.0; BLOCK];
        self.bypass_pos = 0;
        for w in &mut self.weights {
            let t = w.target();
            w.set_immediate(t);
        }
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let delayed = self.bypass_delay[self.bypass_pos];
        self.bypass_delay[self.bypass_pos] = x;
        self.bypass_pos = (self.bypass_pos + 1) % BLOCK;

        let mut out = 0.0;
        let w0 = self.weights[0].tick();
        out += w0 * delayed;
        // Every convolver keeps running, even when silent, so a cabinet that fades in is
        // already "warmed up" and the switch has no start-up transient. Cheap at these lengths.
        let has_custom = self.has_custom;
        for (i, conv) in self.convolvers.iter_mut().enumerate() {
            if i == 2 && !has_custom {
                continue;
            }
            let g = self.weights[i + 1].tick();
            out += g * conv.process(x);
        }
        out
    }
}
