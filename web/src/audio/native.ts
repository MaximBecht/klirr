// The desktop app's audio engine: the native audio host (Rust, `desktop/src-tauri`), driven
// over Tauri's IPC. The webview only shows the UI; audio never passes through it.

import { Channel, invoke } from '@tauri-apps/api/core';
import type { DesktopConfig, LoadedConfig } from '../config';
import { decodeIr } from './ir';
import type {
  AmpEngine,
  Devices,
  Dropouts,
  EngineState,
  LatencyEstimate,
  Measurement,
  Meters,
  StartOptions,
} from './types';

/** An audio host (driver API) of the native side. */
export interface HostInfo {
  id: string;
  name: string;
  isDefault: boolean;
  /**
   * What the Hardware Setup button opens ("the ASIO driver's control panel"); null where
   * there is nothing to open.
   */
  hardwareSetup: string | null;
}

/**
 * The buffer sizes and sample rates a host's devices allow (see
 * `klirr_native::devices::BufferChoice`).
 */
export interface BufferChoice {
  /** The rate the devices open at; the sizes count frames at this rate. */
  sampleRate: number;
  /** Rates Klirr can ask for, lowest first; empty when the rate is not Klirr's to choose. */
  rates: number[];
  /** The size without a request, in frames, where known. */
  default: number | null;
  /** Sizes Klirr can ask for, smallest first; empty when the size is not Klirr's to choose. */
  sizes: number[];
  /** Who sets the size when Klirr cannot ("the Windows audio engine"). */
  setBy: string | null;
}

/** The buffer audio runs with. */
export interface RunningBuffer {
  /** Frames, as the driver reports them (null where it does not). */
  frames: number | null;
  sampleRate: number;
  /** What was asked for; null for the driver's own. */
  requested: number | null;
  /** Why the requested size or rate was not used (the driver's own was), if it was not. */
  refused: string | null;
  /** The sample rate asked for; null for the device's own. */
  requestedRate: number | null;
}

interface NativeDevice {
  name: string;
  channels: number;
  isDefault: boolean;
}

/** What the native side opened. */
interface Started {
  host: string;
  input: string;
  output: string;
  sampleRate: number;
  inputChannels: number;
  inputBuffer: number | null;
  outputBuffer: number | null;
  requestedBuffer: number | null;
  bufferRefused: string | null;
  requestedRate: number | null;
  engineLatency: number;
}

interface Status {
  inputMs: number;
  outputMs: number;
  engineMs: number;
  dropouts: number;
  addedMs: number;
}

/** Native side → webview, while audio runs. */
type NativeEvent =
  | ({ type: 'meters' } & Meters)
  | ({ type: 'status' } & Status)
  | { type: 'device-error' }
  | { type: 'restarted'; started: Started }
  | { type: 'restart-failed'; message: string };

const PARAM_FLUSH_MS = 15;

/** Error messages from the native side start in lower case, to be embedded in sentences. */
function sentence(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export class NativeEngine implements AmpEngine {
  onMeters: (m: Meters) => void = () => undefined;
  onStateChange: (s: EngineState) => void = () => undefined;
  onDeviceLost: (message: string) => void = () => undefined;
  /**
   * The driver asked for a restart (its buffer size or sample rate was changed in its own
   * settings) and audio runs again, perhaps with another buffer or rate.
   */
  onRestarted: () => void = () => undefined;

  readonly canChooseOutput = true;
  readonly labelsNeedPermission = false;
  readonly estimateSource = 'the audio driver';

  private state: EngineState = 'stopped';
  /** What audio runs (or last ran) with; parameters are kept current. */
  private opts: StartOptions | null = null;
  private started: Started | null = null;
  private status: Status | null = null;
  /** Counts starts and stops, so a late reply or event of an earlier run is ignored. */
  private generation = 0;
  private pending = new Map<number, number>();
  private flushTimer: ReturnType<typeof setTimeout> | null = null;

  get running(): boolean {
    return this.state === 'running';
  }

  /** The audio hosts (driver APIs) the native side can open, the default first. */
  static hosts(): Promise<HostInfo[]> {
    return invoke<HostInfo[]>('hosts');
  }

  /**
   * The buffer sizes and sample rates `host` allows on these devices ('' for the defaults),
   * the sizes counted at `rate` where Klirr chooses the rate.
   */
  static bufferChoice(
    host: string,
    input: string,
    output: string,
    rate: number | null,
  ): Promise<BufferChoice> {
    return invoke<BufferChoice>('buffer_sizes', { host, input, output, rate });
  }

  /**
   * Opens the hardware setup of `host` for `output` ('' for the defaults): the ASIO driver's
   * control panel, or the system's audio settings. With some ASIO drivers it resolves only
   * once the panel is closed.
   */
  static async hardwareSetup(host: string, output: string): Promise<void> {
    try {
      await invoke('hardware_setup', { host, output });
    } catch (err) {
      throw new Error(sentence(String(err)), { cause: err });
    }
  }

  /** The settings file, as the app found it at start (with what was saved since). */
  static loadConfig(): Promise<LoadedConfig> {
    return invoke<LoadedConfig>('load_config');
  }

  static async saveConfig(config: DesktopConfig): Promise<void> {
    try {
      await invoke('save_config', { config });
    } catch (err) {
      throw new Error(sentence(String(err)), { cause: err });
    }
  }

  async listDevices(host: string): Promise<Devices> {
    const lists = await invoke<{ inputs: NativeDevice[]; outputs: NativeDevice[] }>('devices', {
      host,
    });
    const map = (d: NativeDevice) => ({
      id: d.name,
      label: d.name,
      channels: d.channels > 0 ? d.channels : null,
      isDefault: d.isDefault,
    });
    return { inputs: lists.inputs.map(map), outputs: lists.outputs.map(map) };
  }

  /**
   * The native side has no device events to listen to: look again when the window is used.
   * Not while audio runs: listing loads drivers (ASIO), which can disturb the running stream.
   */
  watchDevices(onChange: () => void): void {
    window.addEventListener('focus', () => {
      if (this.state === 'stopped') onChange();
    });
  }

  async start(opts: StartOptions): Promise<void> {
    if (this.state !== 'stopped') return;
    this.opts = { ...opts, params: [...opts.params] };
    await this.open();
  }

  async stop(): Promise<void> {
    this.generation++;
    if (this.flushTimer !== null) clearTimeout(this.flushTimer);
    this.flushTimer = null;
    this.pending.clear();
    this.started = this.status = null;
    this.setState('stopped');
    await invoke('stop');
  }

  /** Opens the devices with the current options; whatever ran before stops first. */
  private async open(): Promise<void> {
    const opts = this.opts;
    if (!opts) return;
    const generation = ++this.generation;
    this.started = this.status = null;
    this.setState('starting');
    const events = new Channel<NativeEvent>();
    events.onmessage = (e) => {
      if (generation === this.generation) this.onEvent(e);
    };
    let started: Started;
    try {
      started = await invoke<Started>('start', {
        request: {
          host: opts.host,
          input: opts.inputDeviceId,
          output: opts.outputDeviceId,
          channel: opts.inputChannel,
          buffer: opts.buffer,
          rate: opts.rate,
          params: opts.params,
        },
        events,
      });
    } catch (err) {
      if (generation === this.generation) this.setState('stopped');
      throw err;
    }
    // Stopped (or started again) while the devices opened.
    if (generation !== this.generation) return;
    this.started = started;
    this.setState('running');
  }

  /**
   * Changes the host, devices, channel or buffer; restarts the audio if it runs. Takes them together
   * (a new host comes with new devices), so the audio restarts once.
   */
  async reconfigure(change: Partial<Omit<StartOptions, 'params'>>): Promise<void> {
    if (!this.opts) return;
    this.opts = { ...this.opts, ...change };
    if (this.state !== 'stopped') await this.open();
  }

  /** The buffer audio runs with; null while it does not run. */
  runningBuffer(): RunningBuffer | null {
    const s = this.started;
    if (!s) return null;
    return {
      frames: s.outputBuffer ?? s.inputBuffer,
      sampleRate: s.sampleRate,
      requested: s.requestedBuffer,
      refused: s.bufferRefused,
      requestedRate: s.requestedRate,
    };
  }

  setInputDevice(deviceId: string, channel: number): Promise<void> {
    return this.reconfigure({ inputDeviceId: deviceId, inputChannel: channel });
  }

  setOutputDevice(deviceId: string): Promise<void> {
    return this.reconfigure({ outputDeviceId: deviceId });
  }

  setInputChannel(channel: number): Promise<void> {
    return this.reconfigure({ inputChannel: channel });
  }

  bufferFrames(): { input: number | null; output: number | null } | null {
    const s = this.started;
    if (!s || this.state !== 'running') return null;
    return { input: s.inputBuffer, output: s.outputBuffer };
  }

  setParam(index: number, value: number): void {
    if (this.opts) this.opts.params[index] = value;
    if (this.state === 'stopped') return;
    this.pending.set(index, value);
    this.flushTimer ??= setTimeout(() => {
      this.flushTimer = null;
      if (this.pending.size === 0) return;
      const changes = [...this.pending];
      this.pending.clear();
      void invoke('set_params', { changes });
    }, PARAM_FLUSH_MS);
  }

  sampleRate(): number | null {
    return this.state === 'running' ? (this.started?.sampleRate ?? null) : null;
  }

  inputChannelCount(): number {
    return this.started?.inputChannels ?? 2;
  }

  latencyEstimate(): LatencyEstimate | null {
    const s = this.status;
    if (!s || this.state !== 'running') return null;
    return {
      inputMs: s.inputMs,
      outputMs: s.outputMs,
      engineMs: s.engineMs,
      totalMs: s.inputMs + s.outputMs + s.engineMs,
    };
  }

  dropouts(): Dropouts | null {
    if (this.state !== 'running') return null;
    return { count: this.status?.dropouts ?? 0, addedMs: this.status?.addedMs ?? 0 };
  }

  setTestTone(on: boolean): void {
    if (this.state !== 'running') return;
    void invoke('test_tone', { on });
  }

  measureLatency(): Promise<Measurement> {
    if (this.state !== 'running') return Promise.resolve({ ok: false, reason: 'stopped' });
    return invoke<Measurement>('measure');
  }

  async loadIr(name: string, bytes: ArrayBuffer): Promise<void> {
    const ir = await decodeIr(name, bytes, this.started?.sampleRate ?? 48000);
    // The samples go as raw bytes, not as a JSON array of numbers.
    const raw = new Uint8Array(ir.samples.buffer);
    try {
      await invoke('load_ir', raw, { headers: { 'x-sample-rate': String(ir.sampleRate) } });
    } catch (err) {
      // Native errors arrive as plain strings; the UI shows an Error's message.
      throw new Error(String(err), { cause: err });
    }
  }

  describeError(err: unknown): string {
    const text = err instanceof Error ? err.message : String(err);
    return sentence(text);
  }

  private onEvent(e: NativeEvent): void {
    switch (e.type) {
      case 'meters':
        this.onMeters(e);
        break;
      case 'status':
        this.status = e;
        break;
      case 'device-error':
        this.onDeviceLost(
          'The audio device reported an error (unplugged?). Choose a device and engage again.',
        );
        break;
      case 'restarted':
        this.started = e.started;
        this.status = null;
        this.onRestarted();
        break;
      case 'restart-failed':
        this.generation++;
        this.started = this.status = null;
        this.setState('stopped');
        this.onDeviceLost(
          `The driver asked for a restart, and the devices would not open again: ${e.message}`,
        );
        break;
    }
  }

  private setState(s: EngineState): void {
    this.state = s;
    this.onStateChange(s);
  }
}
