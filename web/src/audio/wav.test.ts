import { describe, expect, it } from 'vitest';
import { wavSampleRate } from './wav';

type Chunk = [id: string, length: number, fill?: (v: DataView, offset: number) => void];

function riff(chunks: Chunk[]): ArrayBuffer {
  const size = 12 + chunks.reduce((n, [, len]) => n + 8 + len + (len % 2), 0);
  const b = new ArrayBuffer(size);
  const v = new DataView(b);
  const tag = (o: number, s: string) => {
    for (let i = 0; i < 4; i++) v.setUint8(o + i, s.charCodeAt(i));
  };
  tag(0, 'RIFF');
  v.setUint32(4, size - 8, true);
  tag(8, 'WAVE');
  let o = 12;
  for (const [id, len, fill] of chunks) {
    tag(o, id);
    v.setUint32(o + 4, len, true);
    fill?.(v, o + 8);
    o += 8 + len + (len % 2);
  }
  return b;
}

describe('wavSampleRate', () => {
  it('reads the fmt chunk, skipping others (with padding)', () => {
    const b = riff([
      ['LIST', 5],
      [
        'fmt ',
        16,
        (v, o) => {
          v.setUint32(o + 4, 96000, true);
        },
      ],
    ]);
    expect(wavSampleRate(b)).toBe(96000);
  });

  it('returns null for other data', () => {
    expect(wavSampleRate(new ArrayBuffer(4))).toBeNull();
    const notWav = new TextEncoder().encode('ID3 this is not a wav file');
    expect(wavSampleRate(notWav.buffer)).toBeNull();
    expect(wavSampleRate(riff([['data', 4]]))).toBeNull();
  });
});
