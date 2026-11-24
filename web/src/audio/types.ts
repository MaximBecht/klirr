// What the UI needs from an audio engine. Two implement it: `AudioEngine` (the browser:
// Web Audio and the wasm engine in an AudioWorklet) and `NativeEngine` (the desktop app:
// the native audio host, over Tauri's IPC).

import type { Meters } from './dsp';
import type { Measurement } from './probe';

export type { Meters, Measurement };

export type EngineState = 'stopped' | 'starting' | 'running';

export interface DeviceInfo {
  /** What `StartOptions` takes to open the device. */
  id: string;
  /** A name to show. */
  label: string;
  /** Channels it delivers, or null where unknown. */
  channels: number | null;
  /** Whether it is the system's default device. */
  isDefault: boolean;
}

export interface Devices {
  inputs: DeviceInfo[];
  outputs: DeviceInfo[];
}

export interface StartOptions {
  /** Audio host (desktop only); empty for the default. */
  host: string;
  /** Empty for the default device. */
  inputDeviceId: string;
  outputDeviceId: string;
  inputChannel: number;
  /** Buffer size to ask the driver for, in frames (desktop only); null for its own. */
  buffer: number | null;
  /**
   * Sample rate to ask for, in Hz (desktop only, where the rate is Klirr's to choose); null
   * for the device's own.
   */
  rate: number | null;
  params: number[];
}

/** Latency as the browser or the audio driver reports it, in ms. */
export interface LatencyEstimate {
  /** Input side, or null where it is not reported (Firefox, Safari). */
  inputMs: number | null;
  /** Output side. */
  outputMs: number;
  /** The engine itself (oversampling filters + cabinet convolution). */
  engineMs: number;
  totalMs: number;
}

export interface Dropouts {
  /** Times the audio fell behind for good since audio started. */
  count: number;
  /** Latency those dropouts added, in ms. */
  addedMs: number;
}

export interface AmpEngine {
  onMeters: (m: Meters) => void;
  onStateChange: (s: EngineState) => void;
  /** Called when a device stops while running (unplugged, permission revoked). */
  onDeviceLost: (message: string) => void;

  readonly running: boolean;
  /** Whether the output device can be chosen. */
  readonly canChooseOutput: boolean;
  /** Whether device names only appear once audio input has been allowed. */
  readonly labelsNeedPermission: boolean;
  /** Who reports the latency estimate, for the UI ("the browser"). */
  readonly estimateSource: string;

  /** Lists devices; `host` is the audio host (desktop only). */
  listDevices(host: string): Promise<Devices>;
  /** Calls `onChange` when devices may have come or gone. */
  watchDevices(onChange: () => void): void;

  /** Must be called from a user gesture (click), so the browser lets audio start. */
  start(opts: StartOptions): Promise<void>;
  stop(): Promise<void>;
  /**
   * Buffer sizes while running: the frames per cycle of input and output (the ASIO buffer
   * size, the WASAPI period). Null when not running or not known (the browser).
   */
  bufferFrames(): { input: number | null; output: number | null } | null;

  /** Queues a parameter change; changes are sent together every few milliseconds. */
  setParam(index: number, value: number): void;
  /**
   * These switch while running (the desktop app restarts the audio to do it). A new input
   * comes with the channel to use on it.
   */
  setInputDevice(deviceId: string, channel: number): Promise<void>;
  setOutputDevice(deviceId: string): Promise<void>;
  setInputChannel(channel: number): Promise<void>;
  /** Number of channels the current input delivers. */
  inputChannelCount(): number;

  /** The sample rate audio runs at; null when not running. */
  sampleRate(): number | null;
  /** Cheap enough to poll; null when not running. */
  latencyEstimate(): LatencyEstimate | null;
  /** Null when not running. */
  dropouts(): Dropouts | null;
  /** Loopback measurement: one to three seconds. */
  measureLatency(): Promise<Measurement>;
  /**
   * Fades a quiet 440 Hz test tone in, in place of the amp, or out again. Only while
   * running; it is off after every start.
   */
  setTestTone(on: boolean): void;

  /**
   * Decodes an impulse response (any format the webview decodes, normally WAV) and loads it.
   * Resolves once the engine accepted it, or when audio is not running, once it is checked
   * (it is loaded on the next start).
   */
  loadIr(name: string, bytes: ArrayBuffer): Promise<void>;

  /** A message for the user explaining an error from this engine. */
  describeError(err: unknown): string;
}
