// The AudioWorklet processor: runs the Klirr engine (WebAssembly) on the audio thread.
//
// Worklets cannot fetch, so the main thread sends the module bytes in an `init` message.
// Until the engine is ready the processor outputs silence.

import { KlirrDsp, instantiate } from './dsp';
import { METER_HZ, type FromWorklet, type ToWorklet } from './messages';
import { PROCESSOR_NAME } from './messages';

// AudioWorkletGlobalScope globals (not part of the DOM typings).
declare const sampleRate: number;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(
  name: string,
  ctor: new (options: AudioWorkletNodeOptions) => AudioWorkletProcessor,
): void;

class KlirrProcessor extends AudioWorkletProcessor {
  private dsp: KlirrDsp | null = null;
  private inputChannel = 0;
  private framesSinceMeters = 0;
  private readonly framesPerMeter = Math.round(sampleRate / METER_HZ);

  constructor() {
    super();
    this.port.onmessage = (e: MessageEvent<ToWorklet>) => {
      void this.onMessage(e.data);
    };
  }

  private post(msg: FromWorklet): void {
    this.port.postMessage(msg);
  }

  private async onMessage(msg: ToWorklet): Promise<void> {
    switch (msg.type) {
      case 'init':
        try {
          const dsp = new KlirrDsp(await instantiate(msg.wasm), sampleRate);
          msg.params.forEach((v, i) => {
            dsp.setParam(i, v);
          });
          dsp.reset(); // start from the stored settings, not a glide from the defaults
          this.inputChannel = msg.inputChannel;
          this.dsp = dsp;
          this.post({ type: 'ready', latencySamples: dsp.latencySamples });
        } catch (err) {
          this.post({ type: 'error', message: String(err) });
        }
        break;
      case 'params':
        for (const [i, v] of msg.changes) this.dsp?.setParam(i, v);
        break;
      case 'input-channel':
        this.inputChannel = msg.channel;
        break;
      case 'ir': {
        const code = this.dsp ? this.dsp.loadIr(msg.samples, msg.sampleRate) : 100;
        this.post({ type: 'ir-result', code });
        break;
      }
    }
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const out = outputs[0];
    const first = out?.[0];
    if (!this.dsp || !out || !first) return true;

    const input = inputs[0];
    const source = input?.[this.inputChannel] ?? input?.[0];
    const result = this.dsp.process(source, first.length);
    for (const ch of out) ch.set(result);

    this.framesSinceMeters += first.length;
    if (this.framesSinceMeters >= this.framesPerMeter) {
      this.framesSinceMeters = 0;
      this.post({ type: 'meters', ...this.dsp.takeMeters() });
    }
    return true;
  }
}

registerProcessor(PROCESSOR_NAME, KlirrProcessor);
