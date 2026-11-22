// Loopback latency measurement.
//
// With a cable from the interface's output to its input, the probe plays short clicks and
// counts the frames until each click comes back. That round trip covers everything outside
// the engine: output buffers, driver, DAC, cable, ADC, input buffers and the browser's own
// plumbing. Added to the engine's own latency it is the real string-to-ear figure.
//
// The probe only runs on request. While it runs it replaces the amp's output; otherwise it
// is not in the signal path at all.

/** Number of clicks played. */
export const PINGS = 6;
/** Level of each click (a single sample), linear. */
const CLICK_LEVEL = 0.5;
/** Silence before the first click, so the amp's tail has died away. */
const SETTLE_S = 0.4;
/** The last part of the settle time measures the noise floor. */
const NOISE_S = 0.2;
/** A click that has not come back after this long is counted as lost. */
const LISTEN_S = 1;
/** Pause after each click, so its ringing has decayed before the next one. */
const GAP_S = 0.25;
/**
 * The input must stay below the threshold for this long before each click; otherwise a
 * ringing string would "answer" the click at once.
 */
const QUIET_S = 0.1;
const MIN_THRESHOLD = 0.01;
/** An input louder than this before the first click is too noisy to measure. */
const MAX_NOISE = 0.08;

export type ProbeResult =
  { status: 'done'; roundTrips: number[] } | { status: 'noisy' } | { status: 'no-signal' };

type Phase = 'settle' | 'listen' | 'gap';

/** Runs one measurement, one audio block at a time. Never allocates while running. */
export class LoopbackProbe {
  private readonly settleFrames: number;
  private readonly noiseFrames: number;
  private readonly listenFrames: number;
  private readonly gapFrames: number;
  private readonly quietFrames: number;
  private readonly hits = new Float64Array(PINGS);
  private hitCount = 0;
  private pings = 0;
  private misses = 0;
  private phase: Phase = 'settle';
  /** Frames since the current phase started. */
  private t = 0;
  private noisePeak = 0;
  private threshold = MIN_THRESHOLD;
  private outcome: ProbeResult | null = null;

  constructor(sampleRate: number) {
    this.settleFrames = Math.round(SETTLE_S * sampleRate);
    this.noiseFrames = Math.round(NOISE_S * sampleRate);
    this.listenFrames = Math.round(LISTEN_S * sampleRate);
    this.gapFrames = Math.round(GAP_S * sampleRate);
    this.quietFrames = Math.round(QUIET_S * sampleRate);
  }

  /** The result, once the measurement is over. */
  get result(): ProbeResult | null {
    return this.outcome;
  }

  /**
   * Reads one block of input and writes the probe's output (silence and clicks) over
   * `output`. A missing input counts as silence. Returns false once the measurement is over.
   */
  process(input: Float32Array | undefined, output: Float32Array): boolean {
    output.fill(0);
    for (let i = 0; i < output.length && !this.outcome; i++) {
      const x = Math.abs(input?.[i] ?? 0);
      switch (this.phase) {
        case 'settle':
          if (this.t >= this.settleFrames - this.noiseFrames)
            this.noisePeak = Math.max(this.noisePeak, x);
          if (++this.t >= this.settleFrames) {
            if (this.noisePeak > MAX_NOISE) {
              this.outcome = { status: 'noisy' };
              break;
            }
            this.threshold = Math.max(MIN_THRESHOLD, this.noisePeak * 4);
            this.click(output, i);
          }
          break;
        case 'listen':
          // `t` counts frames since the click, which can come back one frame later at the
          // earliest.
          this.t++;
          if (x >= this.threshold) {
            this.hits[this.hitCount++] = this.t;
            this.next(true);
          } else if (this.t > this.listenFrames) {
            this.next(false);
          }
          break;
        case 'gap':
          if (this.t >= this.gapFrames - this.quietFrames && x >= this.threshold) {
            this.outcome = { status: 'noisy' };
            break;
          }
          if (++this.t >= this.gapFrames) this.click(output, i);
          break;
      }
    }
    return this.outcome === null;
  }

  private click(output: Float32Array, i: number): void {
    output[i] = CLICK_LEVEL;
    this.pings++;
    this.phase = 'listen';
    this.t = 0;
  }

  private next(hit: boolean): void {
    if (!hit) this.misses++;
    // Without a loopback nothing comes back: stop early instead of waiting for every click.
    if ((this.misses >= 2 && this.hitCount === 0) || this.pings >= PINGS) {
      this.outcome =
        this.hitCount === 0
          ? { status: 'no-signal' }
          : { status: 'done', roundTrips: Array.from(this.hits.subarray(0, this.hitCount)) };
      return;
    }
    this.phase = 'gap';
    this.t = 0;
  }
}

export type Measurement =
  | {
      ok: true;
      /** Output → input round trip outside the engine, in ms. */
      roundTripMs: number;
      /** Round trip plus the engine's own latency: string to ear, in ms. */
      totalMs: number;
      /** Clicks that came back with a consistent delay, out of `PINGS`. */
      hits: number;
    }
  | { ok: false; reason: 'noisy' | 'no-signal' | 'inconsistent' | 'stopped' };

/** Clicks must agree to within this much to count as one measurement. */
const TOLERANCE_MS = 3;
const MIN_HITS = 3;

/** Turns the probe's raw round trips (in frames) into a measurement. */
export function summarise(
  result: ProbeResult,
  sampleRate: number,
  engineLatencySamples: number,
): Measurement {
  if (result.status !== 'done') return { ok: false, reason: result.status };
  const ms = result.roundTrips.map((f) => (f / sampleRate) * 1000).sort((a, b) => a - b);
  const median = ms[Math.floor(ms.length / 2)] ?? 0;
  const agreeing = ms.filter((v) => Math.abs(v - median) <= TOLERANCE_MS);
  if (agreeing.length < MIN_HITS) return { ok: false, reason: 'inconsistent' };
  const roundTripMs = agreeing.reduce((a, b) => a + b, 0) / agreeing.length;
  return {
    ok: true,
    roundTripMs,
    totalMs: roundTripMs + (engineLatencySamples / sampleRate) * 1000,
    hits: agreeing.length,
  };
}
