//! Engine-level behaviour: safety, robustness, custom IRs and cabinets.

mod common;

use common::*;
use klirr_dsp::cabinet::{generate_ir, magnitude_at};
use klirr_dsp::limiter::CEILING_DB;
use klirr_dsp::util::db_to_gain;
use klirr_dsp::{Cabinet, CustomIr, Engine, IrError, Param};

#[test]
fn limiter_keeps_output_under_the_ceiling() {
    let mut settings = dry_amp(2.0, 1.0);
    settings.push((Param::MasterDb, 12.0));
    settings.push((Param::InputGainDb, 12.0));
    let mut engine = engine_with(&settings);
    let out = render(&mut engine, &sine(330.0, 0.9, secs(1.0)));
    assert!(
        peak(&out) <= db_to_gain(CEILING_DB) + 1e-6,
        "peak {}",
        peak(&out)
    );
    assert!(engine.take_meters().limiter_reduction_db > 1.0);
}

#[test]
fn invalid_input_never_reaches_the_output() {
    let mut engine = engine_with(&[(Param::Channel, 2.0), (Param::ReverbMix, 0.5)]);
    let mut input = sine(220.0, 0.3, secs(0.5));
    input[100] = f32::NAN;
    input[2000] = f32::INFINITY;
    input[3000] = f32::NEG_INFINITY;
    let out = render(&mut engine, &input);
    assert!(out.iter().all(|v| v.is_finite()));
    assert!(
        rms(&out[secs(0.4)..]) > 1e-3,
        "engine must recover and keep playing"
    );
}

#[test]
fn nan_parameters_are_ignored() {
    let mut engine = Engine::new(SR);
    engine.set_param(Param::Gain, 0.8);
    engine.set_param(Param::Gain, f32::NAN);
    assert_eq!(engine.param(Param::Gain), 0.8);
}

#[test]
fn silence_in_gives_silence_out() {
    let mut engine = engine_with(&[(Param::Channel, 2.0), (Param::Gain, 1.0)]);
    let out = render(&mut engine, &vec![0.0; secs(1.0)]);
    assert!(out.iter().all(|&v| v == 0.0));
}

#[test]
fn bypass_passes_the_dry_signal() {
    let mut engine = engine_with(&[(Param::Bypass, 1.0), (Param::Channel, 2.0)]);
    let input = sine(440.0, 0.5, secs(0.2));
    let out = render(&mut engine, &input);
    assert_eq!(out, input);
}

#[test]
fn built_in_cabinets_roll_off_the_fizz() {
    for cab in [Cabinet::Combo1x12, Cabinet::Stack4x12] {
        let ir = generate_ir(cab, SR);
        let mid = magnitude_at(&ir, 1000.0, SR);
        let fizz = magnitude_at(&ir, 10_000.0, SR);
        let drop_db = 20.0 * (mid / fizz).log10();
        assert!(
            drop_db > 20.0,
            "{cab:?}: only {drop_db:.1} dB less at 10 kHz"
        );
        let rumble = magnitude_at(&ir, 30.0, SR);
        assert!(rumble < mid * 0.3, "{cab:?}: too much sub-bass");
    }
}

#[test]
fn custom_ir_is_loaded_resampled_and_used() {
    let mut engine = engine_with(&[(Param::Cabinet, 3.0)]);
    // A simple decaying IR at 44.1 kHz.
    let ir: Vec<f32> = (0..2000)
        .map(|n| (-(n as f32) / 200.0).exp() * if n % 2 == 0 { 1.0 } else { 0.5 })
        .collect();
    assert_eq!(engine.load_custom_ir(&ir, 44_100.0), Ok(()));
    let out = render(&mut engine, &plucks(secs(0.5)));
    assert!(rms(&out) > 1e-3);
}

#[test]
fn a_prepared_ir_swaps_in_and_hands_back_the_previous_one() {
    let input = plucks(secs(0.5));
    let ir: Vec<f32> = (0..2000).map(|n| (-(n as f32) / 200.0).exp()).collect();
    let mut loaded = engine_with(&[(Param::Cabinet, 3.0)]);
    loaded.load_custom_ir(&ir, 44_100.0).unwrap();
    loaded.reset();
    let expected = render(&mut loaded, &input);

    let mut engine = engine_with(&[(Param::Cabinet, 3.0)]);
    let mut prepared = CustomIr::new(&ir, 44_100.0, SR).unwrap();
    assert!(engine.swap_custom_ir(&mut prepared));
    engine.reset();
    assert_eq!(render(&mut engine, &input), expected);

    // The previous IR comes back: swapping it in again restores it.
    let mut other = CustomIr::new(&[1.0, 0.5, 0.25], SR, SR).unwrap();
    assert!(engine.swap_custom_ir(&mut other));
    assert!(engine.swap_custom_ir(&mut other));
    engine.reset();
    assert_eq!(render(&mut engine, &input), expected);
}

#[test]
fn an_ir_prepared_for_another_rate_is_refused() {
    let mut engine = Engine::new(SR);
    let mut ir = CustomIr::new(&[1.0, 0.5], SR, 44_100.0).unwrap();
    assert!(!engine.swap_custom_ir(&mut ir));
    assert_eq!(ir.sample_rate(), 44_100.0);
}

#[test]
fn bad_custom_irs_are_rejected() {
    let mut engine = Engine::new(SR);
    assert_eq!(engine.load_custom_ir(&[], SR), Err(IrError::Empty));
    assert_eq!(engine.load_custom_ir(&[0.0; 100], SR), Err(IrError::Silent));
    assert_eq!(
        engine.load_custom_ir(&[1.0, f32::NAN], SR),
        Err(IrError::NotFinite)
    );
    assert_eq!(
        engine.load_custom_ir(&[1.0], 0.0),
        Err(IrError::InvalidSampleRate)
    );
}

#[test]
fn custom_cabinet_without_an_ir_falls_back_to_no_cabinet() {
    let input = plucks(secs(0.5));
    let custom = render(&mut engine_with(&[(Param::Cabinet, 3.0)]), &input);
    let off = render(&mut engine_with(&[(Param::Cabinet, 0.0)]), &input);
    assert_eq!(custom, off);
}

#[test]
fn common_sample_rates_work() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut engine = Engine::new(sr);
        engine.set_param(Param::ReverbMix, 0.3);
        let input: Vec<f32> = (0..(sr as usize / 2))
            .map(|n| 0.3 * (2.0 * std::f32::consts::PI * 220.0 * n as f32 / sr).sin())
            .collect();
        let mut out = vec![0.0; input.len()];
        engine.process(&input, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
        assert!(rms(&out) > 1e-3, "{sr} Hz produced no output");
    }
}

#[test]
fn meters_report_levels() {
    let mut engine = engine_with(&[]);
    render(&mut engine, &sine(220.0, 0.5, secs(0.2)));
    let m = engine.take_meters();
    assert!((m.input_peak - 0.5).abs() < 0.01);
    assert!(m.output_peak > 0.0);
    let m2 = engine.take_meters();
    assert_eq!(m2.input_peak, 0.0, "meters reset after reading");
}
