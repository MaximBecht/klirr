// Remembers the user's settings between visits (localStorage, nothing leaves the machine).

import { PARAMS, P, defaults, sanitise } from './params';

export interface Settings {
  params: number[];
  inputDeviceId: string;
  outputDeviceId: string;
  /** 0 = input 1 (left), 1 = input 2 (right). */
  inputChannel: number;
}

const KEY = 'klirr.settings.v1';

export function defaultSettings(): Settings {
  return { params: defaults(), inputDeviceId: '', outputDeviceId: '', inputChannel: 0 };
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
  if (typeof r.inputDeviceId === 'string') out.inputDeviceId = r.inputDeviceId;
  if (typeof r.outputDeviceId === 'string') out.outputDeviceId = r.outputDeviceId;
  if (r.inputChannel === 0 || r.inputChannel === 1) out.inputChannel = r.inputChannel;
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
        inputDeviceId: s.inputDeviceId,
        outputDeviceId: s.outputDeviceId,
        inputChannel: s.inputChannel,
      }),
    );
  } catch {
    // Storage full or disabled (private mode): settings just are not remembered.
  }
}
