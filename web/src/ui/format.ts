// How knob values are shown.

import { GATE_OFF_DB } from '../params';

const MINUS = '\u2212';

function signed(n: number, digits: number): string {
  const s = Math.abs(n).toFixed(digits);
  if (Number(s) === 0) return (0).toFixed(digits);
  return (n < 0 ? MINUS : '+') + s;
}

/** Signed decibels, e.g. "+3.5 dB". */
export function db(v: number): string {
  return `${signed(v, 1)} dB`;
}

/** Whole decibels without a plus sign, e.g. "−6 dB". */
export function level(v: number): string {
  const r = Math.round(v);
  return `${r < 0 ? MINUS + String(-r) : String(r)} dB`;
}

/** Noise-gate threshold: "OFF" at the minimum. */
export function gate(v: number): string {
  return v <= GATE_OFF_DB + 0.5 ? 'OFF' : level(v);
}

/** A 0–1 value on the 0–10 scale amps use, e.g. "6.5". */
export function dial(v: number, max = 1): string {
  return ((v / max) * 10).toFixed(1);
}

/** Milliseconds below one second, seconds above. */
export function time(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(2)} s`;
}

/** A linear peak level as dBFS for display (−∞ for silence). */
export function peakDb(linear: number): number {
  return linear > 0 ? 20 * Math.log10(linear) : -Infinity;
}
