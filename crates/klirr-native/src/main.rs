//! Klirr's native audio host: the same engine as the web app, on the platform's audio API
//! (WASAPI or ASIO on Windows, Core Audio on macOS, ALSA or JACK on Linux).
//!
//! It is the audio side of the planned desktop app and, for now, a command-line tool to
//! compare native latency with the browser's:
//!
//! ```text
//! klirr-native devices [--host NAME]
//! klirr-native measure [device options] [--runs N]
//! klirr-native play    [device options] [name=value ...] [ir=path.wav]
//! ```
//!
//! Device options: `--host NAME`, `--device PART` (input and output), `--input PART`,
//! `--output PART`, `--channel N` (from 1), `--rate HZ`, `--buffer FRAMES`, `--margin FRAMES`.

mod duplex;
mod host;
mod probe;

use std::error::Error;
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SupportedBufferSize};
use klirr_dsp::Param;
use klirr_dsp::util::gain_to_db;

use host::{Duplex, MeasureError, Options};
use probe::{Failure, PINGS};

const USAGE: &str = "\
usage:
  klirr-native devices [--host NAME]
  klirr-native measure [device options] [--runs N]
  klirr-native play    [device options] [name=value ...] [ir=path.wav]

device options:
  --host NAME       audio API: wasapi (default on Windows), asio (build with --features asio),
                    coreaudio, alsa, jack ...
  --device PART     input and output device whose name contains PART
  --input PART      input device (default: the host's default input)
  --output PART     output device (default: the host's default output)
  --channel N       input channel, from 1 (default 1)
  --rate HZ         sample rate (default: the output device's)
  --buffer FRAMES   buffer size to ask the backend for (default: the backend's)
  --margin FRAMES   safety margin between input and output (default: 0 for ASIO, 1 ms otherwise)

measure needs a cable from an output of the interface to the selected input.";

/// Time for the streams to settle before measuring: the first trims happen in it.
const WARMUP: Duration = Duration::from_millis(1500);

struct Args {
    command: String,
    options: Options,
    runs: usize,
    settings: Vec<String>,
}

fn parse(args: &[String]) -> Result<Args, Box<dyn Error>> {
    let mut it = args.iter();
    let command = it.next().ok_or(USAGE)?.clone();
    let mut options = Options::default();
    let mut runs = 3;
    let mut settings = Vec::new();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--host" => options.host = Some(value()?),
            "--device" => {
                let v = value()?;
                options.input = Some(v.clone());
                options.output = Some(v);
            }
            "--input" => options.input = Some(value()?),
            "--output" => options.output = Some(value()?),
            "--channel" => {
                let n: usize = value()?.parse()?;
                options.channel = n.checked_sub(1).ok_or("channels count from 1")?;
            }
            "--rate" => options.rate = Some(value()?.parse()?),
            "--buffer" => options.buffer = Some(value()?.parse()?),
            "--margin" => options.margin = Some(value()?.parse()?),
            "--runs" => runs = value()?.parse::<usize>()?.max(1),
            "-h" | "--help" => return Err(USAGE.into()),
            s if s.contains('=') && !s.starts_with('-') => settings.push(s.to_owned()),
            s => return Err(format!("unknown option {s:?}\n\n{USAGE}").into()),
        }
    }
    Ok(Args {
        command,
        options,
        runs,
        settings,
    })
}

fn ms(frames: f64, sample_rate: u32) -> f64 {
    frames / f64::from(sample_rate) * 1000.0
}

fn frames_and_ms(frames: Option<u32>, sample_rate: u32) -> String {
    match frames {
        Some(f) => format!("{f} frames ({:.2} ms)", ms(f64::from(f), sample_rate)),
        None => "unknown".to_owned(),
    }
}

fn print_info(d: &Duplex, channel: usize) {
    let i = &d.info;
    println!("  Host      {}", i.host);
    println!(
        "  Input     {}: channel {} of {}, {}",
        i.input,
        channel + 1,
        i.input_channels,
        i.input_format
    );
    println!(
        "  Output    {}: {} channel(s), {}",
        i.output, i.output_channels, i.output_format
    );
    println!("  Rate      {} Hz", i.sample_rate);
    println!(
        "  Buffers   input {}, output {}",
        frames_and_ms(i.input_buffer, i.sample_rate),
        frames_and_ms(i.output_buffer, i.sample_rate)
    );
    println!(
        "  Engine    {} samples ({:.2} ms)",
        i.engine_latency,
        ms(i.engine_latency as f64, i.sample_rate)
    );
}

fn describe_failure(e: MeasureError) -> &'static str {
    match e {
        MeasureError::Failed(Failure::NoSignal) => {
            "nothing came back: connect a cable from an output to the selected input \
             (and turn off direct monitoring)"
        }
        MeasureError::Failed(Failure::Noisy) => {
            "the input is too noisy: mute the strings, turn the input gain down"
        }
        MeasureError::Failed(Failure::Inconsistent) => {
            "the clicks came back with scattered delays: check for dropouts"
        }
        MeasureError::Stalled => "the audio stopped running",
    }
}

fn measure(args: &Args) -> Result<(), Box<dyn Error>> {
    println!("Klirr native latency measurement");
    let mut duplex = Duplex::open(&args.options, |_| {})?;
    print_info(&duplex, args.options.channel);
    std::thread::sleep(WARMUP);

    let stats = duplex.stats.clone();
    let rate = duplex.info.sample_rate;
    let n = |c: &std::sync::atomic::AtomicU32| c.load(Ordering::Relaxed);
    println!(
        "  Callbacks input {} frames, output {} frames; {} frames waiting between them \
         (margin {})",
        n(&stats.input_block),
        n(&stats.output_block),
        n(&stats.slack_frames),
        n(&stats.margin_frames)
    );
    let reported = f64::from(n(&stats.input_delay_us)) / 1000.0
        + ms(f64::from(n(&stats.slack_frames)), rate)
        + f64::from(n(&stats.output_delay_us)) / 1000.0
        + ms(duplex.info.engine_latency as f64, rate);
    println!(
        "  Reported  input {:.2} ms + between streams {:.2} ms + output {:.2} ms + engine {:.2} ms \
         = {reported:.2} ms (backend timestamps; misses converters and cable)",
        f64::from(n(&stats.input_delay_us)) / 1000.0,
        ms(f64::from(n(&stats.slack_frames)), rate),
        f64::from(n(&stats.output_delay_us)) / 1000.0,
        ms(duplex.info.engine_latency as f64, rate),
    );
    for c in [
        &stats.underruns,
        &stats.overflows,
        &stats.backend_errors,
        &stats.load_permille,
    ] {
        c.store(0, Ordering::Relaxed);
    }

    let mut totals = Vec::new();
    let mut round_trips = Vec::new();
    for run in 1..=args.runs {
        match duplex.measure() {
            Ok(m) => {
                println!(
                    "  Run {run}     round trip {:.2} ms, string to ear {:.2} ms ({}/{PINGS} clicks)",
                    m.round_trip_ms, m.total_ms, m.hits
                );
                totals.push(m.total_ms);
                round_trips.push(m.round_trip_ms);
            }
            Err(e) => {
                println!("  Run {run}     no figure: {}", describe_failure(e));
                if matches!(
                    e,
                    MeasureError::Failed(Failure::NoSignal) | MeasureError::Stalled
                ) {
                    break;
                }
            }
        }
    }

    println!(
        "  Dropouts  {} underrun(s), {} overflow(s), {} backend error(s) while measuring; \
         peak load {:.1} %",
        n(&stats.underruns),
        n(&stats.overflows),
        n(&stats.backend_errors),
        f64::from(n(&stats.load_permille)) / 10.0
    );
    if totals.is_empty() {
        return Err("no measurement".into());
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let spread = totals.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - totals.iter().copied().fold(f64::INFINITY, f64::min);
    println!();
    println!(
        "String to ear {:.1} ms (round trip {:.1} ms + engine {:.1} ms), median of {} run(s), spread {spread:.1} ms",
        median(&mut totals),
        median(&mut round_trips),
        ms(duplex.info.engine_latency as f64, rate),
        totals.len()
    );
    Ok(())
}

fn read_mono(path: &str) -> Result<(Vec<f32>, u32), Box<dyn Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
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
    Ok((
        interleaved.chunks(channels).map(|f| f[0]).collect(),
        spec.sample_rate,
    ))
}

fn play(args: &Args) -> Result<(), Box<dyn Error>> {
    let mut params = Vec::new();
    let mut ir = None;
    for setting in &args.settings {
        let (name, value) = setting.split_once('=').ok_or("expected name=value")?;
        if name == "ir" {
            ir = Some(read_mono(value)?);
            continue;
        }
        let param = Param::ALL
            .into_iter()
            .find(|p| p.info().name == name)
            .ok_or_else(|| format!("unknown parameter {name:?}"))?;
        params.push((param, value.parse::<f32>()?));
    }

    println!("Klirr native: playing");
    let mut ir_error = None;
    let duplex = Duplex::open(&args.options, |engine| {
        if let Some((samples, rate)) = &ir {
            ir_error = engine.load_custom_ir(samples, *rate as f32).err();
        }
        for &(param, value) in &params {
            engine.set_param(param, value);
        }
    })?;
    if let Some(e) = ir_error {
        return Err(format!("could not load the IR: {e:?}").into());
    }
    print_info(&duplex, args.options.channel);
    println!("Press Enter to stop.");

    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read_line(&mut String::new());
        let _ = stop_tx.send(());
    });
    let s = &duplex.stats;
    let rate = duplex.info.sample_rate;
    while stop_rx.recv_timeout(Duration::from_secs(1)).is_err() {
        let db = |cell| format!("{:6.1}", gain_to_db(duplex::Stats::take(cell)));
        println!(
            "in {} dBFS  out {} dBFS  limiter {:4.1} dB  load {:5.1} %  waiting {:5.2} ms  underruns {}",
            db(&s.input_peak),
            db(&s.output_peak),
            duplex::Stats::take(&s.limiter_db),
            f64::from(s.load_permille.swap(0, Ordering::Relaxed)) / 10.0,
            ms(f64::from(s.slack_frames.load(Ordering::Relaxed)), rate),
            s.underruns.load(Ordering::Relaxed),
        );
    }
    Ok(())
}

fn describe_device(device: &Device, input: bool) -> String {
    let config = if input {
        device.default_input_config()
    } else {
        device.default_output_config()
    };
    match config {
        Ok(c) => {
            let buffer = match c.buffer_size() {
                SupportedBufferSize::Range { min, max } => format!(", buffer {min}–{max} frames"),
                SupportedBufferSize::Unknown => String::new(),
            };
            format!(
                "{} ch, {} Hz, {}{buffer}",
                c.channels(),
                c.sample_rate(),
                c.sample_format()
            )
        }
        Err(e) => format!("no default config ({e})"),
    }
}

fn devices(args: &Args) -> Result<(), Box<dyn Error>> {
    let ids = match &args.options.host {
        Some(name) => vec![host::host(Some(name))?.id()],
        None => cpal::available_hosts(),
    };
    for id in ids {
        let h = cpal::host_from_id(id)?;
        println!("{}", id.name());
        let default_in = h.default_input_device().map(|d| host::name(&d));
        let default_out = h.default_output_device().map(|d| host::name(&d));
        for (label, input, list, default) in [
            (
                "inputs",
                true,
                h.input_devices()?.collect::<Vec<_>>(),
                default_in,
            ),
            ("outputs", false, h.output_devices()?.collect(), default_out),
        ] {
            println!("  {label}:");
            for device in list {
                let n = host::name(&device);
                let mark = if Some(&n) == default.as_ref() {
                    "*"
                } else {
                    " "
                };
                println!("   {mark} {n}  ({})", describe_device(&device, input));
            }
        }
    }
    println!("(* = default)");
    Ok(())
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let result = parse(&raw).and_then(|args| match args.command.as_str() {
        "devices" => devices(&args),
        "measure" => measure(&args),
        "play" => play(&args),
        other => Err(format!("unknown command {other:?}\n\n{USAGE}").into()),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
