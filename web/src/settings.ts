// Remembers the user's settings between visits (localStorage, nothing leaves the machine).
// The desktop app keeps them in its settings file instead (see `config.ts`), and reads them
// from here once, on its first run.

import { PARAMS, P, defaults, sanitise } from './params';

export interface Settings {
  params: number[];
  /** Audio host (driver API) of the desktop app; empty for the platform's default. */
  host: string;
  inputDeviceId: string;
  outputDeviceId: string;
  /** Input channel, from 0 (0 = input 1, the left channel of a stereo input). */
  inputChannel: number;
  /**
   * Buffer size the desktop app asks the driver for, in frames; null for the driver's own.
   * Kept in the desktop app's settings file only.
   */
  buffer: number | null;
  /**
   * Sample rate the desktop app asks for, in Hz, where the rate is its to choose (WASAPI
   * exclusive); null for the device's own. Kept in the desktop app's settings file only.
   */
  rate: number | null;
}

/** Highest input channel stored (an interface with 64 inputs is plenty). */
const MAX_CHANNEL = 63;

/** Largest buffer size stored, in frames. */
const MAX_BUFFER = 65536;

/** Sample rates stored, in Hz. */
const MIN_RATE = 8000;
const MAX_RATE = 768000;

const KEY = 'klirr.settings.v1';

export function defaultSettings(): Settings {
  return {
    params: defaults(),
    host: '',
    inputDeviceId: '',
    outputDeviceId: '',
    inputChannel: 0,
    buffer: null,
    rate: null,
  };
}

/** A stored input channel, or 0 if it is not one. */
export function validChannel(v: unknown): number {
  return typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= MAX_CHANNEL ? v : 0;
}

/** A stored buffer size, or null (the driver's own) if it is not one. */
export function validBuffer(v: unknown): number | null {
  return typeof v === 'number' && Number.isInteger(v) && v > 0 && v <= MAX_BUFFER ? v : null;
}

/** A stored sample rate, or null (the device's own) if it is not one. */
export function validRate(v: unknown): number | null {
  return typeof v === 'number' && Number.isInteger(v) && v >= MIN_RATE && v <= MAX_RATE ? v : null;
}

/**
 * Parameters as stored: by name, so that reordering parameters never mixes up values.
 */
export function paramsToRecord(params: readonly number[]): Record<string, number> {
  const out: Record<string, number> = {};
  for (const sp of PARAMS) out[sp.name] = params[sp.index] ?? sp.default;
  return out;
}

/** Stored parameters; missing or invalid ones take their default. */
export function paramsFromRecord(raw: unknown): number[] {
  const out = defaults();
  if (typeof raw === 'object' && raw !== null && !Array.isArray(raw)) {
    const byName = raw as Record<string, unknown>;
    for (const sp of PARAMS) {
      const v = byName[sp.name];
      if (typeof v === 'number' && Number.isFinite(v)) out[sp.index] = sanitise(sp.index, v);
    }
  }
  // Never start bypassed: it would look like the amp is broken.
  out[P.Bypass] = 0;
  return out;
}

/** Reads stored settings; anything missing or invalid falls back to the default. */
export function loadSettings(storage: Pick<Storage, 'getItem'> | undefined): Settings {
  const out = defaultSettings();
  let raw: unknown;
  try {
    raw = JSON.parse(storage?.getItem(KEY) ?? 'null');
  } catch {
    return out;
  }
  if (typeof raw !== 'object' || raw === null) return out;
  const r = raw as Record<string, unknown>;

  out.params = paramsFromRecord(r.params);
  if (typeof r.host === 'string') out.host = r.host;
  if (typeof r.inputDeviceId === 'string') out.inputDeviceId = r.inputDeviceId;
  if (typeof r.outputDeviceId === 'string') out.outputDeviceId = r.outputDeviceId;
  out.inputChannel = validChannel(r.inputChannel);
  return out;
}

export function saveSettings(storage: Pick<Storage, 'setItem'> | undefined, s: Settings): void {
  try {
    storage?.setItem(
      KEY,
      JSON.stringify({
        params: paramsToRecord(s.params),
        host: s.host,
        inputDeviceId: s.inputDeviceId,
        outputDeviceId: s.outputDeviceId,
        inputChannel: s.inputChannel,
      }),
    );
  } catch {
    // Storage full or disabled (private mode): settings just are not remembered.
  }
}
