import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { P } from '../params';
import { KlirrDsp, instantiate } from './dsp';

const wasm = readFileSync(new URL('../wasm/klirr.wasm', import.meta.url));

async function dsp(sampleRate = 48000): Promise<KlirrDsp> {
  return new KlirrDsp(await instantiate(wasm), sampleRate);
}

function sine(n: number, freq: number, amp: number, sr = 48000): Float32Array {
  return Float32Array.from({ length: n }, (_, i) => amp * Math.sin((2 * Math.PI * freq * i) / sr));
}

describe('KlirrDsp', () => {
  it('rejects unsupported sample rates', async () => {
    const x = await instantiate(wasm);
    expect(() => new KlirrDsp(x, 100)).toThrow(/sample rate/);
  });

  it('processes audio and reports meters', async () => {
    const d = await dsp();
    d.setParam(P.Channel, 2);
    d.setParam(P.Gain, 0.8);
    expect(d.getParam(P.Channel)).toBe(2);
    const input = sine(48000, 110, 0.3);
    let peak = 0;
    let finite = true;
    for (let i = 0; i < input.length; i += 128) {
      for (const s of d.process(input.subarray(i, i + 128), 128)) {
        finite &&= Number.isFinite(s);
        peak = Math.max(peak, Math.abs(s));
      }
    }
    expect(finite).toBe(true);
    expect(peak).toBeGreaterThan(0.05);
    expect(peak).toBeLessThanOrEqual(1);
    const m = d.takeMeters();
    expect(m.inputPeak).toBeCloseTo(0.3, 2);
    expect(m.outputPeak).toBeCloseTo(peak, 5);
    expect(m.gateOpen).toBe(true);
  });

  it('gives the same output for any block size', async () => {
    const input = sine(4096, 196, 0.4);
    const run = async (block: number) => {
      const d = await dsp();
      d.setParam(P.Channel, 1);
      const out: number[] = [];
      for (let i = 0; i < input.length; i += block) {
        const chunk = input.subarray(i, i + block);
        out.push(...d.process(chunk, chunk.length));
      }
      return out;
    };
    const a = await run(128);
    expect(await run(37)).toEqual(a);
    expect(await run(1024)).toEqual(a);
  });

  it('treats a missing input as silence', async () => {
    const d = await dsp();
    const out = d.process(undefined, 128);
    expect(out.length).toBe(128);
    expect(d.takeMeters().inputPeak).toBe(0);
  });

  it('loads a custom IR, also after memory has grown', async () => {
    const d = await dsp();
    const ir = Float32Array.from({ length: 48000 }, (_, i) => Math.exp(-i / 300) * Math.cos(i));
    expect(d.loadIr(ir, 48000)).toBe(0);
    expect(d.loadIr(new Float32Array(64), 48000)).toBe(4); // silent
    expect(d.loadIr(new Float32Array(0), 48000)).toBe(1); // empty
    d.setParam(P.Cabinet, 3);
    // Views are rebuilt if the loads grew wasm memory.
    const out = d.process(sine(128, 440, 0.2), 128);
    expect(out.every(Number.isFinite)).toBe(true);
  });
});
