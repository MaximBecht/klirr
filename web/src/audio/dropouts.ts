// Detects when the audio falls behind for good.
//
// `AudioContext.currentTime` advances by exactly the audio the engine has rendered. While
// audio runs smoothly it keeps pace with the wall clock, so the difference between the two
// stays put (apart from tiny clock drift). When the audio thread misses its deadlines and does
// not catch up afterwards, the audio clock falls behind and stays behind: the input that kept
// arriving in the meantime now waits in a buffer, and that wait is added to the latency from
// then on. This is how latency grows while you play.
//
// A stall the browser catches up on (rendering faster for a moment) adds no latency and is not
// counted.
//
// Sampling reads two numbers on the main thread: no cost to the audio. Readings are noisy (the
// browser renders in bursts and updates `currentTime` in steps), and the noise only ever makes
// the audio clock look further behind. So each window keeps its smallest reading, which is
// close to the truth.

/** A lasting slip at least this large counts; anything smaller is drift or noise. */
const SLIP_MS = 10;
/** Length of one window of readings. */
const WINDOW_MS = 1000;
/** Readings from the first seconds are ignored: the clocks settle while audio starts. */
const WARMUP_S = 2;

export class DropoutWatch {
  /** Number of lasting slips seen. */
  count = 0;
  /** Latency they added in total, in ms. */
  addedMs = 0;
  /** The settled difference between wall clock and audio clock, in ms. */
  private baseline: number | null = null;
  /** A slip seen in the last window, waiting for the next window to confirm it. */
  private pending = false;
  private windowStart = 0;
  private windowMin = Infinity;

  /**
   * Feeds one reading: the context's `currentTime` (s) and `performance.now()` (ms), read
   * together. Call it often (about ten times a second). Returns true when it confirms a slip.
   */
  feed(contextTime: number, wallMs: number): boolean {
    if (!(contextTime >= WARMUP_S) || !Number.isFinite(wallMs)) return false;
    const lag = wallMs - contextTime * 1000;
    if (this.windowMin === Infinity) this.windowStart = wallMs;
    this.windowMin = Math.min(this.windowMin, lag);
    if (wallMs - this.windowStart < WINDOW_MS) return false;
    const settled = this.windowMin;
    this.windowMin = Infinity;
    return this.judge(settled);
  }

  private judge(lag: number): boolean {
    if (this.baseline === null) {
      this.baseline = lag;
      return false;
    }
    const slip = lag - this.baseline;
    if (slip < SLIP_MS) {
      // Smooth running, slow drift, or a slip that did not last: follow it.
      this.baseline = lag;
      this.pending = false;
      return false;
    }
    if (!this.pending) {
      this.pending = true;
      return false;
    }
    this.count++;
    this.addedMs += slip;
    this.baseline = lag;
    this.pending = false;
    return true;
  }

  /** Starts over. */
  reset(): void {
    this.count = 0;
    this.addedMs = 0;
    this.baseline = null;
    this.pending = false;
    this.windowMin = Infinity;
  }
}
