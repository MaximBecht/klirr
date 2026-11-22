//! WASAPI exclusive mode (Windows only), through the safe bindings of the `wasapi` crate.
//!
//! Shared mode (cpal's WASAPI) goes through the Windows audio engine: a fixed 10 ms period on
//! most devices, a client buffer of about 22 ms that cpal keeps full, and whatever the driver
//! adds behind the mixer. Exclusive mode talks to the driver directly with the device's
//! smallest period (often 3 ms), at the price of taking the device away from every other
//! program while it runs.
//!
//! One audio thread runs both endpoints. The input is event driven and sets the pace: each
//! time it has finished a period, the thread moves it into the ring, then runs the
//! [`Processor`] for as much output as the render buffer needs, the way an ASIO driver's cycle
//! does. The output is polled rather than event driven, and kept only as full as the driver
//! needs (see `FillTarget`). The ring and the processor's margin logic are the same as for
//! cpal's streams, so the probe measures this path exactly like the others.
//!
//! COM objects stay on the audio thread: it opens the devices, reports what it found, receives
//! the processor (built on the calling thread, so the engine can be set up there) and runs.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Instant;

use cpal::SampleFormat;
use wasapi::{
    AudioCaptureClient, AudioClient, AudioRenderClient, Device, DeviceEnumerator, Direction,
    Handle, SampleType, ShareMode, StreamMode, WasapiError, WaveFormat,
};

use crate::devices::{DeviceInfo, DeviceList};
use crate::duplex::{Processor, Stats};
use crate::host::Options;

/// The name `--host` takes.
pub const HOST_ARG: &str = "wasapi-exclusive";
/// The name shown for this host.
pub const HOST_NAME: &str = "WASAPI exclusive";

/// Whether `name` selects this host.
pub fn is_host(name: &str) -> bool {
    name.eq_ignore_ascii_case(HOST_ARG)
}

// HRESULTs this module explains (AUDCLNT_E_*).
const DEVICE_IN_USE: u32 = 0x8889_000A;
const EXCLUSIVE_MODE_NOT_ALLOWED: u32 = 0x8889_000E;
const BUFFER_SIZE_NOT_ALIGNED: u32 = 0x8889_0019;
const UNSUPPORTED_FORMAT: u32 = 0x8889_0008;

/// How long the audio thread waits for the device before it counts a backend error.
const EVENT_TIMEOUT_MS: u32 = 100;

/// A sample encoding the thread can read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    /// 32-bit float.
    F32,
    /// 32-bit integer container, left-justified (also for 24 valid bits).
    I32,
    /// Packed 24-bit integer.
    I24,
    /// 16-bit integer.
    I16,
}

/// Formats to ask the driver for, best first: encoding, container bits, valid bits, type.
const CANDIDATES: [(Encoding, usize, usize, SampleType); 5] = [
    (Encoding::F32, 32, 32, SampleType::Float),
    (Encoding::I32, 32, 32, SampleType::Int),
    (Encoding::I32, 32, 24, SampleType::Int),
    (Encoding::I24, 24, 24, SampleType::Int),
    (Encoding::I16, 16, 16, SampleType::Int),
];

impl Encoding {
    /// Bytes per sample.
    fn bytes(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::I24 => 3,
            Self::I16 => 2,
        }
    }

    /// The matching cpal format, for display.
    fn sample_format(self) -> SampleFormat {
        match self {
            Self::F32 => SampleFormat::F32,
            Self::I32 => SampleFormat::I32,
            Self::I24 => SampleFormat::I24,
            Self::I16 => SampleFormat::I16,
        }
    }

    /// Reads one sample (`b` holds exactly [`Encoding::bytes`] bytes, little-endian).
    fn decode(self, b: &[u8]) -> f32 {
        match self {
            Self::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Self::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            // Into the top three bytes of an i32: the sign comes along for free.
            Self::I24 => i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32 / 2_147_483_648.0,
            Self::I16 => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0,
        }
    }

    /// Writes one sample into `b` ([`Encoding::bytes`] bytes, little-endian), clipped to ±1.
    fn encode(self, x: f32, b: &mut [u8]) {
        let x = x.clamp(-1.0, 1.0);
        match self {
            Self::F32 => b.copy_from_slice(&x.to_le_bytes()),
            // `as` saturates, so +1.0 becomes i32::MAX.
            Self::I32 => b.copy_from_slice(&((x * 2_147_483_648.0) as i32).to_le_bytes()),
            Self::I24 => {
                let v = (x * 8_388_608.0).round().min(8_388_607.0) as i32;
                b.copy_from_slice(&v.to_le_bytes()[..3]);
            }
            Self::I16 => {
                b.copy_from_slice(&((x * 32_768.0).round().min(32_767.0) as i16).to_le_bytes())
            }
        }
    }
}

/// What the audio thread opened.
#[derive(Clone, Debug)]
pub struct Setup {
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
    /// Buffer (one period) of the input stream, in frames.
    pub input_buffer: u32,
    /// Buffer (one period) of the output stream, in frames.
    pub output_buffer: u32,
}

/// Audio thread → caller.
enum Message {
    Ready(Setup),
    Running,
    Failed(String),
}

/// What the caller hands the audio thread once it knows the sample rate.
type Core = (Processor, rtrb::Producer<f32>);

/// The audio thread. Dropping it stops the streams and waits for the thread to end.
pub struct Stream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Devices that are open but not started yet.
pub struct Pending {
    /// What was opened.
    pub setup: Setup,
    core: mpsc::SyncSender<Core>,
    messages: mpsc::Receiver<Message>,
    stream: Stream,
}

/// Opens the devices on a new audio thread. Nothing plays until [`Pending::start`].
pub fn open(options: &Options, stats: Arc<Stats>) -> Result<Pending, Box<dyn Error>> {
    let (message_tx, messages) = mpsc::channel();
    let (core, core_rx) = mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let options = options.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("klirr-wasapi-exclusive".to_owned())
            .spawn(move || thread_main(&options, &stats, &stop, &message_tx, &core_rx))?
    };
    let stream = Stream {
        stop,
        thread: Some(thread),
    };
    match messages.recv() {
        Ok(Message::Ready(setup)) => Ok(Pending {
            setup,
            core,
            messages,
            stream,
        }),
        Ok(Message::Failed(e)) => Err(e.into()),
        Ok(Message::Running) | Err(_) => Err("the WASAPI exclusive thread ended early".into()),
    }
}

impl Pending {
    /// Starts both streams with `processor` between them, `input` feeding its ring.
    pub fn start(
        self,
        processor: Processor,
        input: rtrb::Producer<f32>,
    ) -> Result<Stream, Box<dyn Error>> {
        self.core
            .send((processor, input))
            .map_err(|_| "the WASAPI exclusive thread ended early")?;
        match self.messages.recv() {
            Ok(Message::Running) => Ok(self.stream),
            Ok(Message::Failed(e)) => Err(e.into()),
            Ok(Message::Ready(_)) | Err(_) => Err("the WASAPI exclusive thread ended early".into()),
        }
    }
}

fn thread_main(
    options: &Options,
    stats: &Stats,
    stop: &AtomicBool,
    messages: &mpsc::Sender<Message>,
    core: &mpsc::Receiver<Core>,
) {
    if wasapi::initialize_mta().is_err() {
        let _ = messages.send(Message::Failed("could not initialise COM".to_owned()));
        return;
    }
    let result = (|| {
        let io = Io::open(options)?;
        let _ = messages.send(Message::Ready(io.setup.clone()));
        let Ok((processor, input)) = core.recv() else {
            // The caller gave up (e.g. a bad channel number): close without starting.
            return Ok(());
        };
        io.start()?;
        let _ = messages.send(Message::Running);
        io.run(processor, input, stats, stop)
    })();
    if let Err(e) = result {
        stats.backend_errors.fetch_add(1, Ordering::Relaxed);
        // Nobody listens once the streams run; then the error shows as a stalled stream.
        let _ = messages.send(Message::Failed(e));
    }
    // Every COM object of this thread lived in the closure and is released by now.
    wasapi::deinitialize();
}

/// The HRESULT inside a WASAPI error, if it carries one.
fn hresult(e: &WasapiError) -> Option<u32> {
    match e {
        WasapiError::Windows(w) => Some(w.code().0 as u32),
        _ => None,
    }
}

/// An error message with a hint for the errors users can fix.
fn explain(what: &str, device: &str, e: &WasapiError) -> String {
    let hint = match hresult(e) {
        Some(DEVICE_IN_USE) => {
            "\n(exclusive mode needs the device to itself: close every other program that \
             uses it, e.g. Voicemeeter, a DAW, a browser tab or a media player)"
        }
        Some(EXCLUSIVE_MODE_NOT_ALLOWED) => {
            "\n(tick \"Allow applications to take exclusive control of this device\" in the \
             device's Windows sound settings, under Advanced)"
        }
        _ => "",
    };
    format!("could not {what} {device} in exclusive mode: {e}{hint}")
}

fn find_device(
    enumerator: &DeviceEnumerator,
    direction: Direction,
    part: Option<&str>,
) -> Result<Device, String> {
    let kind = match direction {
        Direction::Render => "output",
        Direction::Capture => "input",
    };
    let Some(part) = part else {
        return enumerator
            .get_default_device(direction)
            .map_err(|e| format!("no default {kind} device: {e}"));
    };
    let devices = enumerator
        .get_device_collection(direction)
        .map_err(|e| format!("could not list {kind} devices: {e}"))?;
    let part_lower = part.to_lowercase();
    let mut names = Vec::new();
    let mut partial = None;
    let count = devices.get_nbr_devices().map_err(|e| e.to_string())?;
    for i in 0..count {
        let device = devices.get_device_at_index(i).map_err(|e| e.to_string())?;
        let name = device.get_friendlyname().unwrap_or_default();
        // An exact name wins over a device whose name only contains it.
        if name == part {
            return Ok(device);
        }
        if partial.is_none() && name.to_lowercase().contains(&part_lower) {
            partial = Some(device);
        }
        names.push(name);
    }
    if let Some(device) = partial {
        return Ok(device);
    }
    Err(format!(
        "no {kind} device matches {part:?}; available: {}",
        names.join(", ")
    ))
}

/// The best format the device accepts in exclusive mode at `rate`.
fn negotiate(
    client: &AudioClient,
    rate: u32,
    channels: u16,
) -> Result<(WaveFormat, Encoding), WasapiError> {
    let format = |&(_, bits, valid, kind): &(Encoding, usize, usize, SampleType)| {
        WaveFormat::new(
            bits,
            valid,
            kind,
            rate as usize,
            usize::from(channels),
            None,
        )
    };
    for candidate in &CANDIDATES {
        if let Ok(f) = client.is_supported_exclusive_with_quirks(&format(candidate)) {
            return Ok((f, candidate.0));
        }
    }
    // Ask once more, plainly, for the reason (the device may be in use, for instance).
    match client.is_supported(&format(&CANDIDATES[0]), ShareMode::Exclusive) {
        Err(e) if hresult(&e) != Some(UNSUPPORTED_FORMAT) => Err(e),
        _ => Err(WasapiError::UnsupportedFormat),
    }
}

/// An exclusive client with the stream mode `mode` makes of the period, aligned if the
/// driver insists.
fn initialize(
    device: &Device,
    direction: Direction,
    format: &WaveFormat,
    period_hns: i64,
    mode: impl Fn(i64) -> StreamMode,
) -> Result<AudioClient, WasapiError> {
    let mut client = device.get_iaudioclient()?;
    match client.initialize_client(format, direction, mode(period_hns), None) {
        Err(e) if hresult(&e) == Some(BUFFER_SIZE_NOT_ALIGNED) => {
            // As Microsoft's example does it: the failed client tells the aligned size.
            let frames = client.get_buffer_size()?;
            let period_hns = wasapi::calculate_period_100ns(
                i64::from(frames),
                i64::from(format.get_samplespersec()),
            );
            let mut client = device.get_iaudioclient()?;
            client.initialize_client(format, direction, mode(period_hns), None)?;
            Ok(client)
        }
        other => other.map(|()| client),
    }
}

/// Both endpoints, initialised and ready to start.
struct Io {
    setup: Setup,
    capture: AudioClient,
    render: AudioClient,
    capture_client: AudioCaptureClient,
    render_client: AudioRenderClient,
    /// Signalled when the input has finished a period: the clock of the loop.
    capture_event: Handle,
    input_encoding: Encoding,
    output_encoding: Encoding,
    channel: usize,
}

impl Io {
    fn open(options: &Options) -> Result<Self, String> {
        let enumerator =
            DeviceEnumerator::new().map_err(|e| format!("could not list devices: {e}"))?;
        let input = find_device(&enumerator, Direction::Capture, options.input.as_deref())?;
        let output = find_device(&enumerator, Direction::Render, options.output.as_deref())?;
        let input_name = input.get_friendlyname().unwrap_or_default();
        let output_name = output.get_friendlyname().unwrap_or_default();

        let probe = |device: &Device, name: &str| {
            let client = device
                .get_iaudioclient()
                .map_err(|e| explain("open", name, &e))?;
            let mix = client
                .get_mixformat()
                .map_err(|e| explain("read the format of", name, &e))?;
            Ok::<_, String>((client, mix))
        };
        let (in_probe, in_mix) = probe(&input, &input_name)?;
        let (out_probe, out_mix) = probe(&output, &output_name)?;
        let sample_rate = options.rate.unwrap_or(out_mix.get_samplespersec());
        let input_channels = in_mix.get_nchannels();
        let output_channels = out_mix.get_nchannels();
        if options.channel >= usize::from(input_channels) {
            return Err(format!(
                "input {input_name} has {input_channels} channel(s), asked for channel {}",
                options.channel + 1
            ));
        }

        let no_format = |name: &str, e: &WasapiError| match e {
            WasapiError::UnsupportedFormat => format!(
                "{name} accepts none of f32, i32, i24 or i16 at {sample_rate} Hz in exclusive \
                 mode (try --rate with the rate set in its Windows sound settings)"
            ),
            e => explain("open", name, e),
        };
        let (in_format, input_encoding) = negotiate(&in_probe, sample_rate, input_channels)
            .map_err(|e| no_format(&input_name, &e))?;
        let (out_format, output_encoding) = negotiate(&out_probe, sample_rate, output_channels)
            .map_err(|e| no_format(&output_name, &e))?;

        // One period for both, at least each device's minimum.
        let min_period = |client: &AudioClient, name: &str| {
            client
                .get_device_period()
                .map(|(_, min)| min)
                .map_err(|e| explain("read the period of", name, &e))
        };
        let floor = min_period(&in_probe, &input_name)?.max(min_period(&out_probe, &output_name)?);
        let wanted = options.buffer.map_or(floor, |frames| {
            wasapi::calculate_period_100ns(i64::from(frames), i64::from(sample_rate))
        });
        let period = in_probe
            .calculate_aligned_period_near(wanted.max(floor), None, &in_format)
            .map_err(|e| explain("choose a period for", &input_name, &e))?;
        drop((in_probe, out_probe));

        // The input is event driven: its event is the loop's clock. The output is polled and
        // topped up after every input period (see `FillTarget`); the driver decides its buffer
        // size from the two periods asked for. (Event-driven exclusive output is unreliable
        // with some USB drivers: the Focusrite's signals only every second period.)
        let capture = initialize(&input, Direction::Capture, &in_format, period, |p| {
            StreamMode::EventsExclusive { period_hns: p }
        })
        .map_err(|e| explain("open", &input_name, &e))?;
        let render = initialize(&output, Direction::Render, &out_format, period, |p| {
            StreamMode::PollingExclusive {
                buffer_duration_hns: 2 * p,
                period_hns: p,
            }
        })
        .map_err(|e| explain("open", &output_name, &e))?;
        fn setup_error(name: &str) -> impl Fn(WasapiError) -> String + '_ {
            move |e| explain("set up", name, &e)
        }
        let capture_event = capture
            .set_get_eventhandle()
            .map_err(setup_error(&input_name))?;
        let capture_client = capture
            .get_audiocaptureclient()
            .map_err(setup_error(&input_name))?;
        let render_client = render
            .get_audiorenderclient()
            .map_err(setup_error(&output_name))?;
        let input_buffer = capture
            .get_buffer_size()
            .map_err(setup_error(&input_name))?;
        let output_buffer = render
            .get_buffer_size()
            .map_err(setup_error(&output_name))?;

        Ok(Self {
            setup: Setup {
                input: input_name,
                output: output_name,
                input_channels,
                output_channels,
                input_format: input_encoding.sample_format(),
                output_format: output_encoding.sample_format(),
                sample_rate,
                input_buffer,
                output_buffer,
            },
            capture,
            render,
            capture_client,
            render_client,
            capture_event,
            input_encoding,
            output_encoding,
            channel: options.channel,
        })
    }

    fn start(&self) -> Result<(), String> {
        let out = &self.setup.output;
        // Render starts from a full buffer of silence.
        let frames = self
            .render
            .get_available_space_in_frames()
            .map_err(|e| explain("start", out, &e))?;
        let bytes = frames as usize
            * usize::from(self.setup.output_channels)
            * self.output_encoding.bytes();
        self.render_client
            .write_to_device(&vec![0; bytes], None)
            .map_err(|e| explain("start", out, &e))?;
        self.capture
            .start_stream()
            .map_err(|e| explain("start", &self.setup.input, &e))?;
        self.render
            .start_stream()
            .map_err(|e| explain("start", out, &e))?;
        Ok(())
    }

    /// The audio loop. Allocates only before it starts.
    fn run(
        &self,
        mut processor: Processor,
        mut ring: rtrb::Producer<f32>,
        stats: &Stats,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let s = &self.setup;
        let in_sample = self.input_encoding.bytes();
        let in_frame = usize::from(s.input_channels) * in_sample;
        let in_offset = self.channel * in_sample;
        let out_sample = self.output_encoding.bytes();
        let out_frame = usize::from(s.output_channels) * out_sample;
        let out_buffer = s.output_buffer as usize;
        let period = s.input_buffer as usize;
        let budget = period as f32 / s.sample_rate as f32;
        // Capture packets are one period; room for two in case a wake-up came late.
        let mut captured = vec![0_u8; 2 * period * in_frame];
        let mut mono = vec![0.0_f32; out_buffer];
        let mut out = vec![0_u8; out_buffer * out_frame];

        // The drivers' own figures, for the "reported" line: stream latency per endpoint,
        // plus what waits in the output buffer.
        let us = |hns: i64| (hns / 10).clamp(0, i64::from(u32::MAX)) as u32;
        let frames_us =
            |frames: usize| (frames as u64 * 1_000_000 / u64::from(s.sample_rate)) as u32;
        let render_latency = us(self.render.get_stream_latency().unwrap_or(0));
        stats.input_delay_us.store(
            us(self.capture.get_stream_latency().unwrap_or(0)),
            Ordering::Relaxed,
        );
        let mut fill = FillTarget::new(period, out_buffer);
        let publish_fill = |target: usize| {
            stats
                .output_delay_us
                .store(frames_us(target) + render_latency, Ordering::Relaxed);
        };
        publish_fill(fill.target);

        let result = 'audio: loop {
            if stop.load(Ordering::Relaxed) {
                break Ok(());
            }
            if self.capture_event.wait_for_event(EVENT_TIMEOUT_MS).is_err() {
                stats.backend_errors.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            // Everything the input has finished so far goes into the ring. (The packets'
            // discontinuity flag is not counted: the Focusrite sets it on about every other
            // packet with nothing missing. Real gaps show as underruns of the ring.)
            loop {
                let n = match self.capture_client.read_from_device(&mut captured) {
                    Ok((n, _)) => n as usize,
                    Err(e) => break 'audio Err(explain("read from", &s.input, &e)),
                };
                if n == 0 {
                    break;
                }
                stats.input_block.store(n as u32, Ordering::Relaxed);
                let fit = n.min(ring.slots());
                if fit < n {
                    stats.overflows.fetch_add(1, Ordering::Relaxed);
                }
                if let Ok(chunk) = ring.write_chunk_uninit(fit) {
                    chunk.fill_from_iter(captured[..n * in_frame].chunks_exact(in_frame).map(
                        |frame| {
                            self.input_encoding
                                .decode(&frame[in_offset..in_offset + in_sample])
                        },
                    ));
                }
            }

            // Then the output is topped up towards the fill target, with no more than the
            // input that is waiting: the output buffer, not the ring, absorbs the difference
            // between the driver's chunks and the input's periods.
            let start = Instant::now();
            let queued = match self.render.get_current_padding() {
                Ok(n) => (n as usize).min(out_buffer),
                Err(e) => break Err(explain("write to", &s.output, &e)),
            };
            let waiting = ring.buffer().capacity() - ring.slots();
            let before = fill.target;
            let step = fill.update(queued, waiting);
            if step.ran_dry {
                stats.backend_errors.fetch_add(1, Ordering::Relaxed);
            }
            if fill.target != before {
                publish_fill(fill.target);
            }
            if step.write == 0 {
                continue;
            }
            processor.process(&mut mono[..step.write]);
            let bytes = &mut out[..step.write * out_frame];
            for (frame, &y) in bytes.chunks_exact_mut(out_frame).zip(&mono) {
                for sample in frame.chunks_exact_mut(out_sample) {
                    self.output_encoding.encode(y, sample);
                }
            }
            if let Err(e) = self.render_client.write_to_device(bytes, None) {
                break Err(explain("write to", &s.output, &e));
            }
            stats
                .output_block
                .store(step.write as u32, Ordering::Relaxed);
            let load = start.elapsed().as_secs_f32() / budget;
            stats
                .load_permille
                .fetch_max((load * 1000.0) as u32, Ordering::Relaxed);
        };
        let _ = self.render.stop_stream();
        let _ = self.capture.stop_stream();
        result
    }
}

/// Input periods the loop watches the output before it sets the fill target.
const LEARN_PERIODS: usize = 150;

/// How full the output buffer is kept, learned from the driver.
///
/// The driver takes output in chunks of its own size, which need not be the period (the
/// Focusrite takes 6 ms at a time at a 3 ms period). Whatever is queued beyond what it takes
/// between two input periods only adds latency. So the loop starts from a full buffer, watches
/// how much the driver takes between two wake-ups, then keeps that much queued plus one period
/// of headroom. If the output ever runs dry, the target grows by a period. A write never takes
/// more input than is waiting, so the ring does not underrun when the driver's chunk is larger
/// than the period.
#[derive(Debug)]
struct FillTarget {
    /// Frames to keep queued.
    target: usize,
    period: usize,
    buffer: usize,
    learning: usize,
    /// Most the driver took between two wake-ups.
    most_taken: usize,
    /// Frames queued right after the last write.
    queued_after: Option<usize>,
}

/// What [`FillTarget::update`] decided.
#[derive(Debug, PartialEq, Eq)]
struct FillStep {
    /// Frames to write now.
    write: usize,
    /// The device had played everything: there was a gap in the output.
    ran_dry: bool,
}

impl FillTarget {
    fn new(period: usize, buffer: usize) -> Self {
        Self {
            target: buffer,
            period,
            buffer,
            learning: LEARN_PERIODS,
            most_taken: 0,
            queued_after: None,
        }
    }

    /// Called once per input period with the frames still queued for output and the input
    /// frames waiting to be processed.
    fn update(&mut self, queued: usize, waiting: usize) -> FillStep {
        if let Some(after) = self.queued_after {
            self.most_taken = self.most_taken.max(after.saturating_sub(queued));
        }
        let mut ran_dry = false;
        if self.learning > 0 {
            self.learning -= 1;
            if self.learning == 0 {
                self.target = (self.most_taken + self.period).min(self.buffer);
            }
        } else if queued == 0 {
            ran_dry = true;
            self.target = (self.target + self.period).min(self.buffer);
        }
        let write = self.target.saturating_sub(queued).min(waiting);
        self.queued_after = Some(queued + write);
        FillStep { write, ran_dry }
    }
}

/// Runs `f` on a new thread with COM set up the way this module needs it.
fn on_com_thread<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, WasapiError> + Send + 'static,
) -> Result<T, Box<dyn Error>> {
    let thread = std::thread::spawn(|| {
        if wasapi::initialize_mta().is_err() {
            return Err("could not initialise COM".to_owned());
        }
        let result = f().map_err(|e| e.to_string());
        wasapi::deinitialize();
        result
    });
    match thread.join() {
        Ok(result) => Ok(result?),
        Err(_) => Err("listing the WASAPI exclusive devices failed".into()),
    }
}

/// Lines describing what exclusive mode can open, for the `devices` command.
///
/// # Errors
///
/// If the devices cannot be listed.
pub fn describe_devices() -> Result<Vec<String>, Box<dyn Error>> {
    on_com_thread(list_devices)
}

/// The devices exclusive mode can try to open (the same as shared mode's), with the
/// channels of their shared-mode format.
///
/// # Errors
///
/// If the devices cannot be listed.
pub fn devices() -> Result<DeviceList, Box<dyn Error>> {
    on_com_thread(|| {
        let enumerator = DeviceEnumerator::new()?;
        let mut list = DeviceList::default();
        for (out, direction) in [
            (&mut list.inputs, Direction::Capture),
            (&mut list.outputs, Direction::Render),
        ] {
            let default = enumerator
                .get_default_device(direction)
                .and_then(|d| d.get_id())
                .ok();
            let devices = enumerator.get_device_collection(direction)?;
            for i in 0..devices.get_nbr_devices()? {
                let device = devices.get_device_at_index(i)?;
                let channels = device
                    .get_iaudioclient()
                    .and_then(|c| c.get_mixformat())
                    .map_or(0, |f| f.get_nchannels());
                out.push(DeviceInfo {
                    name: device.get_friendlyname().unwrap_or_default(),
                    channels,
                    is_default: device.get_id().ok() == default,
                });
            }
        }
        Ok(list)
    })
}

fn list_devices() -> Result<Vec<String>, WasapiError> {
    let enumerator = DeviceEnumerator::new()?;
    let mut lines = vec![HOST_NAME.to_owned()];
    for (label, direction) in [
        ("inputs", Direction::Capture),
        ("outputs", Direction::Render),
    ] {
        lines.push(format!("  {label}:"));
        let default = enumerator
            .get_default_device(direction)
            .and_then(|d| d.get_id())
            .ok();
        let devices = enumerator.get_device_collection(direction)?;
        for i in 0..devices.get_nbr_devices()? {
            let device = devices.get_device_at_index(i)?;
            let name = device.get_friendlyname().unwrap_or_default();
            let mark = if device.get_id().ok() == default {
                "*"
            } else {
                " "
            };
            lines.push(format!("   {mark} {name}  ({})", describe(&device)));
        }
    }
    Ok(lines)
}

fn describe(device: &Device) -> String {
    let result = (|| {
        let client = device.get_iaudioclient()?;
        let mix = client.get_mixformat()?;
        let rate = mix.get_samplespersec();
        let channels = mix.get_nchannels();
        let (default, min) = client.get_device_period()?;
        let (_, encoding) = negotiate(&client, rate, channels)?;
        Ok::<_, WasapiError>(format!(
            "{channels} ch, {rate} Hz, {}, period {:.2}–{:.2} ms",
            encoding.sample_format(),
            min as f64 / 10_000.0,
            default as f64 / 10_000.0
        ))
    })();
    result.unwrap_or_else(|e| match hresult(&e) {
        Some(DEVICE_IN_USE) => "in use by another program".to_owned(),
        Some(EXCLUSIVE_MODE_NOT_ALLOWED) => "exclusive mode not allowed".to_owned(),
        _ if matches!(e, WasapiError::UnsupportedFormat) => {
            "no exclusive format at the shared-mode rate".to_owned()
        }
        _ => format!("no exclusive mode: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(encoding: Encoding, x: f32) -> f32 {
        let mut b = vec![0; encoding.bytes()];
        encoding.encode(x, &mut b);
        encoding.decode(&b)
    }

    #[test]
    fn samples_survive_every_encoding() {
        for (encoding, step) in [
            (Encoding::F32, 0.0),
            (Encoding::I32, 1.0 / 2_147_483_648.0),
            (Encoding::I24, 1.0 / 8_388_608.0),
            (Encoding::I16, 1.0 / 32_768.0),
        ] {
            for x in [0.0, 0.5, -0.5, 0.123_456, -0.987_654, 1.0, -1.0] {
                let y = round_trip(encoding, x);
                assert!(
                    (y - x).abs() <= step + 1e-6,
                    "{encoding:?}: {x} came back as {y}"
                );
            }
        }
    }

    #[test]
    fn samples_are_clipped_not_wrapped() {
        for encoding in [Encoding::I32, Encoding::I24, Encoding::I16] {
            assert!(round_trip(encoding, 2.0) > 0.99, "{encoding:?}");
            assert!(round_trip(encoding, -2.0) < -0.99, "{encoding:?}");
        }
    }

    /// Runs `periods` input periods against a driver that takes `chunk` frames every
    /// `every` periods; returns the target and how often the output ran dry.
    fn simulate(fill: &mut FillTarget, chunk: usize, every: usize, periods: usize) -> usize {
        let mut queued = fill.buffer;
        let mut waiting = 0;
        let mut dry = 0;
        for i in 1..=periods {
            if i % every == 0 {
                queued = queued.saturating_sub(chunk);
            }
            waiting += fill.period;
            let step = fill.update(queued, waiting);
            assert!(step.write <= waiting);
            dry += usize::from(step.ran_dry);
            queued += step.write;
            waiting -= step.write;
            assert!(queued <= fill.buffer);
        }
        dry
    }

    #[test]
    fn the_fill_target_settles_on_the_drivers_chunk_plus_a_period() {
        // The Focusrite: 3 ms periods (144 frames), output taken 288 frames every 6 ms.
        let mut fill = FillTarget::new(144, 1152);
        let dry = simulate(&mut fill, 288, 2, 2 * LEARN_PERIODS);
        assert_eq!(fill.target, 432);
        assert_eq!(dry, 0);
    }

    #[test]
    fn writes_follow_the_input_once_settled() {
        let mut fill = FillTarget::new(144, 1152);
        simulate(&mut fill, 288, 2, 4 * LEARN_PERIODS);
        // From the drained buffer on, each period writes what came in, not 288 then 0.
        let mut queued = 432 - 288;
        for _ in 0..10 {
            let step = fill.update(queued, 144);
            assert_eq!(step.write, 144);
            queued += 144;
            if queued == 432 {
                queued -= 288;
            }
        }
    }

    #[test]
    fn running_dry_raises_the_fill_target() {
        let mut fill = FillTarget::new(144, 1152);
        simulate(&mut fill, 144, 1, LEARN_PERIODS);
        assert_eq!(fill.target, 288);
        // The driver starts taking bigger chunks: one gap, then the target has room again.
        let dry = simulate(&mut fill, 576, 4, 100);
        assert!(dry >= 1, "{dry}");
        assert!(fill.target > 576, "{}", fill.target);
        assert_eq!(simulate(&mut fill, 576, 4, 100), 0);
    }

    #[test]
    fn packed_24_bit_is_little_endian_and_signed() {
        let mut b = [0; 3];
        Encoding::I24.encode(-1.0 / 8_388_607.0, &mut b);
        assert_eq!(b, [0xFF, 0xFF, 0xFF]);
        assert!(Encoding::I24.decode(&[0x00, 0x00, 0x80]) <= -0.999_999);
        assert!((Encoding::I24.decode(&[0x00, 0x00, 0x40]) - 0.5).abs() < 1e-6);
    }
}
