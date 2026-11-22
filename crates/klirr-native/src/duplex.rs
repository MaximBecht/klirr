//! The audio side of the native host: joins an input stream and an output stream into one
//! processing path.
//!
//! cpal opens input and output as two streams with their own callbacks. The input callback
//! pushes the selected channel into a lock-free ring; the output callback pulls from it, runs
//! the engine and writes every output channel. The ring is part of the latency, so the
//! [`Processor`] keeps it as short as it safely can:
//!
//! - The streams start with an arbitrary offset between their callbacks. Once per window the
//!   processor looks at the smallest amount of input that was waiting beyond what the output
//!   needed (the *slack*) and drops whatever exceeds the safety margin.
//! - If the output finds too little input (an underrun), it plays one block of silence, counts
//!   a dropout and raises the margin, so the same gap does not happen again.
//!
//! With ASIO both callbacks run on the driver's thread, input first, so the slack is zero.
//! With WASAPI they run on two threads and the slack is whatever their phase offset needs.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use klirr_dsp::Engine;

use crate::probe::{LoopbackProbe, ProbeResult};

/// The engine runs in pieces of at most this many frames.
const CHUNK: usize = 256;
/// Length of the window over which the slack is judged, in seconds.
const WINDOW_S: f32 = 0.5;
/// Slack this close to the margin is left alone (no trim for a few frames).
const TRIM_TOLERANCE: usize = 8;
/// Extra frames added to the margin after an underrun, on top of the shortfall.
const UNDERRUN_HEADROOM: usize = 16;
/// How often meters are taken from the engine.
const METER_HZ: f32 = 30.0;

/// Main thread → audio thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Start a loopback measurement. It replaces the amp's output while it runs.
    Measure,
}

/// Figures the audio threads publish for the main thread. Reading them costs the audio
/// nothing; every field is a plain atomic.
#[derive(Debug, Default)]
pub struct Stats {
    /// Times the output found too little input (each one is a short gap of silence).
    pub underruns: AtomicU32,
    /// Times excess input was dropped to bring the latency down.
    pub trims: AtomicU32,
    /// Times the input found the ring full (the output stopped pulling).
    pub overflows: AtomicU32,
    /// Errors the backend reported (xruns, device changes).
    pub backend_errors: AtomicU32,
    /// Smallest slack in the last window, in frames.
    pub slack_frames: AtomicU32,
    /// The current safety margin, in frames.
    pub margin_frames: AtomicU32,
    /// Frames of the latest input / output callback.
    pub input_block: AtomicU32,
    /// See [`Stats::input_block`].
    pub output_block: AtomicU32,
    /// Capture → input callback, as the backend reports it, in µs.
    pub input_delay_us: AtomicU32,
    /// Output callback → playback, as the backend reports it, in µs.
    pub output_delay_us: AtomicU32,
    /// Highest share of the callback's time budget spent in the output callback, in ‰.
    pub load_permille: AtomicU32,
    /// Peak input level since the last read (f32 bits).
    pub input_peak: AtomicU32,
    /// Peak output level since the last read (f32 bits).
    pub output_peak: AtomicU32,
    /// Largest limiter gain reduction since the last read, in dB (f32 bits).
    pub limiter_db: AtomicU32,
}

impl Stats {
    /// Raises an f32 stored as bits to at least `value`. Only for values ≥ 0, whose bit
    /// patterns order like the numbers.
    pub fn raise(cell: &AtomicU32, value: f32) {
        cell.fetch_max(value.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// Reads an f32 stored as bits and starts over at 0.
    pub fn take(cell: &AtomicU32) -> f32 {
        f32::from_bits(cell.swap(0, Ordering::Relaxed))
    }
}

/// The output side's processing: owns the engine, the reading end of the ring and the probe.
/// Real-time safe: [`Processor::process`] never allocates, locks or blocks.
pub struct Processor {
    engine: Engine,
    input: rtrb::Consumer<f32>,
    commands: rtrb::Consumer<Command>,
    results: rtrb::Producer<ProbeResult>,
    stats: Arc<Stats>,
    probe: LoopbackProbe,
    probing: bool,
    scratch_in: Box<[f32]>,
    scratch_out: Box<[f32]>,
    /// Input has arrived at least once; underruns before that are just the start.
    started: bool,
    /// The first window since input started is over. Until then the streams are still
    /// finding their rhythm: underruns are counted but do not raise the margin.
    settled: bool,
    margin: usize,
    window_len: usize,
    window_pos: usize,
    window_min: usize,
    meter_len: usize,
    meter_pos: usize,
}

impl Processor {
    /// A processor reading from `input`, keeping at least `margin` frames of slack.
    pub fn new(
        engine: Engine,
        input: rtrb::Consumer<f32>,
        commands: rtrb::Consumer<Command>,
        results: rtrb::Producer<ProbeResult>,
        stats: Arc<Stats>,
        margin: usize,
    ) -> Self {
        let sample_rate = engine.sample_rate();
        stats.margin_frames.store(margin as u32, Ordering::Relaxed);
        Self {
            engine,
            input,
            commands,
            results,
            stats,
            probe: LoopbackProbe::new(sample_rate),
            probing: false,
            scratch_in: vec![0.0; CHUNK].into_boxed_slice(),
            scratch_out: vec![0.0; CHUNK].into_boxed_slice(),
            started: false,
            settled: false,
            margin,
            window_len: (WINDOW_S * sample_rate) as usize,
            window_pos: 0,
            window_min: usize::MAX,
            meter_len: (sample_rate / METER_HZ) as usize,
            meter_pos: 0,
        }
    }

    /// Fills `out` (one channel) with the next block of output.
    pub fn process(&mut self, out: &mut [f32]) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Measure => {
                    self.probe.restart();
                    self.probing = true;
                }
            }
        }

        let n = out.len();
        let level = self.input.slots();
        if !self.started {
            if level == 0 {
                out.fill(0.0);
                return;
            }
            self.started = true;
        }
        if level < n {
            // Too little input: play silence and keep what is there, so the input that is
            // late now is played (later) instead of skipped. The ring is a block longer from
            // here on; the margin grows so the trim does not take that back.
            out.fill(0.0);
            self.stats.underruns.fetch_add(1, Ordering::Relaxed);
            if self.settled {
                self.margin += n - level + UNDERRUN_HEADROOM;
                self.stats
                    .margin_frames
                    .store(self.margin as u32, Ordering::Relaxed);
            }
            self.advance_window(n);
            return;
        }
        self.window_min = self.window_min.min(level - n);

        let mut done = 0;
        while done < n {
            let len = (n - done).min(CHUNK);
            let Ok(chunk) = self.input.read_chunk(len) else {
                unreachable!("the level was checked above");
            };
            let (a, b) = chunk.as_slices();
            self.scratch_in[..a.len()].copy_from_slice(a);
            self.scratch_in[a.len()..len].copy_from_slice(b);
            chunk.commit_all();

            let input = &self.scratch_in[..len];
            self.engine.process(input, &mut self.scratch_out[..len]);
            let o = &mut out[done..done + len];
            if self.probing {
                for (y, &x) in o.iter_mut().zip(input) {
                    *y = self.probe.tick(x);
                }
                if let Some(result) = self.probe.result() {
                    self.probing = false;
                    // Drop the clicks the amp heard through the cable (delay and reverb tails).
                    self.engine.reset();
                    // The main thread reads results long before the ring could fill up.
                    let _ = self.results.push(result);
                }
            } else {
                o.copy_from_slice(&self.scratch_out[..len]);
            }
            done += len;
        }

        self.meter_pos += n;
        if self.meter_pos >= self.meter_len {
            self.meter_pos = 0;
            let m = self.engine.take_meters();
            Stats::raise(&self.stats.input_peak, m.input_peak);
            Stats::raise(&self.stats.output_peak, m.output_peak);
            Stats::raise(&self.stats.limiter_db, m.limiter_reduction_db);
        }

        self.advance_window(n);
    }

    fn advance_window(&mut self, frames: usize) {
        self.window_pos += frames;
        if self.window_pos >= self.window_len {
            self.end_window();
        }
    }

    fn end_window(&mut self) {
        let slack = self.window_min;
        self.settled = true;
        self.window_pos = 0;
        self.window_min = usize::MAX;
        if slack == usize::MAX {
            return;
        }
        // Never trim during a measurement: it would shift the clicks.
        if !self.probing && slack > self.margin + TRIM_TOLERANCE {
            let excess = slack - self.margin;
            if let Ok(chunk) = self.input.read_chunk(excess) {
                chunk.commit_all();
                self.stats.trims.fetch_add(1, Ordering::Relaxed);
            }
            self.stats
                .slack_frames
                .store(self.margin as u32, Ordering::Relaxed);
        } else {
            self.stats
                .slack_frames
                .store(slack as u32, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::PINGS;

    const SR: usize = 48_000;

    struct Rig {
        processor: Processor,
        input: rtrb::Producer<f32>,
        commands: rtrb::Producer<Command>,
        results: rtrb::Consumer<ProbeResult>,
        stats: Arc<Stats>,
        /// Output samples waiting to come back through the simulated cable.
        cable: std::collections::VecDeque<f32>,
        connected: bool,
    }

    impl Rig {
        /// A cable that returns the output `delay` frames later.
        fn new(margin: usize, delay: usize) -> Self {
            let (input, consumer) = rtrb::RingBuffer::new(SR);
            let (commands, command_rx) = rtrb::RingBuffer::new(4);
            let (result_tx, results) = rtrb::RingBuffer::new(4);
            let stats = Arc::new(Stats::default());
            let processor = Processor::new(
                Engine::new(SR as f32),
                consumer,
                command_rx,
                result_tx,
                stats.clone(),
                margin,
            );
            Self {
                processor,
                input,
                commands,
                results,
                stats,
                cable: std::iter::repeat_n(0.0, delay).collect(),
                connected: true,
            }
        }

        /// Captures `n` frames of input from the cable.
        fn capture(&mut self, n: usize) {
            for _ in 0..n {
                let x = self.cable.pop_front().unwrap_or(0.0);
                self.input
                    .push(if self.connected { x } else { 0.0 })
                    .unwrap();
            }
        }

        /// Renders `n` frames of output into the cable.
        fn render(&mut self, n: usize) {
            let mut out = vec![0.0; n];
            self.processor.process(&mut out);
            self.cable.extend(out);
        }

        fn measure(&mut self, block: usize, input_first: bool) -> ProbeResult {
            self.commands.push(Command::Measure).unwrap();
            for _ in 0..10 * SR / block {
                self.cycle(block, input_first);
                if let Ok(r) = self.results.pop() {
                    return r;
                }
            }
            panic!("no result");
        }

        fn cycle(&mut self, block: usize, input_first: bool) {
            if input_first {
                self.capture(block);
                self.render(block);
            } else {
                self.render(block);
                self.capture(block);
            }
        }
    }

    #[test]
    fn asio_style_callbacks_need_no_slack() {
        // Input callback first, then output, in the same cycle: the round trip is the cable.
        let mut rig = Rig::new(0, 300);
        for _ in 0..SR / 128 {
            rig.cycle(128, true);
        }
        assert_eq!(rig.stats.underruns.load(Ordering::Relaxed), 0);
        let ProbeResult::Done { round_trips, count } = rig.measure(128, true) else {
            panic!("expected a measurement");
        };
        assert_eq!(count, PINGS);
        assert!(round_trips.iter().all(|&f| f == 300), "{round_trips:?}");
    }

    #[test]
    fn excess_input_is_trimmed_to_the_margin() {
        let mut rig = Rig::new(32, 300);
        // A start-up offset between the streams: input that arrived before the output started.
        for _ in 0..1000 {
            rig.input.push(0.0).unwrap();
        }
        for _ in 0..2 * SR / 128 {
            rig.cycle(128, true);
        }
        assert_eq!(rig.stats.slack_frames.load(Ordering::Relaxed), 32);
        assert_eq!(rig.stats.trims.load(Ordering::Relaxed), 1);
        let ProbeResult::Done { round_trips, .. } = rig.measure(128, true) else {
            panic!("expected a measurement");
        };
        // The ring's slack is part of the measured round trip.
        assert!(round_trips.iter().all(|&f| f == 332), "{round_trips:?}");
    }

    #[test]
    fn an_underrun_raises_the_margin_once() {
        let mut rig = Rig::new(0, 300);
        for _ in 0..SR / 128 {
            rig.cycle(128, true);
        }
        // From here on the output runs first and finds only part of a block waiting.
        rig.capture(64);
        for _ in 0..SR / 128 {
            rig.cycle(128, false);
        }
        assert_eq!(rig.stats.underruns.load(Ordering::Relaxed), 1);
        let margin = rig.stats.margin_frames.load(Ordering::Relaxed);
        assert!(margin >= 64, "{margin}");
        let ProbeResult::Done { round_trips, .. } = rig.measure(128, false) else {
            panic!("expected a measurement");
        };
        assert!(round_trips.iter().all(|&f| f == round_trips[0]));
        assert_eq!(rig.stats.underruns.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn underruns_while_starting_leave_the_margin_alone() {
        let mut rig = Rig::new(0, 300);
        rig.capture(64);
        for _ in 0..SR / 128 {
            rig.cycle(128, false);
        }
        assert_eq!(rig.stats.underruns.load(Ordering::Relaxed), 1);
        assert_eq!(rig.stats.margin_frames.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn no_signal_without_a_cable() {
        let mut rig = Rig::new(0, 300);
        rig.connected = false;
        assert_eq!(rig.measure(128, true), ProbeResult::NoSignal);
    }
}
