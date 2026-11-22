import { describe, expect, it } from 'vitest';
import { P, spec } from '../params';
import * as fmt from './format';
import { fromPosition, toPosition } from './taper';

const MINUS = '−';

describe('tapers', () => {
  it('round-trip linear and log', () => {
    for (const [param, taper] of [
      [P.Gain, 'linear'],
      [P.MasterDb, 'linear'],
      [P.DelayTimeMs, 'log'],
    ] as const) {
      const sp = spec(param);
      for (const t of [0, 0.13, 0.5, 0.99, 1]) {
        expect(toPosition(sp, fromPosition(sp, t, taper), taper)).toBeCloseTo(t, 6);
      }
    }
  });

  it('log taper puts the geometric mean in the middle', () => {
    const sp = spec(P.DelayTimeMs);
    expect(fromPosition(sp, 0.5, 'log')).toBeCloseTo(Math.sqrt(10 * 2000), 6);
    expect(fromPosition(sp, 0, 'log')).toBeCloseTo(10, 6);
    expect(fromPosition(sp, 1, 'log')).toBeCloseTo(2000, 6);
  });

  it('clamps and rounds stepped parameters', () => {
    const sp = spec(P.Cabinet);
    expect(fromPosition(sp, 0.6)).toBe(2);
    expect(toPosition(sp, 99)).toBe(1);
    expect(toPosition(spec(P.Gain), -5)).toBe(0);
  });
});

describe('formatting', () => {
  it('formats values for the panel', () => {
    expect(fmt.db(3.25)).toBe('+3.3 dB');
    expect(fmt.db(-0.01)).toBe('0.0 dB');
    expect(fmt.db(-12)).toBe(`${MINUS}12.0 dB`);
    expect(fmt.level(-6.4)).toBe(`${MINUS}6 dB`);
    expect(fmt.gate(-96)).toBe('OFF');
    expect(fmt.gate(-70)).toBe(`${MINUS}70 dB`);
    expect(fmt.dial(0.65)).toBe('6.5');
    expect(fmt.time(300)).toBe('300 ms');
    expect(fmt.time(1250)).toBe('1.25 s');
    expect(fmt.peakDb(0)).toBe(-Infinity);
    expect(fmt.peakDb(1)).toBe(0);
  });
});
