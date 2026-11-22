// Engine parameters as the web app sees them. Indices, ranges and defaults mirror
// `klirr_dsp::Param`; `params.test.ts` checks them against the built wasm module.

export const P = {
  InputGainDb: 0,
  GateThresholdDb: 1,
  Channel: 2,
  Gain: 3,
  BassDb: 4,
  MidDb: 5,
  TrebleDb: 6,
  Cabinet: 7,
  MasterDb: 8,
  DelayTimeMs: 9,
  DelayFeedback: 10,
  DelayMix: 11,
  ReverbSize: 12,
  ReverbDamping: 13,
  ReverbMix: 14,
  Bypass: 15,
} as const;
export type P = (typeof P)[keyof typeof P];

export interface ParamSpec {
  index: P;
  name: string;
  min: number;
  max: number;
  default: number;
  stepped: boolean;
}

const p = (index: P, name: string, min: number, max: number, def: number): ParamSpec => ({
  index,
  name,
  min,
  max,
  default: def,
  stepped: false,
});
const s = (index: P, name: string, max: number, def: number): ParamSpec => ({
  index,
  name,
  min: 0,
  max,
  default: def,
  stepped: true,
});

/** All parameters, in index order. */
export const PARAMS: readonly ParamSpec[] = [
  p(P.InputGainDb, 'input_gain_db', -24, 24, 0),
  p(P.GateThresholdDb, 'gate_threshold_db', -96, -20, -70),
  s(P.Channel, 'channel', 2, 0),
  p(P.Gain, 'gain', 0, 1, 0.5),
  p(P.BassDb, 'bass_db', -12, 12, 0),
  p(P.MidDb, 'mid_db', -12, 12, 0),
  p(P.TrebleDb, 'treble_db', -12, 12, 0),
  s(P.Cabinet, 'cabinet', 3, 1),
  p(P.MasterDb, 'master_db', -60, 12, -6),
  p(P.DelayTimeMs, 'delay_time_ms', 10, 2000, 300),
  p(P.DelayFeedback, 'delay_feedback', 0, 0.95, 0.35),
  p(P.DelayMix, 'delay_mix', 0, 1, 0),
  p(P.ReverbSize, 'reverb_size', 0, 1, 0.5),
  p(P.ReverbDamping, 'reverb_damping', 0, 1, 0.5),
  p(P.ReverbMix, 'reverb_mix', 0, 1, 0),
  s(P.Bypass, 'bypass', 1, 0),
];

export function spec(index: P): ParamSpec {
  const sp = PARAMS[index];
  if (!sp) throw new Error(`unknown parameter ${index}`);
  return sp;
}

/**
 * Clamps (and for stepped parameters rounds) a value, like `Param::sanitise` in the core.
 * NaN (which the core ignores) becomes the default.
 */
export function sanitise(index: P, value: number): number {
  const sp = spec(index);
  if (Number.isNaN(value)) return sp.default;
  const v = Math.min(sp.max, Math.max(sp.min, value));
  return sp.stepped ? Math.round(v) : v;
}

export function defaults(): number[] {
  return PARAMS.map((sp) => sp.default);
}

export const CHANNEL_NAMES = ['Clean', 'Crunch', 'High gain'] as const;
export const CABINET_NAMES = ['Off', '1×12 combo', '4×12 stack', 'Custom IR'] as const;
export const CABINET_CUSTOM = 3;

/** Gate threshold at which the gate is off (the parameter's minimum). */
export const GATE_OFF_DB = -96;
