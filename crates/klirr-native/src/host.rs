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
use klirr_dsp::{CustomIr, Engine, Param};

use crate::duplex::{Command, Event, Processor, Stats};
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
/// Commands that can wait for the audio thread at once. Parameter changes come in bursts
/// while a knob turns, about one per parameter every few milliseconds.
const COMMAND_SLOTS: usize = 256;
/// A measurement that has not finished after this long counts as stalled.
const MEASURE_TIMEOUT: Duration = Duration::from_secs(15);

/// Which devices to open and how.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Host (audio API) name, e.g. `wasapi`, `asio` or `wasapi-exclusive`. Default: the
    /// platform's default host.
    pub host: Option<String>,
    /// Part of the input device's name. Default: the host's default input.
    pub input: Option<String>,
    /// Part of the output device's name. Default: the host's default output.
    pub output: Option<String>,
    /// Input channel, counted from 0.
    pub channel: usize,
    /// Sample rate. Default: the output device's.
    pub rate: Option<u32>,
    /// Buffer size in frames (the period, for WASAPI exclusive). Default: the backend's.
    pub buffer: Option<u32>,
    /// Safety margin between the streams, in frames. Default: none for ASIO and WASAPI
    /// exclusive, 1 ms otherwise.
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
    /// Buffer size of the input stream: the frames it delivers per cycle (the ASIO buffer
    /// size, the WASAPI period), as the backend reports it.
    pub input_buffer: Option<u32>,
    /// Buffer size of the output stream, like `input_buffer`.
    pub output_buffer: Option<u32>,
    /// The engine's own latency, in samples.
    pub engine_latency: usize,
}

/// What keeps the audio running; dropping it stops the streams.
#[allow(
    clippy::large_enum_variant,
    reason = "one per open device, never moved around"
)]
enum Streams {
    /// Two cpal streams. Output first, so it never waits for input.
    Cpal { _output: Stream, _input: Stream },
    /// One thread running both endpoints in WASAPI exclusive mode.
    #[cfg(windows)]
    Exclusive { _stream: crate::exclusive::Stream },
}

/// The engine between the streams, with the ends of its rings the streams and the main
/// thread use.
struct Core {
    processor: Processor,
    input: rtrb::Producer<f32>,
    commands: rtrb::Producer<Command>,
    events: rtrb::Consumer<Event>,
    engine_latency: usize,
}

impl Core {
    fn new(
        sample_rate: u32,
        margin: usize,
        engine_setup: impl FnOnce(&mut Engine),
        stats: &Arc<Stats>,
    ) -> Self {
        let mut engine = Engine::new(sample_rate as f32);
        engine_setup(&mut engine);
        engine.reset();
        let engine_latency = engine.latency_samples();
        // One second of input: far more than any sane buffer, so it only fills if the output
        // stops pulling.
        let (input, consumer) = rtrb::RingBuffer::new(sample_rate as usize);
        let (commands, command_rx) = rtrb::RingBuffer::new(COMMAND_SLOTS);
        // Room for every IR that can be queued, so the audio thread never frees one.
        let (event_tx, events) = rtrb::RingBuffer::new(COMMAND_SLOTS);
        let processor = Processor::new(
            engine,
            consumer,
            command_rx,
            event_tx,
            stats.clone(),
            margin,
        );
        Self {
            processor,
            input,
            commands,
            events,
            engine_latency,
        }
    }
}

/// A running duplex stream: input → engine → output.
pub struct Duplex {
    _streams: Streams,
    commands: rtrb::Producer<Command>,
    events: rtrb::Consumer<Event>,
    /// When the running measurement started, if one runs.
    measuring: Option<Instant>,
    /// The result of the last measurement, until it is collected.
    probe_result: Option<ProbeResult>,
    /// Figures published by the audio threads.
    pub stats: Arc<Stats>,
    /// What was opened.
    pub info: Info,
}

/// Hosts this crate adds to cpal's, by their `--host` names.
#[cfg(windows)]
pub const EXTRA_HOSTS: &[&str] = &[crate::exclusive::HOST_ARG];
/// Hosts this crate adds to cpal's, by their `--host` names.
#[cfg(not(windows))]
pub const EXTRA_HOSTS: &[&str] = &[];

/// Finds a cpal host by name (case-insensitive), or the default host.
///
/// # Errors
///
/// If no available host has that name.
pub fn host(name: Option<&str>) -> Result<Host, Box<dyn Error>> {
    let Some(name) = name else {
        return Ok(cpal::default_host());
    };
    let id = cpal::available_hosts()
        .into_iter()
        .find(|id| id.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            let known: Vec<_> = cpal::available_hosts()
                .iter()
                .map(HostId::name)
                .chain(EXTRA_HOSTS.iter().copied())
                .collect();
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

/// The device whose name is `part`, or else the first whose name contains it.
pub(crate) fn find(
    devices: impl Iterator<Item = Device>,
    part: &str,
    kind: &str,
) -> Result<Device, Box<dyn Error>> {
    let part_lower = part.to_lowercase();
    let mut names = Vec::new();
    let mut partial = None;
    for device in devices {
        let n = name(&device);
        // An exact name wins over a device whose name only contains it ("Speakers" over
        // "Speakers (2)").
        if n == part {
            return Ok(device);
        }
        if partial.is_none() && n.to_lowercase().contains(&part_lower) {
            partial = Some(device);
        }
        names.push(n);
    }
    if let Some(device) = partial {
        return Ok(device);
    }
    Err(format!(
        "no {kind} device matches {part:?}; available: {}",
        names.join(", ")
    )
    .into())
}

/// The input and output device of `host` whose names contain `input` and `output`, or its
/// defaults.
pub(crate) fn pick_devices(
    host: &Host,
    input: Option<&str>,
    output: Option<&str>,
) -> Result<(Device, Device), Box<dyn Error>> {
    let input = match input {
        Some(part) => find(host.input_devices()?, part, "input")?,
        None => host
            .default_input_device()
            .ok_or("no default input device")?,
    };
    let output = match output {
        Some(part) => find(host.output_devices()?, part, "output")?,
        None => host
            .default_output_device()
            .ok_or("no default output device")?,
    };
    Ok((input, output))
}

impl Duplex {
    /// Opens the devices, starts both streams and runs `engine` between them.
    ///
    /// # Errors
    ///
    /// If a device cannot be found or opened, or a stream does not start.
    pub fn open(
        options: &Options,
        engine_setup: impl FnOnce(&mut Engine),
    ) -> Result<Self, Box<dyn Error>> {
        #[cfg(windows)]
        if options
            .host
            .as_deref()
            .is_some_and(crate::exclusive::is_host)
        {
            return Self::open_exclusive(options, engine_setup);
        }
        let host = host(options.host.as_deref())?;
        let is_asio = host.id().name().eq_ignore_ascii_case("asio");
        let (input_device, output_device) =
            pick_devices(&host, options.input.as_deref(), options.output.as_deref())?;

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

        let stats = Arc::new(Stats::default());
        let margin = options.margin.unwrap_or(if is_asio {
            0
        } else {
            sample_rate as usize / 1000
        });
        let core = Core::new(sample_rate, margin, engine_setup, &stats);

        // Input first: with ASIO, callbacks run in the order the streams were built, so the
        // output finds this cycle's input already waiting.
        let input = by_format!(
            in_default.sample_format(),
            "input",
            input_stream(
                &input_device,
                in_config,
                options.channel,
                core.input,
                stats.clone()
            )
        )
        .map_err(|e| opening("input", &input_device, &e))?;
        let output = by_format!(
            out_default.sample_format(),
            "output",
            output_stream(&output_device, out_config, core.processor, stats.clone())
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
            engine_latency: core.engine_latency,
        };
        Ok(Self {
            _streams: Streams::Cpal {
                _output: output,
                _input: input,
            },
            commands: core.commands,
            events: core.events,
            measuring: None,
            probe_result: None,
            stats,
            info,
        })
    }

    /// [`Duplex::open`] for WASAPI exclusive mode.
    #[cfg(windows)]
    fn open_exclusive(
        options: &Options,
        engine_setup: impl FnOnce(&mut Engine),
    ) -> Result<Self, Box<dyn Error>> {
        let stats = Arc::new(Stats::default());
        let pending = crate::exclusive::open(options, stats.clone())?;
        let s = pending.setup.clone();
        // Both endpoints run in one cycle, input first, as with ASIO.
        let margin = options.margin.unwrap_or(0);
        let core = Core::new(s.sample_rate, margin, engine_setup, &stats);
        let stream = pending.start(core.processor, core.input)?;
        let info = Info {
            host: crate::exclusive::HOST_NAME,
            input: s.input,
            output: s.output,
            input_channels: s.input_channels,
            output_channels: s.output_channels,
            input_format: s.input_format,
            output_format: s.output_format,
            sample_rate: s.sample_rate,
            // Both endpoints run at the one period. The render buffer the driver allocated
            // (`s.output_buffer`) is only room: the loop keeps far less of it queued.
            input_buffer: Some(s.input_buffer),
            output_buffer: Some(s.input_buffer),
            engine_latency: core.engine_latency,
        };
        Ok(Self {
            _streams: Streams::Exclusive { _stream: stream },
            commands: core.commands,
            events: core.events,
            measuring: None,
            probe_result: None,
            stats,
            info,
        })
    }

    /// Sets a parameter on the running engine. Returns false if the audio thread has not
    /// taken the earlier commands yet (it has stopped, or is far behind); try again later.
    pub fn set_param(&mut self, param: Param, value: f32) -> bool {
        self.collect();
        self.commands.push(Command::Set(param, value)).is_ok()
    }

    /// Loads a custom cabinet IR (mono samples at `ir_sample_rate`) into the running engine.
    /// The IR is prepared here; the audio thread only swaps it in.
    ///
    /// # Errors
    ///
    /// If the IR is invalid, or the audio thread is not taking commands.
    pub fn load_ir(&mut self, samples: &[f32], ir_sample_rate: f32) -> Result<(), Box<dyn Error>> {
        self.collect();
        let ir = CustomIr::new(samples, ir_sample_rate, self.info.sample_rate as f32)?;
        self.commands
            .push(Command::Ir(Box::new(ir)))
            .map_err(|_| "the audio is not running")?;
        Ok(())
    }

    /// Starts a loopback measurement; [`Duplex::poll_measure`] collects the result. A
    /// measurement that is already running starts over.
    ///
    /// # Errors
    ///
    /// [`MeasureError::Stalled`] if the audio thread is not taking commands.
    pub fn start_measure(&mut self) -> Result<(), MeasureError> {
        self.collect();
        self.probe_result = None;
        self.commands
            .push(Command::Measure)
            .map_err(|_| MeasureError::Stalled)?;
        self.measuring = Some(Instant::now());
        Ok(())
    }

    /// The result of the measurement [`Duplex::start_measure`] started, once it is over.
    /// None while it runs, or if none was started.
    pub fn poll_measure(&mut self) -> Option<Result<Measurement, MeasureError>> {
        let started = self.measuring?;
        self.collect();
        if let Some(result) = self.probe_result.take() {
            self.measuring = None;
            return Some(
                probe::summarise(
                    result,
                    f64::from(self.info.sample_rate),
                    self.info.engine_latency,
                )
                .map_err(MeasureError::Failed),
            );
        }
        if started.elapsed() > MEASURE_TIMEOUT {
            self.measuring = None;
            return Some(Err(MeasureError::Stalled));
        }
        None
    }

    /// Fades the test tone (see [`crate::tone`]) in or out. Returns false if the audio thread
    /// has not taken the earlier commands yet; try again later.
    pub fn set_test_tone(&mut self, on: bool) -> bool {
        self.collect();
        self.commands.push(Command::TestTone(on)).is_ok()
    }

    /// Measures the latency over a loopback cable. Blocks for one to three seconds.
    ///
    /// # Errors
    ///
    /// If the probe heard nothing usable, or the audio stopped.
    pub fn measure(&mut self) -> Result<Measurement, MeasureError> {
        self.start_measure()?;
        loop {
            if let Some(result) = self.poll_measure() {
                return result;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Takes what the audio thread sent back: probe results, and IRs to free.
    fn collect(&mut self) {
        while let Ok(event) = self.events.pop() {
            match event {
                Event::Probe(result) => self.probe_result = Some(result),
                Event::Freed(ir) => drop(ir),
            }
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
    move |e| {
        // ASIO: the driver asked for a reset or resync, or its sample rate changed.
        let counter = if e.kind() == cpal::ErrorKind::StreamInvalidated {
            &stats.reset_requests
        } else {
            &stats.backend_errors
        };
        counter.fetch_add(1, Ordering::Relaxed);
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
