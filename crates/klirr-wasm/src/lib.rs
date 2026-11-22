//! # klirr-wasm
//!
//! The WebAssembly bridge of **Klirr**: a small C ABI around [`klirr_dsp::Engine`], loaded by
//! the web app's `AudioWorklet`. No wasm-bindgen glue: the module has no imports, and the
//! worklet talks to it through the exported functions below and the module's linear memory.
//!
//! Each WebAssembly instance holds exactly one engine (one instance per worklet), so the state
//! lives in a module-level slot instead of behind a pointer handed to JavaScript.
//!
//! ## Protocol
//!
//! 1. `klirr_init(sample_rate)` creates the engine (allocates).
//! 2. `klirr_input_ptr()` / `klirr_output_ptr()` give the addresses of two `f32` buffers of
//!    `klirr_max_block()` samples. They never move, but the memory's `ArrayBuffer` is replaced
//!    whenever memory grows, so JavaScript must refresh its typed-array views when
//!    `memory.buffer` changes.
//! 3. Per audio block: write the input samples, call `klirr_process(len)`, read the output.
//! 4. `klirr_set_param(index, value)` with the indices of [`klirr_dsp::Param`].
//! 5. `klirr_take_meters()` returns the address of four `f32`s: input peak, output peak,
//!    gate open (0/1), limiter gain reduction in dB.
//! 6. Custom cabinet IR: `klirr_ir_buffer(len)` returns the address to write `len` samples
//!    to, then `klirr_ir_load(ir_sample_rate)` loads them (allocates; call it from the
//!    worklet's message handler, never inside `process`).
//!
//! Every function is a no-op (or returns 0 / null / NaN) before `klirr_init`.

use std::cell::RefCell;

use klirr_dsp::{Engine, IrError, Param};

/// Largest block `klirr_process` accepts. Web Audio uses 128-sample render quanta today;
/// the headroom covers configurable render sizes.
pub const MAX_BLOCK: usize = 2048;

/// Number of values written by [`klirr_take_meters`].
pub const METER_COUNT: usize = 4;

struct State {
    engine: Engine,
    input: Vec<f32>,
    output: Vec<f32>,
    meters: [f32; METER_COUNT],
    ir: Vec<f32>,
}

thread_local! {
    // wasm32-unknown-unknown is single-threaded, so this is a plain static there.
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(default: R, f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| s.borrow_mut().as_mut().map_or(default, f))
}

/// Creates (or re-creates) the engine. Returns 1 on success, 0 for an unsupported sample rate.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_init(sample_rate: f32) -> u32 {
    if !(sample_rate.is_finite() && sample_rate >= 8000.0) {
        return 0;
    }
    let state = State {
        engine: Engine::new(sample_rate),
        input: vec![0.0; MAX_BLOCK],
        output: vec![0.0; MAX_BLOCK],
        meters: [0.0; METER_COUNT],
        ir: Vec::new(),
    };
    STATE.with(|s| *s.borrow_mut() = Some(state));
    1
}

/// Largest block size accepted by [`klirr_process`].
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_max_block() -> u32 {
    MAX_BLOCK as u32
}

/// Address of the input buffer ([`MAX_BLOCK`] samples), or null before init.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_input_ptr() -> *mut f32 {
    with_state(std::ptr::null_mut(), |s| s.input.as_mut_ptr())
}

/// Address of the output buffer ([`MAX_BLOCK`] samples), or null before init.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_output_ptr() -> *mut f32 {
    with_state(std::ptr::null_mut(), |s| s.output.as_mut_ptr())
}

/// Processes the first `len` samples of the input buffer into the output buffer.
/// `len` is capped at [`MAX_BLOCK`]. Real-time safe.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_process(len: u32) {
    with_state((), |s| {
        let n = (len as usize).min(MAX_BLOCK);
        s.engine.process(&s.input[..n], &mut s.output[..n]);
    });
}

/// Sets parameter `index` (see [`Param`]). Unknown indices and NaN are ignored.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_set_param(index: u32, value: f32) {
    if let Some(p) = Param::from_index(index) {
        with_state((), |s| s.engine.set_param(p, value));
    }
}

/// Current (target) value of parameter `index`, or NaN.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_get_param(index: u32) -> f32 {
    match Param::from_index(index) {
        Some(p) => with_state(f32::NAN, |s| s.engine.param(p)),
        None => f32::NAN,
    }
}

/// Number of parameters. Works before init.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_param_count() -> u32 {
    Param::COUNT as u32
}

/// Smallest value of parameter `index`, or NaN. Works before init.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_param_min(index: u32) -> f32 {
    Param::from_index(index).map_or(f32::NAN, |p| p.info().min)
}

/// Largest value of parameter `index`, or NaN. Works before init.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_param_max(index: u32) -> f32 {
    Param::from_index(index).map_or(f32::NAN, |p| p.info().max)
}

/// Default value of parameter `index`, or NaN. Works before init.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_param_default(index: u32) -> f32 {
    Param::from_index(index).map_or(f32::NAN, |p| p.info().default)
}

/// 1 if parameter `index` only takes whole numbers, 0 otherwise. Works before init.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_param_stepped(index: u32) -> u32 {
    Param::from_index(index).is_some_and(|p| p.info().stepped) as u32
}

/// Reads the meters accumulated since the previous call into a buffer of [`METER_COUNT`]
/// floats and returns its address (null before init).
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_take_meters() -> *const f32 {
    with_state(std::ptr::null(), |s| {
        let m = s.engine.take_meters();
        s.meters = [
            m.input_peak,
            m.output_peak,
            if m.gate_open { 1.0 } else { 0.0 },
            m.limiter_reduction_db,
        ];
        s.meters.as_ptr()
    })
}

/// Processing latency of the engine in samples (0 before init).
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_latency_samples() -> u32 {
    with_state(0, |s| s.engine.latency_samples() as u32)
}

/// Clears all audio state (tails, filters).
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_reset() {
    with_state((), |s| s.engine.reset());
}

/// Makes room for a custom IR of `len` samples and returns the address to write them to
/// (null before init or for `len` = 0). **Allocates.**
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_ir_buffer(len: u32) -> *mut f32 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    with_state(std::ptr::null_mut(), |s| {
        s.ir.clear();
        s.ir.resize(len as usize, 0.0);
        s.ir.as_mut_ptr()
    })
}

/// Loads the samples written to [`klirr_ir_buffer`] as the custom cabinet IR, then frees the
/// buffer. Returns 0 on success; otherwise an [`ir_error_code`]. **Allocates.**
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn klirr_ir_load(ir_sample_rate: f32) -> u32 {
    with_state(ERR_NOT_INITIALISED, |s| {
        let ir = std::mem::take(&mut s.ir);
        match s.engine.load_custom_ir(&ir, ir_sample_rate) {
            Ok(()) => 0,
            Err(e) => ir_error_code(e),
        }
    })
}

/// Returned by [`klirr_ir_load`] before [`klirr_init`].
pub const ERR_NOT_INITIALISED: u32 = 100;

/// Stable numeric codes for [`IrError`], as returned by [`klirr_ir_load`].
pub fn ir_error_code(e: IrError) -> u32 {
    match e {
        IrError::Empty => 1,
        IrError::NotFinite => 2,
        IrError::InvalidSampleRate => 3,
        IrError::Silent => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test runs on its own thread, so each gets a fresh engine slot.

    #[allow(unsafe_code)]
    fn write(ptr: *mut f32, data: &[f32]) {
        // SAFETY: test-only stand-in for JavaScript writing into linear memory; `ptr` points to
        // a live buffer of at least `data.len()` samples and no Rust reference to it is held.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len()) }
    }

    #[allow(unsafe_code)]
    fn read(ptr: *const f32, len: usize) -> Vec<f32> {
        // SAFETY: as above, for reading.
        unsafe { std::slice::from_raw_parts(ptr, len).to_vec() }
    }

    #[test]
    fn everything_is_inert_before_init() {
        assert!(klirr_input_ptr().is_null());
        assert!(klirr_take_meters().is_null());
        assert!(klirr_get_param(0).is_nan());
        assert_eq!(klirr_ir_load(48_000.0), ERR_NOT_INITIALISED);
        klirr_process(128);
        klirr_set_param(0, 1.0);
        assert_eq!(klirr_param_count(), Param::COUNT as u32);
        assert_eq!(klirr_param_default(Param::Gain as u32), 0.5);
    }

    #[test]
    fn rejects_bad_sample_rates() {
        assert_eq!(klirr_init(f32::NAN), 0);
        assert_eq!(klirr_init(100.0), 0);
        assert_eq!(klirr_init(48_000.0), 1);
    }

    #[test]
    fn processes_like_the_engine() {
        assert_eq!(klirr_init(48_000.0), 1);
        let mut reference = Engine::new(48_000.0);
        for (p, v) in [
            (Param::Channel, 2.0),
            (Param::Gain, 0.8),
            (Param::ReverbMix, 0.3),
        ] {
            klirr_set_param(p as u32, v);
            reference.set_param(p, v);
        }
        assert_eq!(klirr_get_param(Param::Channel as u32), 2.0);

        let input: Vec<f32> = (0..1024).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
        let mut expected = vec![0.0; input.len()];
        reference.process(&input, &mut expected);

        let mut got = Vec::new();
        for block in input.chunks(128) {
            write(klirr_input_ptr(), block);
            klirr_process(block.len() as u32);
            got.extend(read(klirr_output_ptr(), block.len()));
        }
        assert_eq!(got, expected);

        let meters = read(klirr_take_meters(), METER_COUNT);
        assert!(
            meters[0] > 0.29 && meters[0] <= 0.3,
            "input peak {}",
            meters[0]
        );
        assert!(meters[1] > 0.0);
    }

    #[test]
    fn loads_custom_ir_and_reports_errors() {
        assert_eq!(klirr_init(48_000.0), 1);
        let mut ir = vec![0.0_f32; 2400];
        ir[0] = 1.0;
        ir[10] = -0.4;
        let ptr = klirr_ir_buffer(ir.len() as u32);
        write(ptr, &ir);
        assert_eq!(klirr_ir_load(44_100.0), 0);

        let ptr = klirr_ir_buffer(16);
        write(ptr, &[0.0; 16]);
        assert_eq!(klirr_ir_load(48_000.0), ir_error_code(IrError::Silent));
        assert!(klirr_ir_buffer(0).is_null());
        // Buffer was consumed: loading again sees an empty IR.
        assert_eq!(klirr_ir_load(48_000.0), ir_error_code(IrError::Empty));
    }

    #[test]
    fn process_caps_the_block_length() {
        assert_eq!(klirr_init(48_000.0), 1);
        klirr_process(u32::MAX); // must not panic
        assert!(klirr_latency_samples() > 0);
    }
}
