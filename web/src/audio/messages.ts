// Messages between the main thread and the AudioWorklet (over the node's MessagePort).

import type { Meters } from './dsp';

export const PROCESSOR_NAME = 'klirr';

/** Main thread → worklet. */
export type ToWorklet =
  | { type: 'init'; wasm: ArrayBuffer; params: number[]; inputChannel: number }
  | { type: 'params'; changes: [index: number, value: number][] }
  | { type: 'input-channel'; channel: number }
  | { type: 'ir'; samples: Float32Array; sampleRate: number };

/** Worklet → main thread. */
export type FromWorklet =
  | { type: 'ready'; latencySamples: number }
  | { type: 'error'; message: string }
  | ({ type: 'meters' } & Meters)
  | { type: 'ir-result'; code: number };

/** How often the worklet reports meters. */
export const METER_HZ = 30;
