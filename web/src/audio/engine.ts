// Main-thread side of the audio engine: devices, the AudioContext and the worklet node.

import wasmUrl from '../wasm/klirr.wasm?url';
import workletUrl from './worklet.ts?worker&url';
import type { Meters } from './dsp';
import { IR_ERRORS } from './dsp';
import { PROCESSOR_NAME, type FromWorklet, type ToWorklet } from './messages';
import { wavSampleRate } from './wav';

export interface Devices {
  inputs: MediaDeviceInfo[];
  outputs: MediaDeviceInfo[];
}

export interface StartOptions {
  inputDeviceId: string;
  outputDeviceId: string;
  inputChannel: number;
  params: number[];
}

export interface Ir {
  name: string;
  samples: Float32Array;
  sampleRate: number;
}

type State = 'stopped' | 'starting' | 'running';

/** True if the browser can choose the output device (Chrome, Edge). */
export const outputSelectionSupported =
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

export class AudioEngine {
  onMeters: (m: Meters) => void = () => undefined;
  onStateChange: (s: State) => void = () => undefined;
  /** Called when the browser stops the input (device unplugged, permission revoked). */
  onInputEnded: () => void = () => undefined;

  private state: State = 'stopped';
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

  get running(): boolean {
    return this.state === 'running';
  }

  /** Lists audio devices. Labels are empty until the user has allowed microphone access. */
  static async listDevices(): Promise<Devices> {
    const all = await navigator.mediaDevices.enumerateDevices();
    return {
      inputs: all.filter((d) => d.kind === 'audioinput'),
      outputs: all.filter((d) => d.kind === 'audiooutput'),
    };
  }

  /** Must be called from a user gesture (click), so the browser lets audio start. */
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
      this.setState('running');
    } catch (err) {
      if (cancelled()) return;
      await this.stop();
      throw err;
    }
  }

  async stop(): Promise<void> {
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

  setInputChannel(channel: number): void {
    this.post({ type: 'input-channel', channel });
  }

  /** Switches the input device while running. */
  async setInputDevice(deviceId: string): Promise<void> {
    if (!this.ctx || this.state !== 'running') return;
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

  /** Rough string-to-ear latency added by the browser and the engine, in ms. */
  latencyMs(): number | null {
    const ctx = this.ctx;
    if (!ctx) return null;
    const track = this.stream?.getAudioTracks()[0];
    const inputLatency = (track?.getSettings() as { latency?: number } | undefined)?.latency ?? 0;
    const outputLatency = ctx.baseLatency + (ctx.outputLatency || 0);
    return (inputLatency + outputLatency + this.latencySamples / ctx.sampleRate) * 1000;
  }

  get irName(): string | null {
    return this.ir?.name ?? null;
  }

  /**
   * Decodes an impulse response (any format the browser decodes, normally WAV) and loads it.
   * The file is read locally; nothing is uploaded. Resolves once the engine accepted it, or
   * right away when audio is not running (it is loaded on the next start).
   */
  async loadIr(name: string, bytes: ArrayBuffer): Promise<void> {
    const rate = wavSampleRate(bytes) ?? this.ctx?.sampleRate ?? 48000;
    // Decode at the file's own rate; the engine resamples once, to the context rate.
    const decoder = new OfflineAudioContext(1, 1, Math.min(384000, Math.max(8000, rate)));
    const buffer = await decoder.decodeAudioData(bytes);
    const ir: Ir = {
      name,
      samples: buffer.getChannelData(0).slice(),
      sampleRate: buffer.sampleRate,
    };
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
        this.onInputEnded();
      };
    }
  }

  private onWorkletMessage(msg: FromWorklet): void {
    if (msg.type === 'meters') this.onMeters(msg);
    else if (msg.type === 'ir-result') this.irWaiters.shift()?.(msg.code);
  }

  private post(msg: ToWorklet, transfer: Transferable[] = []): void {
    this.node?.port.postMessage(msg, transfer);
  }

  private setState(s: State): void {
    this.state = s;
    this.onStateChange(s);
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

/** A message for the user explaining why audio could not start. */
export function describeStartError(err: unknown): string {
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
