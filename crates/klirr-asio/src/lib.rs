//! Opens an ASIO driver's own control panel: the window where the interface's buffer size
//! (and often its sample rate and clock) is set, the "Hardware Setup" of music programs.
//!
//! cpal does not offer this, but its `asio-sys` library holds the whole ASIO SDK, including
//! `ASIOControlPanel`, which works on the driver that is loaded at the time. ASIO loads one
//! driver per process: either the one cpal runs audio on (`open_loaded`) or, while no audio
//! runs, one loaded just for the panel (`open`).
//!
//! Call both from the thread that runs (or will run) the audio streams: ASIO drivers are COM
//! objects that expect to stay on the thread that loaded them. Some drivers show the panel as
//! a dialog and return once it is closed; others open it (or a separate program) and return at
//! once, and an in-process window needs that thread to dispatch its messages
//! (`pump_messages`).
//!
//! The crate holds code only on Windows with the `sdk` feature, which builds the ASIO SDK
//! (`klirr-native`'s `asio` feature turns it on); otherwise it is empty, so a workspace build
//! never needs the SDK. The names above are not links because the items are missing then.

#![cfg(all(windows, feature = "sdk"))]

use std::fmt;

/// What `ASIOControlPanel` and friends return (`ASIOError`).
type AsioError = i32;
const ASE_OK: AsioError = 0;
const ASE_SUCCESS: AsioError = 0x3f48_47a0;
const ASE_NOT_PRESENT: AsioError = -1000;

#[allow(unsafe_code)]
unsafe extern "C" {
    // `ASIOError ASIOControlPanel(void)` from the SDK's asio.cpp, compiled into asio-sys's
    // library. A C++ function: its name is the mangled one, as in asio-sys's own bindings.
    #[link_name = "\u{1}?ASIOControlPanel@@YAJXZ"]
    fn ASIOControlPanel() -> AsioError;
}

/// Why the panel did not open.
#[derive(Debug)]
pub enum Error {
    /// The driver could not be loaded (not installed, or in use by another program).
    Load(String),
    /// No driver is loaded.
    NotLoaded,
    /// The driver has no control panel, or refused to show it.
    Refused(i32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(e) => write!(f, "could not load the ASIO driver: {e}"),
            Self::NotLoaded => f.write_str("no ASIO driver is loaded"),
            Self::Refused(code) => write!(
                f,
                "the ASIO driver did not open its control panel (error {code})"
            ),
        }
    }
}

impl std::error::Error for Error {}

fn control_panel() -> Result<(), Error> {
    #[allow(unsafe_code)]
    // SAFETY: takes no arguments; with no driver loaded the SDK returns ASE_NotPresent.
    let result = unsafe { ASIOControlPanel() };
    match result {
        ASE_OK | ASE_SUCCESS => Ok(()),
        ASE_NOT_PRESENT => Err(Error::NotLoaded),
        code => Err(Error::Refused(code)),
    }
}

/// Opens the control panel of the driver that runs the audio now (loaded by cpal).
///
/// # Errors
///
/// If no driver is loaded, or it shows no panel.
pub fn open_loaded() -> Result<(), Error> {
    control_panel()
}

/// Loads `driver` (an ASIO driver name, as cpal lists it as a device), opens its control
/// panel and unloads it again. Only while no audio runs on ASIO: loading a driver unloads the
/// one that was loaded.
///
/// A driver that opens its panel in-process without waiting for it to close may close it
/// again when it is unloaded; most open a dialog, or their own program.
///
/// # Errors
///
/// If the driver cannot be loaded or shows no panel.
pub fn open(driver: &str) -> Result<(), Error> {
    init_com();
    let asio = asio_sys::Asio::new();
    let loaded = asio
        .load_driver(driver)
        .map_err(|e| Error::Load(e.to_string()))?;
    let result = control_panel();
    drop(loaded);
    result
}

/// Makes sure COM is set up on this thread (ASIO drivers are COM objects).
fn init_com() {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    #[allow(unsafe_code)]
    // SAFETY: no reserved pointer; S_FALSE (already set up) and RPC_E_CHANGED_MODE (set up
    // in the other mode, by cpal's WASAPI host) both leave COM usable on this thread.
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }
}

/// Dispatches the window messages waiting for this thread, so a control panel window the
/// driver opened on it responds. Returns at once when there are none.
pub fn pump_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    #[allow(unsafe_code)]
    // SAFETY: `msg` is a valid, writable MSG; a null window takes every message of this
    // thread.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
