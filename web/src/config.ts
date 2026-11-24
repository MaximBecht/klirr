// The desktop app's settings file (`desktop/src-tauri/src/config.rs`, which reads, migrates
// and writes it): its shape, and how the UI's settings go into it and come out of it.
//
// The UI changes only the fields it knows and hands back everything else as it came, so
// settings of a newer app survive a run of this one.
//
// Device settings are kept per audio host (driver API), keyed by host id, so switching from
// ASIO to WASAPI and back brings the ASIO devices and buffer back. The buffer and the sample
// rate belong to the output device: the driver that clocks the stream.

import {
  defaultSettings,
  paramsFromRecord,
  paramsToRecord,
  validBuffer,
  validChannel,
  validRate,
  type Settings,
} from './settings';

/** What Klirr asks one output device for when it opens it. */
export interface DeviceSettings {
  /** Frames; null for the driver's own size. */
  buffer: number | null;
  /** Hz, where the rate is Klirr's to choose; null (or absent) for the device's own. */
  rate?: number | null;
  [other: string]: unknown;
}

/** The devices last chosen on one audio host. */
export interface HostSettings {
  input: string;
  output: string;
  channel: number;
  /** By output device name ('' for the default output). */
  devices: Record<string, DeviceSettings>;
  [other: string]: unknown;
}

export interface DesktopConfig {
  version: number;
  audio: {
    /** Host id; '' for the platform's default. */
    host: string;
    /** By host id (the default host by its own id, not ''). */
    hosts: Record<string, HostSettings>;
    [other: string]: unknown;
  };
  /** Amp parameters by name. */
  params: Record<string, unknown>;
  [other: string]: unknown;
}

/** How the native side found the file when the app started. */
export type ConfigStatus = 'new' | 'loaded' | 'migrated' | 'newer' | 'broken';

export interface LoadedConfig {
  config: DesktopConfig;
  status: ConfigStatus;
  /** Where the file is. */
  path: string;
  /** Where an unreadable file was moved to. */
  brokenPath: string | null;
}

export function emptyConfig(): DesktopConfig {
  return { version: 1, audio: { host: '', hosts: {} }, params: {} };
}

/** The key a host's settings are kept under: its id, also for the default host. */
export function hostKey(host: string, defaultHost: string): string {
  return host || defaultHost;
}

/** Sets the devices and buffer of `s` to what was last chosen on its host. */
export function recallHost(config: DesktopConfig, s: Settings, defaultHost: string): void {
  const entry = config.audio.hosts[hostKey(s.host, defaultHost)];
  s.inputDeviceId = typeof entry?.input === 'string' ? entry.input : '';
  s.outputDeviceId = typeof entry?.output === 'string' ? entry.output : '';
  s.inputChannel = validChannel(entry?.channel);
  recallBuffer(config, s, defaultHost);
}

/** Sets the buffer and sample rate of `s` to what was last chosen for its output device. */
export function recallBuffer(config: DesktopConfig, s: Settings, defaultHost: string): void {
  const device = config.audio.hosts[hostKey(s.host, defaultHost)]?.devices[s.outputDeviceId];
  s.buffer = validBuffer(device?.buffer);
  s.rate = validRate(device?.rate);
}

/** The UI's settings, as the file holds them. */
export function readConfig(config: DesktopConfig, defaultHost: string): Settings {
  const s = defaultSettings();
  s.params = paramsFromRecord(config.params);
  s.host = typeof config.audio.host === 'string' ? config.audio.host : '';
  recallHost(config, s, defaultHost);
  return s;
}

/**
 * `config` with the UI's settings written into it; its other fields as they were. Only the
 * current host's entry changes: the others keep what was chosen on them.
 */
export function writeConfig(
  config: DesktopConfig,
  s: Settings,
  defaultHost: string,
): DesktopConfig {
  const key = hostKey(s.host, defaultHost);
  const old = config.audio.hosts[key];
  const devices = old?.devices ?? {};
  const host: HostSettings = {
    ...old,
    input: s.inputDeviceId,
    output: s.outputDeviceId,
    channel: s.inputChannel,
    devices: {
      ...devices,
      [s.outputDeviceId]: { ...devices[s.outputDeviceId], buffer: s.buffer, rate: s.rate },
    },
  };
  return {
    ...config,
    audio: { ...config.audio, host: s.host, hosts: { ...config.audio.hosts, [key]: host } },
    // Parameters of a newer app stay too.
    params: { ...config.params, ...paramsToRecord(s.params) },
  };
}
