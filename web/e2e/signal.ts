// Generates the WAV file Chrome plays as its fake microphone in the smoke tests.
//
// The pattern loops every 2 s: a plucked A2 (decaying harmonics, peak NOTE_PEAK) for 1 s,
// then 1 s of digital silence. The silence lets the tests check that the output really
// follows the input instead of producing sound on its own.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';

export const SAMPLE_RATE = 48000;
/** Peak level of the note, linear. */
export const NOTE_PEAK = 0.5;

const NOTE_HZ = 110;
const DECAY_S = 0.35;
const NOTE_S = 1;
const SILENCE_S = 1;
const HARMONICS = [1, 0.6, 0.4, 0.25, 0.15, 0.1];

function pattern(): Float32Array {
  const noteLen = NOTE_S * SAMPLE_RATE;
  const out = new Float32Array((NOTE_S + SILENCE_S) * SAMPLE_RATE);
  let peak = 0;
  for (let i = 0; i < noteLen; i++) {
    const t = i / SAMPLE_RATE;
    // Short fade in and out, so the loop has no clicks.
    const fade = Math.min(1, i / 48, (noteLen - i) / 480);
    let v = 0;
    HARMONICS.forEach((a, h) => {
      v += a * Math.sin(2 * Math.PI * NOTE_HZ * (h + 1) * t);
    });
    out[i] = v * Math.exp(-t / DECAY_S) * fade;
    peak = Math.max(peak, Math.abs(out[i] ?? 0));
  }
  for (let i = 0; i < noteLen; i++) out[i] = ((out[i] ?? 0) / peak) * NOTE_PEAK;
  return out;
}

/** Writes the fake-microphone signal as 16-bit stereo PCM (identical channels) to `path`. */
export function writeFakeMicWav(path: string): void {
  const samples = pattern();
  const channels = 2;
  const dataBytes = samples.length * channels * 2;
  const buf = Buffer.alloc(44 + dataBytes);
  buf.write('RIFF', 0, 'ascii');
  buf.writeUInt32LE(36 + dataBytes, 4);
  buf.write('WAVE', 8, 'ascii');
  buf.write('fmt ', 12, 'ascii');
  buf.writeUInt32LE(16, 16);
  buf.writeUInt16LE(1, 20); // PCM
  buf.writeUInt16LE(channels, 22);
  buf.writeUInt32LE(SAMPLE_RATE, 24);
  buf.writeUInt32LE(SAMPLE_RATE * channels * 2, 28);
  buf.writeUInt16LE(channels * 2, 32);
  buf.writeUInt16LE(16, 34);
  buf.write('data', 36, 'ascii');
  buf.writeUInt32LE(dataBytes, 40);
  let offset = 44;
  for (const s of samples) {
    const v = Math.round(Math.max(-1, Math.min(1, s)) * 32767);
    for (let c = 0; c < channels; c++) {
      buf.writeInt16LE(v, offset);
      offset += 2;
    }
  }
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, buf);
}
