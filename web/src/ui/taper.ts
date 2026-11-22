// Knob tapers: map a parameter value to a knob position in [0, 1] and back.

import type { ParamSpec } from '../params';

export type Taper = 'linear' | 'log';

export function toPosition(sp: ParamSpec, value: number, taper: Taper = 'linear'): number {
  const v = Math.min(sp.max, Math.max(sp.min, value));
  const t =
    taper === 'log' && sp.min > 0
      ? Math.log(v / sp.min) / Math.log(sp.max / sp.min)
      : (v - sp.min) / (sp.max - sp.min);
  return Math.min(1, Math.max(0, t));
}

export function fromPosition(sp: ParamSpec, position: number, taper: Taper = 'linear'): number {
  const t = Math.min(1, Math.max(0, position));
  const v =
    taper === 'log' && sp.min > 0
      ? sp.min * Math.pow(sp.max / sp.min, t)
      : sp.min + t * (sp.max - sp.min);
  return sp.stepped ? Math.round(v) : v;
}
