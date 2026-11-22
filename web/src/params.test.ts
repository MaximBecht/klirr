import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { instantiate } from './audio/dsp';
import { PARAMS, P, sanitise } from './params';

const wasm = readFileSync(new URL('./wasm/klirr.wasm', import.meta.url));

describe('parameter table', () => {
  it('matches the engine (run `npm run build:wasm` first)', async () => {
    const x = await instantiate(wasm);
    expect(x.klirr_param_count()).toBe(PARAMS.length);
    PARAMS.forEach((sp, i) => {
      expect(sp.index).toBe(i);
      expect(x.klirr_param_min(i), sp.name).toBeCloseTo(sp.min, 5);
      expect(x.klirr_param_max(i), sp.name).toBeCloseTo(sp.max, 5);
      expect(x.klirr_param_default(i), sp.name).toBeCloseTo(sp.default, 5);
      expect(x.klirr_param_stepped(i) === 1, sp.name).toBe(sp.stepped);
    });
  });

  it('sanitises like the core', () => {
    expect(sanitise(P.Gain, 2)).toBe(1);
    expect(sanitise(P.Channel, 1.4)).toBe(1);
    expect(sanitise(P.MasterDb, -Infinity)).toBe(-60);
    expect(sanitise(P.MasterDb, NaN)).toBe(-6);
  });
});
