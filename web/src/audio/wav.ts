// Minimal WAV header reading: just enough to decode an impulse response at its own rate.

/** Sample rate from a RIFF/WAVE file's `fmt ` chunk, or null if it is not a WAV file. */
export function wavSampleRate(bytes: ArrayBuffer): number | null {
  const v = new DataView(bytes);
  if (v.byteLength < 12) return null;
  const tag = (o: number) =>
    String.fromCharCode(v.getUint8(o), v.getUint8(o + 1), v.getUint8(o + 2), v.getUint8(o + 3));
  if ((tag(0) !== 'RIFF' && tag(0) !== 'RF64') || tag(8) !== 'WAVE') return null;
  let o = 12;
  while (o + 8 <= v.byteLength) {
    const id = tag(o);
    const size = v.getUint32(o + 4, true);
    if (id === 'fmt ' && o + 16 <= v.byteLength) {
      const rate = v.getUint32(o + 12, true);
      return rate > 0 ? rate : null;
    }
    o += 8 + size + (size % 2); // chunks are padded to an even size
  }
  return null;
}
