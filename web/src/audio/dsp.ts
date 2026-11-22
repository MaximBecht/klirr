// Thin typed wrapper around the klirr-wasm C ABI. Used inside the AudioWorklet, and directly
// in tests (Node can instantiate the same module).

interface KlirrExports {
  memory: WebAssembly.Memory;
  klirr_init(sampleRate: number): number;
  klirr_max_block(): number;
  klirr_input_ptr(): number;
  klirr_output_ptr(): number;
  klirr_process(len: number): void;
  klirr_set_param(index: number, value: number): void;
  klirr_get_param(index: number): number;
  klirr_param_count(): number;
  klirr_param_min(index: number): number;
  klirr_param_max(index: number): number;
  klirr_param_default(index: number): number;
  klirr_param_stepped(index: number): number;
  klirr_take_meters(): number;
  klirr_latency_samples(): number;
  klirr_reset(): void;
  klirr_ir_buffer(len: number): number;
  klirr_ir_load(irSampleRate: number): number;
}

export interface Meters {
  inputPeak: number;
  outputPeak: number;
  gateOpen: boolean;
  limiterDb: number;
}

/** Messages for the error codes of `klirr_ir_load`. */
export const IR_ERRORS: Record<number, string> = {
  1: 'the impulse response is empty',
  2: 'the impulse response contains invalid samples',
  3: 'the impulse response sample rate is invalid',
  4: 'the impulse response is silent',
  100: 'the engine is not running',
};

export async function instantiate(bytes: BufferSource): Promise<KlirrExports> {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  return instance.exports as unknown as KlirrExports;
}

export class KlirrDsp {
  readonly maxBlock: number;
  readonly latencySamples: number;
  private readonly x: KlirrExports;
  private buffer: ArrayBuffer | null = null;
  private input = new Float32Array(0);
  private output = new Float32Array(0);
  /** The last returned block of output, reused while the block length stays the same. */
  private outputBlock = new Float32Array(0);
  private meters = new Float32Array(0);

  constructor(exports: KlirrExports, sampleRate: number) {
    this.x = exports;
    if (exports.klirr_init(sampleRate) !== 1) {
      throw new Error(`unsupported sample rate ${sampleRate}`);
    }
    this.maxBlock = exports.klirr_max_block();
    this.latencySamples = exports.klirr_latency_samples();
    this.refreshViews();
  }

  /** Typed-array views go stale whenever wasm memory grows; rebuild them if it did. */
  private refreshViews(): void {
    const buf = this.x.memory.buffer;
    if (buf === this.buffer) return;
    this.buffer = buf;
    this.input = new Float32Array(buf, this.x.klirr_input_ptr(), this.maxBlock);
    this.output = new Float32Array(buf, this.x.klirr_output_ptr(), this.maxBlock);
    this.outputBlock = this.output;
  }

  /**
   * Processes one block. `input` may be undefined (no input connected), which is silence.
   * Returns a view of the output; it is only valid until the next call.
   *
   * Runs on the audio thread, so it creates no garbage once the block length is steady:
   * garbage means collector pauses, and a pause can make the browser miss its deadline.
   */
  process(input: Float32Array | undefined, length: number): Float32Array {
    this.refreshViews();
    const n = Math.min(length, this.maxBlock);
    if (!input) this.input.fill(0, 0, n);
    else if (input.length === n) this.input.set(input);
    else this.input.set(input.subarray(0, n));
    this.x.klirr_process(n);
    if (this.outputBlock.length !== n) this.outputBlock = this.output.subarray(0, n);
    return this.outputBlock;
  }

  setParam(index: number, value: number): void {
    this.x.klirr_set_param(index, value);
  }

  getParam(index: number): number {
    return this.x.klirr_get_param(index);
  }

  takeMeters(): Meters {
    const ptr = this.x.klirr_take_meters();
    if (this.meters.byteOffset !== ptr || this.meters.buffer !== this.x.memory.buffer)
      this.meters = new Float32Array(this.x.memory.buffer, ptr, 4);
    const m = this.meters;
    return {
      inputPeak: m[0] ?? 0,
      outputPeak: m[1] ?? 0,
      gateOpen: m[2] === 1,
      limiterDb: m[3] ?? 0,
    };
  }

  /** Loads a custom cabinet IR. Allocates: call from a message handler, not `process`. */
  loadIr(samples: Float32Array, sampleRate: number): number {
    if (samples.length === 0) return 1;
    const ptr = this.x.klirr_ir_buffer(samples.length);
    // Reserving the buffer may have grown memory, so take a fresh view.
    new Float32Array(this.x.memory.buffer, ptr, samples.length).set(samples);
    return this.x.klirr_ir_load(sampleRate);
  }

  reset(): void {
    this.x.klirr_reset();
  }
}

export type { KlirrExports };
