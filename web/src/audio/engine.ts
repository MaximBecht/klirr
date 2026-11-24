// The browser's audio engine: devices, the AudioContext and the worklet node, which runs the
// wasm engine on the audio thread.

import wasmUrl from '../wasm/klirr.wasm?url';
import workletUrl from './worklet.ts?worker&url';
import { IR_ERRORS } from './dsp';
import { PROCESSOR_NAME, type FromWorklet, type ToWorklet } from './messages';
import { DropoutWatch } from './dropouts';
import { decodeIr, type Ir } from './ir';
import { summarise } from './probe';
import type {
  AmpEngine,
  DeviceInfo,
  Devices,
  Dropouts,
  EngineState,
  LatencyEstimate,
  Measurement,
  Meters,
  StartOptions,
} from './types';

/** True if the browser can choose the output device (Chrome, Edge). */
const outputSelectionSupported =
  typeof AudioContext !== 'undefined' && 'setSinkId' in AudioContext.prototype;

const PARAM_FLUSH_MS = 15;

let wasmBytes: Promise<ArrayBuffer> | null = null;
function loadWasm(): Promise<ArrayBuffer> {
  wasmBytes ??= fetch(wasmUrl).then((r) => {
    if (!r.ok) throw new Error(`could not load the engine (${r.status})`);
    return r.arrayBuffer();
  });
  return wasmBytes;
}

/** Turns the browser's device list into the UI's, without the system's alias entries. */
function toDevices(devices: MediaDeviceInfo[], fallback: string): DeviceInfo[] {
  return devices
    .filter((d) => d.deviceId && d.deviceId !== 'default' && d.deviceId !== 'communications')
    .map((d, i) => ({
      id: d.deviceId,
      label: d.label || `${fallback} ${String(i + 1)}`,
      channels: null,
      isDefault: false,
    }));
}

export class AudioEngine implements AmpEngine {
  onMeters: (m: Meters) => void = () => undefined;
  onStateChange: (s: EngineState) => void = () => undefined;
  onDeviceLost: (message: string) => void = () => undefined;

  readonly canChooseOutput = outputSelectionSupported;
  readonly labelsNeedPermission = true;
  readonly estimateSource = 'the browser';

  private state: EngineState = 'stopped';
  private ctx: AudioContext | null = null;
  private node: AudioWorkletNode | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private stream: MediaStream | null = null;
  private latencySamples = 0;
  private pending = new Map<number, number>();
  private flushTimer: ReturnType<typeof setTimeout> | null = null;
  private ir: Ir | null = null;
  private irWaiters: ((code: number) => void)[] = [];
  private abortStart: (() => void) | null = null;
  private measureWaiter: ((m: Measurement) => void) | null = null;
  private readonly dropoutWatch = new DropoutWatch();
  private dropoutTimer: ReturnType<typeof setInterval> | null = null;

  get running(): boolean {
    return this.state === 'running';
  }

  /** Lists audio devices. Labels are empty until the user has allowed microphone access. */
  async listDevices(): Promise<Devices> {
    const all = await navigator.mediaDevices.enumerateDevices();
    return {
      inputs: toDevices(
        all.filter((d) => d.kind === 'audioinput'),
        'Input',
      ),
      outputs: toDevices(
        all.filter((d) => d.kind === 'audiooutput'),
        'Output',
      ),
    };
  }

  watchDevices(onChange: () => void): void {
    navigator.mediaDevices.addEventListener('devicechange', onChange);
  }

  /** The browser does not say what buffers the device runs with. */
  bufferFrames(): null {
    return null;
  }

  async start(opts: StartOptions): Promise<void> {
    if (this.state !== 'stopped') return;
    this.setState('starting');
    // Create the context synchronously inside the gesture; awaiting first can leave it
    // suspended in some browsers.
    const ctx = new AudioContext({ latencyHint: 'interactive' });
    this.ctx = ctx;
    const cancelled = () => this.ctx !== ctx;
    try {
      const setup = Promise.all([loadWasm(), ctx.audioWorklet.addModule(workletUrl)]);
      setup.catch(() => undefined); // awaited below; avoid an unhandled rejection meanwhile
      const stream = await openInput(opts.inputDeviceId);
      if (cancelled()) {
        stream.getTracks().forEach((t) => {
          t.stop();
        });
        return;
      }
      this.stream = stream;
      const [wasm] = await setup;
      if (cancelled()) return;

      const node = new AudioWorkletNode(ctx, PROCESSOR_NAME, {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [2],
        // Keep both input channels apart, so "input 2" really is the right channel.
        channelCount: 2,
        channelCountMode: 'explicit',
        channelInterpretation: 'discrete',
      });
      this.node = node;
      const ready = new Promise<void>((resolve, reject) => {
        this.abortStart = () => {
          reject(new Error('stopped'));
        };
        node.port.onmessage = (e: MessageEvent<FromWorklet>) => {
          const msg = e.data;
          if (msg.type === 'ready') {
            this.latencySamples = msg.latencySamples;
            resolve();
          } else if (msg.type === 'error') {
            reject(new Error(msg.message));
          } else {
            this.onWorkletMessage(msg);
          }
        };
      });
      const copy = wasm.slice(0);
      this.post(
        { type: 'init', wasm: copy, params: opts.params, inputChannel: opts.inputChannel },
        [copy],
      );
      await ready;
      this.abortStart = null;
      if (cancelled()) return;

      this.connectInput(stream);
      node.connect(ctx.destination);
      if (opts.outputDeviceId) await this.setOutputDevice(opts.outputDeviceId);
      await ctx.resume();
      if (this.ir) void this.sendIr(this.ir);
      this.watchDropouts(ctx);
      this.setState('running');
    } catch (err) {
      if (cancelled()) return;
      await this.stop();
      throw err;
    }
  }

  async stop(): Promise<void> {
    if (this.dropoutTimer !== null) clearInterval(this.dropoutTimer);
    this.dropoutTimer = null;
    if (this.flushTimer !== null) clearTimeout(this.flushTimer);
    this.flushTimer = null;
    this.pending.clear();
    this.source?.disconnect();
    this.node?.disconnect();
    if (this.node) this.node.port.onmessage = null;
    this.stream?.getTracks().forEach((t) => {
      t.onended = null;
      t.stop();
    });
    const ctx = this.ctx;
    this.ctx = this.node = this.source = this.stream = null;
    for (const w of this.irWaiters.splice(0)) w(100);
    this.measureWaiter?.({ ok: false, reason: 'stopped' });
    this.measureWaiter = null;
    this.abortStart?.();
    this.abortStart = null;
    this.setState('stopped');
    if (ctx && ctx.state !== 'closed') await ctx.close();
  }

  /** Queues a parameter change; changes are sent together every few milliseconds. */
  setParam(index: number, value: number): void {
    if (!this.node) return;
    this.pending.set(index, value);
    this.flushTimer ??= setTimeout(() => {
      this.flushTimer = null;
      if (this.pending.size === 0) return;
      this.post({ type: 'params', changes: [...this.pending] });
      this.pending.clear();
    }, PARAM_FLUSH_MS);
  }

  setInputChannel(channel: number): Promise<void> {
    this.post({ type: 'input-channel', channel });
    return Promise.resolve();
  }

  /** Switches the input device while running. */
  async setInputDevice(deviceId: string, channel: number): Promise<void> {
    if (!this.ctx || this.state !== 'running') return;
    void this.setInputChannel(channel);
    const stream = await openInput(deviceId);
    this.stream?.getTracks().forEach((t) => {
      t.onended = null;
      t.stop();
    });
    this.stream = stream;
    this.connectInput(stream);
  }

  async setOutputDevice(deviceId: string): Promise<void> {
    if (!this.ctx || !outputSelectionSupported) return;
    // `setSinkId` is not in every TypeScript DOM lib version yet.
    const ctx = this.ctx as AudioContext & { setSinkId(id: string): Promise<void> };
    await ctx.setSinkId(deviceId);
  }

  /** Number of channels the current input delivers (1 for a mono device). */
  inputChannelCount(): number {
    const track = this.stream?.getAudioTracks()[0];
    return track?.getSettings().channelCount ?? 2;
  }

  /**
   * The latency the browser reports. Cheap (it reads a few properties), so it can be polled;
   * the output figure can change while running. It misses whatever the browser does not
   * know about: converters, driver buffers and, in some browsers, the whole input side.
   */
  latencyEstimate(): LatencyEstimate | null {
    const ctx = this.ctx;
    if (!ctx || this.state !== 'running') return null;
    const track = this.stream?.getAudioTracks()[0];
    const input = (track?.getSettings() as { latency?: unknown } | undefined)?.latency;
    const inputMs = typeof input === 'number' && Number.isFinite(input) ? input * 1000 : null;
    const outputMs = (ctx.baseLatency + (ctx.outputLatency || 0)) * 1000;
    const engineMs = (this.latencySamples / ctx.sampleRate) * 1000;
    return { inputMs, outputMs, engineMs, totalMs: (inputMs ?? 0) + outputMs + engineMs };
  }

  sampleRate(): number | null {
    return this.state === 'running' && this.ctx ? this.ctx.sampleRate : null;
  }

  /** Times the audio fell behind for good since audio started, and the latency that added. */
  dropouts(): Dropouts | null {
    if (this.state !== 'running') return null;
    return { count: this.dropoutWatch.count, addedMs: this.dropoutWatch.addedMs };
  }

  /** Reads the audio clock ten times a second (two numbers each time: no cost to the audio). */
  private watchDropouts(ctx: AudioContext): void {
    this.dropoutWatch.reset();
    if (this.dropoutTimer !== null) clearInterval(this.dropoutTimer);
    this.dropoutTimer = setInterval(() => {
      this.dropoutWatch.feed(ctx.currentTime, performance.now());
    }, 100);
  }

  /**
   * Measures the real latency over a loopback cable (output → selected input): plays a few
   * clicks in place of the amp and times their return. Takes one to three seconds.
   */
  measureLatency(): Promise<Measurement> {
    if (!this.ctx || !this.node || this.state !== 'running')
      return Promise.resolve({ ok: false, reason: 'stopped' });
    if (this.measureWaiter) return Promise.reject(new Error('a measurement is already running'));
    const result = new Promise<Measurement>((resolve) => {
      this.measureWaiter = resolve;
    });
    this.post({ type: 'measure-latency' });
    return result;
  }

  async loadIr(name: string, bytes: ArrayBuffer): Promise<void> {
    const ir = await decodeIr(name, bytes, this.ctx?.sampleRate ?? 48000);
    if (!this.node) {
      this.ir = ir;
      return;
    }
    const code = await this.sendIr(ir);
    if (code !== 0) throw new Error(IR_ERRORS[code] ?? `error ${code}`);
    this.ir = ir;
  }

  private sendIr(ir: Ir): Promise<number> {
    const samples = ir.samples.slice();
    const result = new Promise<number>((resolve) => this.irWaiters.push(resolve));
    this.post({ type: 'ir', samples, sampleRate: ir.sampleRate }, [samples.buffer]);
    return result;
  }

  private connectInput(stream: MediaStream): void {
    if (!this.ctx || !this.node) return;
    this.source?.disconnect();
    this.source = this.ctx.createMediaStreamSource(stream);
    this.source.connect(this.node);
    for (const t of stream.getAudioTracks()) {
      t.onended = () => {
        this.onDeviceLost(
          'The input device stopped (unplugged?). Choose another input or engage again.',
        );
      };
    }
  }

  private onWorkletMessage(msg: FromWorklet): void {
    if (msg.type === 'meters') this.onMeters(msg);
    else if (msg.type === 'ir-result') this.irWaiters.shift()?.(msg.code);
    else if (msg.type === 'latency-result' && this.ctx) {
      this.measureWaiter?.(summarise(msg.result, this.ctx.sampleRate, this.latencySamples));
      this.measureWaiter = null;
    }
  }

  private post(msg: ToWorklet, transfer: Transferable[] = []): void {
    this.node?.port.postMessage(msg, transfer);
  }

  private setState(s: EngineState): void {
    this.state = s;
    this.onStateChange(s);
  }

  describeError(err: unknown): string {
    if (err instanceof DOMException) {
      switch (err.name) {
        case 'NotAllowedError':
          return 'Microphone access was blocked. Allow it in the address bar, then try again.';
        case 'NotFoundError':
          return 'No audio input found. Connect your interface or enable line-in.';
        case 'NotReadableError':
          return 'The input is in use by another program, or the driver refused to open it.';
      }
    }
    return err instanceof Error ? err.message : String(err);
  }
}

/** Opens an input with all voice processing off: a guitar is not a phone call. */
async function openInput(deviceId: string): Promise<MediaStream> {
  const constraints: MediaTrackConstraints = {
    echoCancellation: false,
    noiseSuppression: false,
    autoGainControl: false,
    channelCount: { ideal: 2 },
  };
  if (deviceId) constraints.deviceId = { exact: deviceId };
  try {
    return await navigator.mediaDevices.getUserMedia({ audio: constraints, video: false });
  } catch (err) {
    if (deviceId && err instanceof DOMException && err.name === 'OverconstrainedError') {
      // The remembered device is gone; fall back to the default input.
      return openInput('');
    }
    throw err;
  }
}
