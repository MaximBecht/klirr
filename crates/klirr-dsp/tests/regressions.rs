//! Regression tests for the sound artefacts listed in docs/PLAN.md, section 3.
//! Each test is named after the artefact it guards against.

mod common;

use common::*;
use klirr_dsp::Param;
use klirr_dsp::delay::Delay;
use klirr_dsp::gate::Gate;
use klirr_dsp::reverb::Reverb;

/// Block-rate artefacts: output must not depend on how the host splits audio into blocks.
#[test]
fn output_is_independent_of_block_size() {
    let settings = [
        (Param::GateThresholdDb, -60.0),
        (Param::Channel, 1.0),
        (Param::Gain, 0.7),
        (Param::BassDb, 6.0),
        (Param::MidDb, -4.0),
        (Param::TrebleDb, 3.0),
        (Param::Cabinet, 2.0),
        (Param::DelayTimeMs, 120.0),
        (Param::DelayFeedback, 0.5),
        (Param::DelayMix, 0.4),
        (Param::ReverbMix, 0.3),
    ];
    let input = plucks(secs(1.5));
    let reference = render(&mut engine_with(&settings), &input);
    assert!(rms(&reference) > 1e-3, "test signal must produce output");
    for block in [1, 37, 128, 1024] {
        let out = render_in_blocks(&mut engine_with(&settings), &input, block);
        assert!(out == reference, "block size {block} changed the output");
    }
}

/// Block-rate artefacts: an EQ knob off centre must not cause jumps at block edges.
#[test]
fn tone_stack_has_no_block_edge_jumps() {
    let mut settings = dry_amp(0.0, 0.0);
    settings.push((Param::BassDb, 6.0));
    let input = sine(220.0, 0.3, secs(1.0));
    let out = render_in_blocks(&mut engine_with(&settings), &input, 1024);
    let steady = &out[secs(0.2)..];
    let jumps: Vec<f32> = steady.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let at_edges = jumps
        .iter()
        .enumerate()
        .filter(|(i, _)| (i + secs(0.2) + 1) % 1024 == 0)
        .fold(0.0_f32, |m, (_, &j)| m.max(j));
    let overall = jumps.iter().fold(0.0_f32, |m, &j| m.max(j));
    assert!(
        at_edges <= overall * 1.01,
        "edge jump {at_edges} vs overall {overall}"
    );
}

/// Lost dynamics: quiet and loud playing must not come out equally loud.
#[test]
fn crunch_keeps_playing_dynamics() {
    let settings = dry_amp(1.0, 0.3);
    let quiet = render(&mut engine_with(&settings), &sine(220.0, 0.02, secs(0.5)));
    let loud = render(&mut engine_with(&settings), &sine(220.0, 0.2, secs(0.5)));
    let ratio_db = 20.0 * (rms(&loud[secs(0.1)..]) / rms(&quiet[secs(0.1)..])).log10();
    assert!(
        ratio_db > 6.0,
        "20 dB input difference became only {ratio_db:.1} dB"
    );
}

/// Fizzy distortion: aliasing must stay far below the harmonics, even at maximum gain.
#[test]
fn aliasing_is_far_below_the_harmonics() {
    let f0 = 1760.0;
    // Measured ≈ −72 / −68 / −70 dB (without oversampling: ≈ −22 dB). Limits leave a margin.
    for (channel, limit_db) in [(0.0, -65.0), (1.0, -62.0), (2.0, -62.0)] {
        let input = sine(f0, 0.3, secs(1.2));
        let out = render(&mut engine_with(&dry_amp(channel, 1.0)), &input);
        let db = inharmonic_db(&out[secs(0.2)..], f64::from(f0));
        eprintln!("channel {channel}: inharmonic (aliased) content at {db:.1} dB");
        assert!(
            db < limit_db,
            "channel {channel}: inharmonic content at {db:.1} dB"
        );
    }
}

/// Gate artefacts: it must fade, hold and not chatter on a decaying note.
#[test]
fn gate_fades_smoothly_without_chatter() {
    let threshold_db = -50.0;
    let mut gate = Gate::new(threshold_db, SR);
    // 0.3 s note, then an exponential decay through the threshold into a quiet noise floor.
    let len = secs(2.0);
    let floor = noise(len, 3);
    let input: Vec<f32> = (0..len)
        .map(|n| {
            let t = n as f32 / SR;
            let env = if t < 0.3 {
                0.3
            } else {
                0.3 * (-(t - 0.3) * 8.0).exp()
            };
            env * (2.0 * std::f32::consts::PI * 220.0 * t).sin() + 1e-4 * floor[n]
        })
        .collect();

    let mut prev_gain = gate.gain();
    let mut max_step = 0.0_f32;
    let mut transitions = 0;
    let mut was_open = gate.is_open();
    for &x in &input {
        gate.process(x);
        max_step = max_step.max((gate.gain() - prev_gain).abs());
        prev_gain = gate.gain();
        if gate.is_open() != was_open {
            transitions += 1;
            was_open = gate.is_open();
        }
    }
    assert!(max_step < 0.05, "gain jumped by {max_step} in one sample");
    assert!(
        transitions <= 2,
        "gate chattered: {transitions} open/close changes"
    );
    assert!(
        !gate.is_open() && gate.gain() == 0.0,
        "gate should end closed"
    );
}

/// Gate artefacts: while a note is clearly above the threshold the gate leaves it alone.
#[test]
fn gate_does_not_touch_notes_above_threshold() {
    let mut gate = Gate::new(-50.0, SR);
    for x in sine(220.0, 0.01, secs(0.5)) {
        // 0.01 ≈ −40 dBFS, 10 dB above the threshold
        gate.process(x);
        assert!(gate.gain() > 0.999);
    }
}

/// Runaway feedback: the reverb must always decay, even at maximum size.
#[test]
fn reverb_decays_at_maximum_size() {
    let mut reverb = Reverb::new(SR);
    reverb.set(1.0, 0.0, 1.0);
    reverb.reset();
    let mut out = Vec::with_capacity(secs(10.0));
    for n in 0..secs(10.0) {
        out.push(reverb.process(if n == 0 { 0.5 } else { 0.0 }));
    }
    assert!(out.iter().all(|v| v.is_finite()));
    assert!(peak(&out) < 1.0);
    let first = rms(&out[..secs(1.0)]);
    let last = rms(&out[secs(9.0)..]);
    assert!(last < first * 0.01, "reverb not decaying: {first} → {last}");
}

/// Runaway feedback: delay repeats must die out even at maximum feedback.
#[test]
fn delay_decays_at_maximum_feedback() {
    let mut delay = Delay::new(SR);
    delay.set(50.0, 1.0, 1.0); // feedback is capped below 1
    delay.reset();
    let mut out = Vec::with_capacity(secs(20.0));
    for n in 0..secs(20.0) {
        out.push(delay.process(if n == 0 { 0.5 } else { 0.0 }));
    }
    assert!(out.iter().all(|v| v.is_finite()));
    let first = peak(&out[1..secs(1.0)]);
    let last = peak(&out[secs(19.0)..]);
    assert!(last < first * 0.01, "delay not decaying: {first} → {last}");
}

/// Knob crackle: parameter, channel and cabinet changes must glide without clicks.
#[test]
fn knob_changes_do_not_click() {
    let changes: [(Param, f32, f32); 7] = [
        (Param::BassDb, -12.0, 12.0),
        (Param::MidDb, -12.0, 12.0),
        (Param::TrebleDb, -12.0, 12.0),
        (Param::MasterDb, -40.0, -10.0),
        (Param::Gain, 0.0, 0.6),
        (Param::Cabinet, 1.0, 2.0),
        (Param::Channel, 0.0, 1.0),
    ];
    for (param, from, to) in changes {
        let mut settings = dry_amp(0.0, 0.2);
        settings.push((param, from));
        let mut engine = engine_with(&settings);
        let input = sine(220.0, 0.2, secs(1.5));
        let switch_at = secs(0.5);
        let mut out = render(&mut engine, &input[..switch_at]);
        engine.set_param(param, to);
        out.extend(render(&mut engine, &input[switch_at..]));

        let before = max_second_diff(&out[secs(0.3)..switch_at]);
        let after = max_second_diff(&out[secs(1.2)..]);
        let during = max_second_diff(&out[switch_at - 2..switch_at + secs(0.3)]);
        let limit = before.max(after) * 1.5;
        assert!(
            during <= limit,
            "{param:?} {from} → {to}: {during:.2e} during vs {before:.2e} / {after:.2e} steady"
        );
    }
}

/// DC offset: asymmetric clipping must not leave DC in the output.
#[test]
fn dc_offset_is_removed() {
    let out = render(
        &mut engine_with(&dry_amp(2.0, 1.0)),
        &sine(110.0, 0.3, secs(2.0)),
    );
    let tail = &out[secs(1.0)..];
    let mean = tail.iter().sum::<f32>() / tail.len() as f32;
    assert!(
        mean.abs() < rms(tail) * 1e-3,
        "DC {mean} vs RMS {}",
        rms(tail)
    );
}
