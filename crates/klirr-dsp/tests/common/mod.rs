//! Signal generators and measurements shared by the integration tests.
#![allow(dead_code)]

use klirr_dsp::{Engine, Param};
use realfft::RealFftPlanner;
use std::f32::consts::PI;

pub const SR: f32 = 48_000.0;

/// Seconds to samples.
pub fn secs(s: f32) -> usize {
    (s * SR) as usize
}

pub fn sine(freq: f32, amplitude: f32, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| amplitude * (2.0 * PI * freq * n as f32 / SR).sin())
        .collect()
}

/// Deterministic white noise in −1..1.
pub fn noise(len: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        })
        .collect()
}

/// A guitar-like test signal: plucked notes (decaying harmonics) plus a little noise.
pub fn plucks(len: usize) -> Vec<f32> {
    let notes = [82.4_f32, 110.0, 146.8, 196.0, 246.9, 329.6];
    let note_len = secs(0.35);
    let hiss = noise(len, 7);
    (0..len)
        .map(|n| {
            let note = notes[(n / note_len) % notes.len()];
            let t = (n % note_len) as f32 / SR;
            let env = (-t * 6.0).exp();
            let tone: f32 = (1..=8)
                .map(|h| (2.0 * PI * note * h as f32 * t).sin() / h as f32)
                .sum();
            0.25 * env * tone + 0.001 * hiss[n]
        })
        .collect()
}

/// An engine with `settings` applied and all glides settled.
pub fn engine_with(settings: &[(Param, f32)]) -> Engine {
    let mut e = Engine::new(SR);
    for &(p, v) in settings {
        e.set_param(p, v);
    }
    e.reset();
    e
}

/// The neutral "measure the amp alone" setup: no gate, no cab, no effects, quiet master.
pub fn dry_amp(channel: f32, gain: f32) -> Vec<(Param, f32)> {
    vec![
        (Param::GateThresholdDb, -96.0),
        (Param::Channel, channel),
        (Param::Gain, gain),
        (Param::Cabinet, 0.0),
        (Param::MasterDb, -20.0),
        (Param::DelayMix, 0.0),
        (Param::ReverbMix, 0.0),
    ]
}

pub fn render(engine: &mut Engine, input: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0; input.len()];
    engine.process(input, &mut out);
    out
}

pub fn render_in_blocks(engine: &mut Engine, input: &[f32], block: usize) -> Vec<f32> {
    let mut out = vec![0.0; input.len()];
    for (i, o) in input.chunks(block).zip(out.chunks_mut(block)) {
        engine.process(i, o);
    }
    out
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// Largest second difference: tiny for smooth signals, large at clicks and discontinuities.
pub fn max_second_diff(x: &[f32]) -> f32 {
    x.windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .fold(0.0, f32::max)
}

/// Power spectrum (Blackman-Harris window) with the bin frequencies.
pub fn power_spectrum(x: &[f32]) -> (Vec<f64>, Vec<f64>) {
    let n = x.len();
    let mut buf: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let t = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            let w =
                0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
            f64::from(v) * w
        })
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut spec = fft.make_output_vec();
    fft.process(&mut buf, &mut spec).unwrap();
    let power = spec.iter().map(|c| c.norm_sqr()).collect();
    let freqs = (0..spec.len())
        .map(|k| k as f64 * f64::from(SR) / n as f64)
        .collect();
    (power, freqs)
}

/// Energy away from the harmonics of `f0`, relative to the energy at the harmonics, in dB.
/// Everything that is not a harmonic is aliasing (or noise) for a pure sine input.
pub fn inharmonic_db(x: &[f32], f0: f64) -> f64 {
    let (power, freqs) = power_spectrum(x);
    let (mut harmonic, mut other) = (0.0, 0.0);
    for (p, f) in power.iter().zip(&freqs) {
        if *f < 30.0 {
            continue;
        }
        let h = (f / f0).round();
        if h >= 1.0 && (f - h * f0).abs() < 15.0 {
            harmonic += p;
        } else {
            other += p;
        }
    }
    10.0 * (other / harmonic).log10()
}
