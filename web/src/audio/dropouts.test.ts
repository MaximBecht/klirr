import { describe, expect, it } from 'vitest';
import { DropoutWatch } from './dropouts';

/**
 * Feeds ten readings a second for `seconds`. `behindMs(t)` is how far the audio clock is
 * behind the wall clock at wall time t (s), before noise. `noiseMs(i)` (≥ 0) makes reading i
 * look further behind, as stale `currentTime` values do.
 */
function run(
  seconds: number,
  behindMs: (t: number) => number,
  noiseMs: (i: number) => number = () => 0,
): DropoutWatch {
  const watch = new DropoutWatch();
  for (let i = 0; i <= seconds * 10; i++) {
    const wall = i * 100;
    const audio = (wall - behindMs(wall / 1000) - noiseMs(i)) / 1000;
    watch.feed(audio, 3000 + wall);
  }
  return watch;
}

/** Pseudo-random but repeatable noise in [0, max). */
const noise = (max: number) => (i: number) => (((i * 7919) % 101) / 101) * max;

describe('dropout watch', () => {
  it('stays at zero while audio keeps pace, despite drift and noisy readings', () => {
    // 100 ppm of drift (0.1 ms/s) for ten minutes; readings up to 25 ms stale.
    const w = run(600, (t) => 0.1 * t, noise(25));
    expect(w.count).toBe(0);
  });

  it('counts a lasting slip once, with the latency it added', () => {
    const w = run(20, (t) => (t >= 8.5 ? 40 : 0), noise(5));
    expect(w.count).toBe(1);
    expect(w.addedMs).toBeGreaterThan(35);
    expect(w.addedMs).toBeLessThan(45);
  });

  it('counts separate slips separately', () => {
    const w = run(30, (t) => (t >= 5.5 ? 20 : 0) + (t >= 15.5 ? 30 : 0));
    expect(w.count).toBe(2);
    expect(w.addedMs).toBeCloseTo(50, 6);
  });

  it('ignores a stall the browser catches up on', () => {
    const w = run(20, (t) => (t >= 6 && t < 6.4 ? 150 : 0));
    expect(w.count).toBe(0);
  });

  it('ignores the first seconds while the clocks settle', () => {
    const w = run(20, (t) => (t < 1.5 ? 0 : 60));
    expect(w.count).toBe(0);
  });
});
