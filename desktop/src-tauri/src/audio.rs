//! The audio controller: one thread that owns the running [`Duplex`].
//!
//! cpal's streams must stay on the thread that opened them, so the commands do not touch the
//! audio themselves: they send a [`Request`] to the controller and wait for its reply. Between
//! requests the controller sends the meters (30 times a second) and the latency figures (once
//! a second) to the webview, flushes parameter changes the audio thread could not take yet and
//! collects measurement results.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use klirr_dsp::{CustomIr, Param};
use klirr_native::duplex::Stats;
use klirr_native::host::{Duplex, MeasureError, Options};
use klirr_native::probe::Failure;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

/// How often meters are sent while audio runs.
const METER_PERIOD: Duration = Duration::from_millis(33);
/// How often the latency figures are sent while audio runs.
const STATUS_PERIOD: Duration = Duration::from_secs(1);
/// Sample rate custom IRs are checked at while no audio runs (they are prepared again, at
/// the real rate, when audio starts).
const CHECK_RATE: f32 = 48_000.0;

/// What the webview asks for when it engages the amp.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    /// Host id (see [`klirr_native::devices::hosts`]); empty for the default host.
    pub host: String,
    /// Input device name; empty for the default input.
    pub input: String,
    /// Output device name; empty for the default output.
    pub output: String,
    /// Input channel, from 0.
    pub channel: usize,
    /// Every parameter, by index.
    pub params: Vec<f32>,
}

/// What was opened.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    host: String,
    input: String,
    output: String,
    sample_rate: u32,
    input_channels: u16,
    /// Buffer sizes the backend reports, in frames.
    input_buffer: Option<u32>,
    output_buffer: Option<u32>,
    /// The engine's own latency, in samples.
    engine_latency: usize,
}

/// Controller → webview, while audio runs.
#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    /// Levels since the last meters, linear peaks.
    Meters {
        input_peak: f32,
        output_peak: f32,
        gate_open: bool,
        limiter_db: f32,
    },
    /// Latency as the backend reports it, and the dropouts since audio started.
    Status {
        input_ms: f64,
        output_ms: f64,
        engine_ms: f64,
        dropouts: u32,
        added_ms: f64,
    },
    /// The backend reported an error (often: the device was unplugged).
    DeviceError,
}

/// The result of a loopback measurement, shaped like the web app's `Measurement`.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum MeasureReply {
    /// A figure.
    Done {
        ok: bool,
        round_trip_ms: f64,
        total_ms: f64,
        hits: usize,
    },
    /// No figure, and why.
    Failed { ok: bool, reason: &'static str },
}

impl MeasureReply {
    fn failed(reason: &'static str) -> Self {
        Self::Failed { ok: false, reason }
    }
}

type Reply<T> = mpsc::Sender<T>;

enum Request {
    Start(
        Box<StartRequest>,
        Channel<Event>,
        Reply<Result<Started, String>>,
    ),
    Stop(Reply<()>),
    Params(Vec<(Param, f32)>),
    Ir(Vec<f32>, f32, Reply<Result<(), String>>),
    Measure(Reply<MeasureReply>),
}

/// The handle the commands use: sends requests to the controller thread.
pub struct Audio {
    requests: mpsc::Sender<Request>,
}

impl Audio {
    /// Starts the controller thread.
    pub fn spawn() -> Self {
        let (requests, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("klirr-audio-control".to_owned())
            .spawn(move || Controller::default().run(&rx))
            .expect("could not start the audio controller thread");
        Self { requests }
    }

    fn call<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        self.requests
            .send(request(tx))
            .map_err(|_| "the audio controller has stopped")?;
        rx.recv()
            .map_err(|_| "the audio controller has stopped".to_owned())
    }

    /// Opens the devices and starts the amp; stops what ran before.
    pub fn start(&self, request: StartRequest, events: Channel<Event>) -> Result<Started, String> {
        self.call(|reply| Request::Start(Box::new(request), events, reply))?
    }

    /// Stops the amp.
    pub fn stop(&self) -> Result<(), String> {
        self.call(Request::Stop)
    }

    /// Sets parameters (kept for the next start as well).
    pub fn set_params(&self, changes: Vec<(Param, f32)>) -> Result<(), String> {
        self.requests
            .send(Request::Params(changes))
            .map_err(|_| "the audio controller has stopped".to_owned())
    }

    /// Loads a custom IR (mono samples at `rate`), now if audio runs and on every start.
    pub fn load_ir(&self, samples: Vec<f32>, rate: f32) -> Result<(), String> {
        self.call(|reply| Request::Ir(samples, rate, reply))?
    }

    /// Runs a loopback measurement. Takes one to three seconds.
    pub fn measure(&self) -> Result<MeasureReply, String> {
        self.call(Request::Measure)
    }
}

/// The amp while audio runs.
struct Running {
    duplex: Duplex,
    events: Channel<Event>,
    /// The margin between the streams at start; what it grew by since is added latency.
    initial_margin: u32,
    /// Underruns while the streams found their rhythm, which are not dropouts. Taken at the
    /// first status, a second after start.
    startup_underruns: Option<u32>,
    backend_errors: u32,
    measurement: Option<Reply<MeasureReply>>,
    last_meters: Instant,
    last_status: Instant,
}

#[derive(Default)]
struct Controller {
    running: Option<Running>,
    /// Every parameter's latest value, for the next start.
    params: Vec<(Param, f32)>,
    /// Parameters the audio thread has not taken yet.
    unsent: Vec<(Param, f32)>,
    /// The custom IR and its sample rate, for the next start.
    ir: Option<(Vec<f32>, f32)>,
}

impl Controller {
    fn run(mut self, requests: &mpsc::Receiver<Request>) {
        loop {
            match requests.recv_timeout(METER_PERIOD) {
                Ok(request) => self.handle(request),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }
        self.stop();
    }

    fn handle(&mut self, request: Request) {
        match request {
            Request::Start(request, events, reply) => {
                let _ = reply.send(self.start(&request, events));
            }
            Request::Stop(reply) => {
                self.stop();
                let _ = reply.send(());
            }
            Request::Params(changes) => {
                for (param, value) in changes {
                    remember(&mut self.params, param, value);
                    remember(&mut self.unsent, param, value);
                }
            }
            Request::Ir(samples, rate, reply) => {
                let result = match &mut self.running {
                    Some(r) => r.duplex.load_ir(&samples, rate).map_err(|e| e.to_string()),
                    None => CustomIr::new(&samples, rate, CHECK_RATE)
                        .map(drop)
                        .map_err(|e| e.to_string()),
                };
                if result.is_ok() {
                    self.ir = Some((samples, rate));
                }
                let _ = reply.send(result);
            }
            Request::Measure(reply) => match &mut self.running {
                Some(r) => {
                    if r.duplex.start_measure().is_ok() {
                        // A measurement already running starts over; its caller hears "stopped".
                        if let Some(old) = r.measurement.replace(reply) {
                            let _ = old.send(MeasureReply::failed("stopped"));
                        }
                    } else {
                        let _ = reply.send(MeasureReply::failed("stopped"));
                    }
                }
                None => {
                    let _ = reply.send(MeasureReply::failed("stopped"));
                }
            },
        }
    }

    fn start(&mut self, request: &StartRequest, events: Channel<Event>) -> Result<Started, String> {
        self.stop();
        for (index, &value) in request.params.iter().enumerate() {
            if let Some(param) = u32::try_from(index).ok().and_then(Param::from_index) {
                remember(&mut self.params, param, value);
            }
        }
        self.unsent.clear();
        let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        let options = Options {
            host: non_empty(&request.host),
            input: non_empty(&request.input),
            output: non_empty(&request.output),
            channel: request.channel,
            ..Options::default()
        };
        let params = &self.params;
        let ir = &self.ir;
        let duplex = Duplex::open(&options, |engine| {
            for &(param, value) in params {
                engine.set_param(param, value);
            }
            if let Some((samples, rate)) = ir {
                // Checked when it was loaded; it can only fail for a broken sample rate.
                let _ = engine.load_custom_ir(samples, *rate);
            }
        })
        .map_err(|e| e.to_string())?;

        let i = &duplex.info;
        let started = Started {
            host: i.host.to_owned(),
            input: i.input.clone(),
            output: i.output.clone(),
            sample_rate: i.sample_rate,
            input_channels: i.input_channels,
            input_buffer: i.input_buffer,
            output_buffer: i.output_buffer,
            engine_latency: i.engine_latency,
        };
        let now = Instant::now();
        self.running = Some(Running {
            initial_margin: duplex.stats.margin_frames.load(Ordering::Relaxed),
            duplex,
            events,
            startup_underruns: None,
            backend_errors: 0,
            measurement: None,
            last_meters: now,
            last_status: now,
        });
        Ok(started)
    }

    fn stop(&mut self) {
        if let Some(reply) = self.running.take().and_then(|r| r.measurement) {
            let _ = reply.send(MeasureReply::failed("stopped"));
        }
    }

    fn tick(&mut self) {
        let Some(r) = &mut self.running else {
            return;
        };
        self.unsent
            .retain(|&(param, value)| !r.duplex.set_param(param, value));

        if let Some(result) = r.duplex.poll_measure() {
            let reply = match result {
                Ok(m) => MeasureReply::Done {
                    ok: true,
                    round_trip_ms: m.round_trip_ms,
                    total_ms: m.total_ms,
                    hits: m.hits,
                },
                Err(MeasureError::Failed(Failure::NoSignal)) => MeasureReply::failed("no-signal"),
                Err(MeasureError::Failed(Failure::Noisy)) => MeasureReply::failed("noisy"),
                Err(MeasureError::Failed(Failure::Inconsistent)) => {
                    MeasureReply::failed("inconsistent")
                }
                Err(MeasureError::Stalled) => MeasureReply::failed("stopped"),
            };
            if let Some(tx) = r.measurement.take() {
                let _ = tx.send(reply);
            }
        }

        let stats = &r.duplex.stats;
        let now = Instant::now();
        if now - r.last_meters >= METER_PERIOD {
            r.last_meters = now;
            let _ = r.events.send(Event::Meters {
                input_peak: Stats::take(&stats.input_peak),
                output_peak: Stats::take(&stats.output_peak),
                gate_open: stats.gate_open.load(Ordering::Relaxed),
                limiter_db: Stats::take(&stats.limiter_db),
            });
        }
        if now - r.last_status >= STATUS_PERIOD {
            r.last_status = now;
            r.startup_underruns
                .get_or_insert_with(|| stats.underruns.load(Ordering::Relaxed));
            let _ = r.events.send(status(r));
        }
        let errors = stats.backend_errors.load(Ordering::Relaxed);
        if errors > r.backend_errors {
            // Once per run: a flaky device would otherwise repeat it every tick.
            if r.backend_errors == 0 {
                let _ = r.events.send(Event::DeviceError);
            }
            r.backend_errors = errors;
        }
    }
}

/// The latency figures of a running amp.
fn status(r: &Running) -> Event {
    let stats = &r.duplex.stats;
    let rate = f64::from(r.duplex.info.sample_rate);
    let frames_ms = |frames: f64| frames / rate * 1000.0;
    let us_ms =
        |cell: &std::sync::atomic::AtomicU32| f64::from(cell.load(Ordering::Relaxed)) / 1000.0;
    let margin = stats.margin_frames.load(Ordering::Relaxed);
    Event::Status {
        // The input waits in the ring between the streams before the engine takes it.
        input_ms: us_ms(&stats.input_delay_us)
            + frames_ms(f64::from(stats.slack_frames.load(Ordering::Relaxed))),
        output_ms: us_ms(&stats.output_delay_us),
        engine_ms: frames_ms(r.duplex.info.engine_latency as f64),
        dropouts: stats
            .underruns
            .load(Ordering::Relaxed)
            .saturating_sub(r.startup_underruns.unwrap_or(0)),
        added_ms: frames_ms(f64::from(margin.saturating_sub(r.initial_margin))),
    }
}

/// Sets `param` to `value` in `list`, adding it if it is not there.
fn remember(list: &mut Vec<(Param, f32)>, param: Param, value: f32) {
    match list.iter_mut().find(|(p, _)| *p == param) {
        Some(entry) => entry.1 = value,
        None => list.push((param, value)),
    }
}
