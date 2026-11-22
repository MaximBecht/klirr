//! # klirr-dsp
//!
//! The DSP core of **Klirr**, the free & open-source guitar amp.
//!
//! The crate is pure Rust with no I/O. The same code runs in the browser (compiled to
//! WebAssembly inside an `AudioWorklet`) and, later, natively in the desktop app.
//!
//! ## Signal chain
//!
//! ```text
//! input trim → noise gate → high-pass → amp channel (4× oversampled) → tone stack
//!   → cabinet → master → delay → reverb → safety limiter → output
//! ```
//!
//! ## Real-time rules
//!
//! - [`Engine::process`] never allocates, locks or blocks.
//! - Output does not depend on how the host splits audio into blocks: every stage runs per
//!   sample and keeps its state from one call to the next.
//! - Parameter changes glide to their new value instead of jumping.
//!
//! The only allocating call is [`Engine::load_custom_ir`]; call it outside the audio callback
//! (in an `AudioWorklet`, from the message handler, which runs between `process` calls).
//!
//! ## Example
//!
//! ```
//! use klirr_dsp::{Engine, Param};
//!
//! let mut engine = Engine::new(48_000.0);
//! engine.set_param(Param::Channel, 1.0); // crunch
//! engine.set_param(Param::Gain, 0.6);
//!
//! let input = vec![0.0_f32; 128];
//! let mut output = vec![0.0_f32; 128];
//! engine.process(&input, &mut output);
//! ```

pub mod amp;
pub mod biquad;
pub mod cabinet;
pub mod convolver;
pub mod delay;
mod engine;
pub mod gate;
pub mod limiter;
pub mod oversample;
mod params;
pub mod reverb;
pub mod shaper;
pub mod smooth;
pub mod tone;
pub mod util;

pub use amp::Channel;
pub use cabinet::{Cabinet, IrError};
pub use engine::{Engine, Meters};
pub use params::{Param, ParamInfo};
