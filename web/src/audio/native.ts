// The desktop app's audio engine: the native audio host (Rust, `desktop/src-tauri`), driven
// over Tauri's IPC. The webview only shows the UI; audio never passes through it.

import { Channel, invoke } from '@tauri-apps/api/core';
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
  ({ type: 'meters' } & Meters) | ({ type: 'status' } & Status) | { type: 'device-error' };

const PARAM_FLUSH_MS = 15;

/** Error messages from the native side start in lower case, to be embedded in sentences. */
function sentence(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export class NativeEngine implements AmpEngine {
  onMeters: (m: Meters) => void = () => undefined;
  onStateChange: (s: EngineState) => void = () => undefined;
  onDeviceLost: (message: string) => void = () => undefined;

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
   * Changes the host, devices or channel; restarts the audio if it runs. Takes them together
   * (a new host comes with new devices), so the audio restarts once.
   */
  async reconfigure(change: Partial<Omit<StartOptions, 'params'>>): Promise<void> {
    if (!this.opts) return;
    this.opts = { ...this.opts, ...change };
    if (this.state !== 'stopped') await this.open();
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
    }
  }

  private setState(s: EngineState): void {
    this.state = s;
    this.onStateChange(s);
  }
}
