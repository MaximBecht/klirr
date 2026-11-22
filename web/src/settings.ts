// Remembers the user's settings between visits (localStorage, nothing leaves the machine).

import { PARAMS, P, defaults, sanitise } from './params';

export interface Settings {
  params: number[];
  /** Audio host (driver API) of the desktop app; empty for the platform's default. */
  host: string;
  inputDeviceId: string;
  outputDeviceId: string;
  /** Input channel, from 0 (0 = input 1, the left channel of a stereo input). */
  inputChannel: number;
}

/** Highest input channel stored (an interface with 64 inputs is plenty). */
const MAX_CHANNEL = 63;

const KEY = 'klirr.settings.v1';

export function defaultSettings(): Settings {
  return { params: defaults(), host: '', inputDeviceId: '', outputDeviceId: '', inputChannel: 0 };
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

  if (typeof r.params === 'object' && r.params !== null && !Array.isArray(r.params)) {
    // Stored by name, so that reordering parameters never mixes up values.
    const byName = r.params as Record<string, unknown>;
    for (const sp of PARAMS) {
      const v = byName[sp.name];
      if (typeof v === 'number' && Number.isFinite(v)) out.params[sp.index] = sanitise(sp.index, v);
    }
  }
  // Never start bypassed: it would look like the amp is broken.
  out.params[P.Bypass] = 0;
  if (typeof r.host === 'string') out.host = r.host;
  if (typeof r.inputDeviceId === 'string') out.inputDeviceId = r.inputDeviceId;
  if (typeof r.outputDeviceId === 'string') out.outputDeviceId = r.outputDeviceId;
  if (
    typeof r.inputChannel === 'number' &&
    Number.isInteger(r.inputChannel) &&
    r.inputChannel >= 0 &&
    r.inputChannel <= MAX_CHANNEL
  )
    out.inputChannel = r.inputChannel;
  return out;
}

export function saveSettings(storage: Pick<Storage, 'setItem'> | undefined, s: Settings): void {
  const params: Record<string, number> = {};
  for (const sp of PARAMS) params[sp.name] = s.params[sp.index] ?? sp.default;
  try {
    storage?.setItem(
      KEY,
      JSON.stringify({
        params,
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
