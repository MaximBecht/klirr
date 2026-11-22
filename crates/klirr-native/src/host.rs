//! Opening audio devices with cpal: host and device selection, and the two streams that feed
//! the [`Processor`].

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, FromSample, Host, HostId, SampleFormat, SizedSample, Stream, StreamConfig,
};
use klirr_dsp::Engine;

use crate::duplex::{Command, Processor, Stats};
use crate::probe::{self, Failure, Measurement, ProbeResult};

/// Calls a stream builder with the Rust sample type for `format`.
macro_rules! by_format {
    ($format:expr, $what:literal, $f:ident($($arg:expr),*)) => {
        match $format {
            SampleFormat::I16 => $f::<i16>($($arg),*),
            SampleFormat::I24 => $f::<cpal::I24>($($arg),*),
            SampleFormat::I32 => $f::<i32>($($arg),*),
            SampleFormat::F32 => $f::<f32>($($arg),*),
            SampleFormat::F64 => $f::<f64>($($arg),*),
            other => Err(format!("{} sample format {other} is not supported", $what).into()),
        }
    };
}

/// Output callbacks are processed in pieces of at most this many frames.
const MAX_FRAMES: usize = 8192;

/// Which devices to open and how.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Host (audio API) name, e.g. `wasapi` or `asio`. Default: the platform's default host.
    pub host: Option<String>,
    /// Part of the input device's name. Default: the host's default input.
    pub input: Option<String>,
    /// Part of the output device's name. Default: the host's default output.
    pub output: Option<String>,
    /// Input channel, counted from 0.
    pub channel: usize,
    /// Sample rate. Default: the output device's.
    pub rate: Option<u32>,
    /// Buffer size in frames. Default: the backend's.
    pub buffer: Option<u32>,
    /// Safety margin between the streams, in frames. Default: none for ASIO, 1 ms otherwise.
    pub margin: Option<usize>,
}

/// What was opened.
#[derive(Clone, Debug)]
pub struct Info {
    /// Host name.
    pub host: &'static str,
    /// Input device name.
    pub input: String,
    /// Output device name.
    pub output: String,
    /// Channels of the input stream.
    pub input_channels: u16,
    /// Channels of the output stream.
    pub output_channels: u16,
    /// Sample format of the input stream.
    pub input_format: SampleFormat,
    /// Sample format of the output stream.
    pub output_format: SampleFormat,
    /// Sample rate of both streams.
    pub sample_rate: u32,
    /// Buffer size the backend reports for the input stream, in frames.
    pub input_buffer: Option<u32>,
    /// Buffer size the backend reports for the output stream, in frames.
    pub output_buffer: Option<u32>,
    /// The engine's own latency, in samples.
    pub engine_latency: usize,
}

/// A running duplex stream: input → engine → output.
pub struct Duplex {
    // Dropping the streams stops them. Output first, so it never waits for input.
    _output: Stream,
    _input: Stream,
    commands: rtrb::Producer<Command>,
    results: rtrb::Consumer<ProbeResult>,
    /// Figures published by the audio threads.
    pub stats: Arc<Stats>,
    /// What was opened.
    pub info: Info,
}

/// Finds a host by name (case-insensitive), or the default host.
pub fn host(name: Option<&str>) -> Result<Host, Box<dyn Error>> {
    let Some(name) = name else {
        return Ok(cpal::default_host());
    };
    let id = cpal::available_hosts()
        .into_iter()
        .find(|id| id.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            let known: Vec<_> = cpal::available_hosts().iter().map(HostId::name).collect();
            let hint = if name.eq_ignore_ascii_case("asio") {
                " (ASIO needs a build with `--features asio`)"
            } else {
                ""
            };
            format!(
                "unknown host {name:?}{hint}; available: {}",
                known.join(", ")
            )
        })?;
    Ok(cpal::host_from_id(id)?)
}

/// The device's name, or a placeholder.
pub fn name(device: &Device) -> String {
    device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "(unnamed)".to_owned())
}

fn find(
    devices: impl Iterator<Item = Device>,
    part: &str,
    kind: &str,
) -> Result<Device, Box<dyn Error>> {
    let part_lower = part.to_lowercase();
    let mut names = Vec::new();
    for device in devices {
        let n = name(&device);
        if n.to_lowercase().contains(&part_lower) {
            return Ok(device);
        }
        names.push(n);
    }
    Err(format!(
        "no {kind} device matches {part:?}; available: {}",
        names.join(", ")
    )
    .into())
}

impl Duplex {
    /// Opens the devices, starts both streams and runs `engine` between them.
    pub fn open(
        options: &Options,
        engine_setup: impl FnOnce(&mut Engine),
    ) -> Result<Self, Box<dyn Error>> {
        let host = host(options.host.as_deref())?;
        let is_asio = host.id().name().eq_ignore_ascii_case("asio");
        let input_device = match &options.input {
            Some(part) => find(host.input_devices()?, part, "input")?,
            None => host
                .default_input_device()
                .ok_or("no default input device")?,
        };
        let output_device = match &options.output {
            Some(part) => find(host.output_devices()?, part, "output")?,
            None => host
                .default_output_device()
                .ok_or("no default output device")?,
        };

        let out_default = output_device.default_output_config()?;
        let in_default = input_device.default_input_config()?;
        let sample_rate = options.rate.unwrap_or(out_default.sample_rate());
        let buffer_size = options
            .buffer
            .map_or(BufferSize::Default, BufferSize::Fixed);
        let in_config = StreamConfig {
            channels: in_default.channels(),
            sample_rate,
            buffer_size,
        };
        let out_config = StreamConfig {
            channels: out_default.channels(),
            sample_rate,
            buffer_size,
        };
        if options.channel >= usize::from(in_config.channels) {
            return Err(format!(
                "input {} has {} channel(s), asked for channel {}",
                name(&input_device),
                in_config.channels,
                options.channel + 1
            )
            .into());
        }

        let mut engine = Engine::new(sample_rate as f32);
        engine_setup(&mut engine);
        engine.reset();
        let engine_latency = engine.latency_samples();

        let stats = Arc::new(Stats::default());
        // One second of input: far more than any sane buffer, so it only fills if the output
        // stops pulling.
        let (producer, consumer) = rtrb::RingBuffer::new(sample_rate as usize);
        let (commands, command_rx) = rtrb::RingBuffer::new(4);
        let (result_tx, results) = rtrb::RingBuffer::new(4);
        let margin = options.margin.unwrap_or(if is_asio {
            0
        } else {
            sample_rate as usize / 1000
        });
        let processor = Processor::new(
            engine,
            consumer,
            command_rx,
            result_tx,
            stats.clone(),
            margin,
        );

        // Input first: with ASIO, callbacks run in the order the streams were built, so the
        // output finds this cycle's input already waiting.
        let input = by_format!(
            in_default.sample_format(),
            "input",
            input_stream(
                &input_device,
                in_config,
                options.channel,
                producer,
                stats.clone()
            )
        )
        .map_err(|e| opening("input", &input_device, &e))?;
        let output = by_format!(
            out_default.sample_format(),
            "output",
            output_stream(&output_device, out_config, processor, stats.clone())
        )
        .map_err(|e| opening("output", &output_device, &e))?;
        input
            .play()
            .map_err(|e| opening("input", &input_device, &e))?;
        output
            .play()
            .map_err(|e| opening("output", &output_device, &e))?;

        let info = Info {
            host: host.id().name(),
            input: name(&input_device),
            output: name(&output_device),
            input_channels: in_config.channels,
            output_channels: out_config.channels,
            input_format: in_default.sample_format(),
            output_format: out_default.sample_format(),
            sample_rate,
            input_buffer: input.buffer_size().ok(),
            output_buffer: output.buffer_size().ok(),
            engine_latency,
        };
        Ok(Self {
            _output: output,
            _input: input,
            commands,
            results,
            stats,
            info,
        })
    }

    /// Measures the latency over a loopback cable. Blocks for one to three seconds.
    pub fn measure(&mut self) -> Result<Measurement, MeasureError> {
        while self.results.pop().is_ok() {}
        self.commands
            .push(Command::Measure)
            .map_err(|_| MeasureError::Stalled)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(result) = self.results.pop() {
                return probe::summarise(
                    result,
                    f64::from(self.info.sample_rate),
                    self.info.engine_latency,
                )
                .map_err(MeasureError::Failed);
            }
            if Instant::now() > deadline {
                return Err(MeasureError::Stalled);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Why [`Duplex::measure`] gave no figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureError {
    /// The probe ran but could not measure.
    Failed(Failure),
    /// The audio stopped running (no result in time).
    Stalled,
}

/// Explains a failure to open or start a stream.
fn opening(kind: &str, device: &Device, e: &dyn std::fmt::Display) -> Box<dyn Error> {
    let text = e.to_string();
    // AUDCLNT_E_DEVICE_IN_USE: another program holds the device in exclusive mode.
    let hint = if text.contains("-2004287478") {
        "
(the device is in use in exclusive mode by another program, e.g. Voicemeeter, a DAW          or a browser tab; close it, or untick \"Allow applications to take exclusive control\"          in the device's Windows sound settings)"
    } else {
        ""
    };
    format!(
        "could not open the {kind} stream on {}: {text}{hint}",
        name(device)
    )
    .into()
}

fn on_error(stats: Arc<Stats>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |_| {
        stats.backend_errors.fetch_add(1, Ordering::Relaxed);
    }
}

fn input_stream<T>(
    device: &Device,
    config: StreamConfig,
    channel: usize,
    mut ring: rtrb::Producer<f32>,
    stats: Arc<Stats>,
) -> Result<Stream, Box<dyn Error>>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let errors = on_error(stats.clone());
    let stream = device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], info| {
            let ts = info.timestamp();
            if let Some(d) = ts.callback.checked_duration_since(ts.capture) {
                stats
                    .input_delay_us
                    .store(d.as_micros() as u32, Ordering::Relaxed);
            }
            let frames = data.len() / channels;
            stats.input_block.store(frames as u32, Ordering::Relaxed);
            let fit = frames.min(ring.slots());
            if fit < frames {
                stats.overflows.fetch_add(1, Ordering::Relaxed);
            }
            if let Ok(chunk) = ring.write_chunk_uninit(fit) {
                chunk.fill_from_iter(
                    data.chunks_exact(channels)
                        .map(|frame| frame[channel].to_sample::<f32>()),
                );
            }
        },
        errors,
        None,
    )?;
    Ok(stream)
}

fn output_stream<T>(
    device: &Device,
    config: StreamConfig,
    mut processor: Processor,
    stats: Arc<Stats>,
) -> Result<Stream, Box<dyn Error>>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = usize::from(config.channels);
    let sample_rate = config.sample_rate as f32;
    let mut mono = vec![0.0_f32; MAX_FRAMES];
    let errors = on_error(stats.clone());
    let stream = device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], info| {
            let start = Instant::now();
            let ts = info.timestamp();
            if let Some(d) = ts.playback.checked_duration_since(ts.callback) {
                stats
                    .output_delay_us
                    .store(d.as_micros() as u32, Ordering::Relaxed);
            }
            let frames = data.len() / channels;
            stats.output_block.store(frames as u32, Ordering::Relaxed);
            for piece in data.chunks_mut(channels * MAX_FRAMES) {
                let n = piece.len() / channels;
                processor.process(&mut mono[..n]);
                for (frame, &y) in piece.chunks_exact_mut(channels).zip(&mono[..n]) {
                    frame.fill(T::from_sample(y));
                }
            }
            if frames > 0 {
                let budget = frames as f32 / sample_rate;
                let load = start.elapsed().as_secs_f32() / budget;
                stats
                    .load_permille
                    .fetch_max((load * 1000.0) as u32, Ordering::Relaxed);
            }
        },
        errors,
        None,
    )?;
    Ok(stream)
}
