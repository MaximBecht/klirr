//! Measure how much faster than real time the engine runs on this machine.
//!
//! ```text
//! cargo run --release --example bench
//! ```
//!
//! Uses the heaviest realistic setting: high gain, 4×12 cabinet, delay and reverb on, processed
//! in 128-sample blocks like an `AudioWorklet`.

use std::time::Instant;

use klirr_dsp::{Engine, Param};

const SR: f32 = 48_000.0;
const BLOCK: usize = 128;
const SECONDS: usize = 30;

fn main() {
    let mut engine = Engine::new(SR);
    for (p, v) in [
        (Param::GateThresholdDb, -60.0),
        (Param::Channel, 2.0),
        (Param::Gain, 0.8),
        (Param::BassDb, 3.0),
        (Param::TrebleDb, 2.0),
        (Param::Cabinet, 2.0),
        (Param::DelayMix, 0.3),
        (Param::ReverbMix, 0.3),
    ] {
        engine.set_param(p, v);
    }
    engine.reset();

    let len = SR as usize * SECONDS;
    let input: Vec<f32> = (0..len)
        .map(|n| {
            let t = (n % 24_000) as f32 / SR;
            0.3 * (-t * 5.0).exp() * (2.0 * std::f32::consts::PI * 110.0 * t).sin()
        })
        .collect();
    let mut output = vec![0.0; BLOCK];

    let mut worst_block = 0.0_f64;
    let start = Instant::now();
    for block in input.chunks(BLOCK) {
        let t = Instant::now();
        engine.process(block, &mut output[..block.len()]);
        worst_block = worst_block.max(t.elapsed().as_secs_f64());
    }
    let elapsed = start.elapsed().as_secs_f64();

    let budget = BLOCK as f64 / f64::from(SR);
    println!("Processed {SECONDS} s of audio in {:.3} s", elapsed);
    println!(
        "Real-time factor: {:.0}× (CPU load ≈ {:.1} %)",
        SECONDS as f64 / elapsed,
        elapsed / SECONDS as f64 * 100.0
    );
    println!(
        "Per {BLOCK}-sample block: average {:.1} µs, worst {:.1} µs, budget {:.0} µs",
        elapsed / (len / BLOCK) as f64 * 1e6,
        worst_block * 1e6,
        budget * 1e6
    );
}
