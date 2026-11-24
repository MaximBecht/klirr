//! Klirr's native audio host: the same engine as the web app, on the platform's audio API
//! (WASAPI shared or exclusive, or ASIO on Windows; Core Audio on macOS; ALSA or JACK on
//! Linux).
//!
//! It is the audio side of the desktop app (`desktop/`) and of the `klirr-native`
//! command-line tool.
//!
//! - [`host::Duplex`] opens an input and an output and runs the engine between them. While it
//!   runs it takes parameter changes, custom IRs and loopback measurements.
//! - [`devices`] lists the hosts and their devices.
//! - [`duplex`] is the audio side: the ring between the streams and the [`duplex::Stats`] the
//!   audio threads publish.
//! - [`setup`] opens the driver's or the system's own audio settings ("hardware setup").

pub mod devices;
pub mod duplex;
#[cfg(windows)]
pub mod exclusive;
pub mod host;
pub mod probe;
pub mod setup;
pub mod tone;
pub mod wav;
