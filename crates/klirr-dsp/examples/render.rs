//! Render a guitar recording through the Klirr engine, to listen to the DSP without a UI.
//!
//! ```text
//! cargo run --release --example render -- <input.wav> <output.wav> [name=value ...]
//! ```
//!
//! Record your guitar "dry" (DI, straight from the interface) to a `.wav` file in the `audio/`
//! folder (ignored by git), then e.g.:
//!
//! ```text
//! cargo run --release --example render -- audio/di.wav audio/out.wav channel=2 gain=0.7 cabinet=2 reverb_mix=0.2
//! ```
//!
//! Parameter names are the `name` fields of `Param::info()` (`gain`, `bass_db`, `channel`, ...).
//! Use `input_channel=N` to pick a channel of a multi-channel file (default 0) and `ir=path.wav`
//! to load a custom cabinet IR (also select it with `cabinet=3`).

use std::error::Error;
use std::process::ExitCode;

use klirr_dsp::{Engine, Param};

fn read_mono(path: &str, channel: usize) -> Result<(Vec<f32>, u32), Box<dyn Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    if channel >= channels {
        return Err(format!("{path} has {channels} channel(s), asked for {channel}").into());
    }
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    let mono = interleaved
        .chunks(channels)
        .map(|frame| frame[channel])
        .collect();
    Ok((mono, spec.sample_rate))
}

fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let [input, output, settings @ ..] = args else {
        return Err("usage: render <input.wav> <output.wav> [name=value ...]".into());
    };

    let mut input_channel = 0;
    let mut ir_path = None;
    let mut params = Vec::new();
    for setting in settings {
        let (name, value) = setting
            .split_once('=')
            .ok_or_else(|| format!("expected name=value, got {setting:?}"))?;
        match name {
            "input_channel" => input_channel = value.parse()?,
            "ir" => ir_path = Some(value.to_owned()),
            _ => {
                let param = Param::ALL
                    .into_iter()
                    .find(|p| p.info().name == name)
                    .ok_or_else(|| format!("unknown parameter {name:?}"))?;
                params.push((param, value.parse::<f32>()?));
            }
        }
    }

    let (samples, sample_rate) = read_mono(input, input_channel)?;
    let mut engine = Engine::new(sample_rate as f32);
    if let Some(path) = ir_path {
        let (ir, ir_rate) = read_mono(&path, 0)?;
        engine.load_custom_ir(&ir, ir_rate as f32)?;
    }
    for (param, value) in params {
        engine.set_param(param, value);
    }
    engine.reset();

    let mut out = vec![0.0; samples.len()];
    for (i, o) in samples.chunks(128).zip(out.chunks_mut(128)) {
        engine.process(i, o);
    }
    let meters = engine.take_meters();

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(output, spec)?;
    for s in out {
        writer.write_sample(s)?;
    }
    writer.finalize()?;

    println!(
        "Rendered {:.1} s at {sample_rate} Hz → {output}",
        samples.len() as f32 / sample_rate as f32
    );
    println!(
        "Input peak {:.1} dBFS, output peak {:.1} dBFS, limiter reduction {:.1} dB, latency {} samples",
        klirr_dsp::util::gain_to_db(meters.input_peak),
        klirr_dsp::util::gain_to_db(meters.output_peak),
        meters.limiter_reduction_db,
        engine.latency_samples()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
