import { describe, expect, it } from 'vitest';
import { LoopbackProbe, PINGS, summarise, type ProbeResult } from './probe';

const SR = 48000;
const BLOCK = 128;

/**
 * Runs the probe against a simulated cable: what it outputs comes back `delay` frames later,
 * scaled by `gain`, plus `noise(t)`. Returns the probe's result.
 */
function run(delay: number, gain = 0.3, noise: (t: number) => number = () => 0): ProbeResult {
  const probe = new LoopbackProbe(SR);
  const line = new Float32Array(delay + BLOCK);
  const input = new Float32Array(BLOCK);
  const output = new Float32Array(BLOCK);
  let t = 0;
  for (let block = 0; block < 10 * (SR / BLOCK); block++) {
    for (let i = 0; i < BLOCK; i++) input[i] = (line[(t + i) % line.length] ?? 0) + noise(t + i);
    const running = probe.process(input, output);
    for (let i = 0; i < BLOCK; i++) {
      line[(t + i) % line.length] = 0;
      line[(t + i + delay) % line.length] = (output[i] ?? 0) * gain;
    }
    t += BLOCK;
    if (!running) break;
  }
  const result = probe.result;
  if (!result) throw new Error('the probe did not finish');
  return result;
}

describe('loopback probe', () => {
  it('measures the round trip to the frame, whatever the block alignment', () => {
    // A real round trip is at least one block (the simulated cable needs that too).
    for (const delay of [128, 129, 255, 1000, 2411]) {
      const result = run(delay);
      expect(result).toEqual({ status: 'done', roundTrips: Array<number>(PINGS).fill(delay) });
    }
  });

  it('stops early when nothing comes back', () => {
    expect(run(500, 0)).toEqual({ status: 'no-signal' });
  });

  it('refuses a noisy input', () => {
    expect(run(500, 0.3, () => 0.2)).toEqual({ status: 'noisy' });
  });

  it('refuses a note that starts ringing after the first click', () => {
    const lateNote = (t: number) => (t > SR ? 0.3 * Math.sin((2 * Math.PI * 110 * t) / SR) : 0);
    expect(run(500, 0.3, lateNote)).toEqual({ status: 'noisy' });
  });

  it('hears a quiet loopback above a low noise floor', () => {
    const hum = (t: number) => 0.004 * Math.sin((2 * Math.PI * 50 * t) / SR);
    const result = run(700, 0.05, hum);
    expect(result.status).toBe('done');
    if (result.status === 'done') for (const f of result.roundTrips) expect(f).toBe(700);
  });
});

describe('summarise', () => {
  it('adds the engine latency', () => {
    const m = summarise({ status: 'done', roundTrips: [960, 960, 961, 960] }, SR, 144);
    expect(m.ok).toBe(true);
    if (!m.ok) return;
    expect(m.roundTripMs).toBeCloseTo(20.005, 3);
    expect(m.totalMs).toBeCloseTo(23.005, 3);
    expect(m.hits).toBe(4);
  });

  it('drops outliers and rejects scattered results', () => {
    const ok = summarise({ status: 'done', roundTrips: [960, 962, 5000, 961] }, SR, 0);
    expect(ok.ok && ok.hits).toBe(3);
    const bad = summarise({ status: 'done', roundTrips: [500, 2000, 9000, 20000] }, SR, 0);
    expect(bad).toEqual({ ok: false, reason: 'inconsistent' });
  });

  it('passes failures through', () => {
    expect(summarise({ status: 'noisy' }, SR, 0)).toEqual({ ok: false, reason: 'noisy' });
  });
});
