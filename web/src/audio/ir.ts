// Decoding impulse responses in the page, for both engines: the browser's decoder reads
// every format it knows, and the file never leaves the machine.

import { wavSampleRate } from './wav';

export interface Ir {
  name: string;
  samples: Float32Array<ArrayBuffer>;
  sampleRate: number;
}

/**
 * Decodes the first channel of an impulse response at the file's own rate (the engine
 * resamples once, to its rate). `fallbackRate` is used for formats whose rate is not read
 * from the header.
 */
export async function decodeIr(
  name: string,
  bytes: ArrayBuffer,
  fallbackRate: number,
): Promise<Ir> {
  const rate = wavSampleRate(bytes) ?? fallbackRate;
  const decoder = new OfflineAudioContext(1, 1, Math.min(384000, Math.max(8000, rate)));
  const buffer = await decoder.decodeAudioData(bytes);
  return { name, samples: buffer.getChannelData(0).slice(), sampleRate: buffer.sampleRate };
}
