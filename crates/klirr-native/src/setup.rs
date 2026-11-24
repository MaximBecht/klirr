//! "Hardware setup": where the settings Klirr does not change live, opened for the user.
//!
//! Klirr only asks for what applies to itself while it runs (see
//! [`crate::devices::BufferChoice`]). Settings that apply to every program, such as an ASIO
//! interface's buffer size or the Windows mix format, belong to the driver or the system, and
//! stay there after Klirr closes or crashes. This module opens the place they are set:
//!
//! - ASIO: the driver's own control panel (through `klirr-asio`, in builds with ASIO).
//! - WASAPI shared and exclusive: the Windows Sound control panel, whose device properties
//!   hold the shared-mode format and the "allow exclusive control" switch.
//! - Core Audio: Audio MIDI Setup.
//!
//! JACK, ALSA and other hosts have no standard place, so there is nothing to open.

use std::error::Error;

use crate::host::Duplex;

/// What [`open`] opens for `host_id` (a host id as [`crate::devices::HostInfo::id`]), for
/// the user; None where there is nothing to open.
pub fn describe(host_id: &str) -> Option<&'static str> {
    let id = host_id.to_ascii_lowercase();
    match id.as_str() {
        "asio" => Some("the ASIO driver's control panel"),
        "wasapi" => Some("the Windows Sound settings"),
        #[cfg(windows)]
        _ if crate::exclusive::is_host(&id) => Some("the Windows Sound settings"),
        "coreaudio" => Some("Audio MIDI Setup"),
        _ => None,
    }
}

/// Opens the hardware setup of `host_id` (None: the default host) for the output device
/// whose name contains `output` (None: the default output). `running` is the audio that
/// runs now, if any: with ASIO the panel belongs to the driver it runs on.
///
/// Call it from the thread that opens the audio: an ASIO driver's panel is shown on it, and
/// many drivers show it as a dialog that returns only once the user closes it. When the user
/// changes the buffer size there, the driver asks for a restart (see
/// [`crate::duplex::Stats::reset_requests`]).
///
/// # Errors
///
/// If the host has no hardware setup, or it could not be opened.
pub fn open(
    host_id: Option<&str>,
    output: Option<&str>,
    running: Option<&Duplex>,
) -> Result<(), Box<dyn Error>> {
    let id = match host_id {
        Some(id) => id.to_ascii_lowercase(),
        None => cpal::default_host().id().name().to_ascii_lowercase(),
    };
    if id == "asio" {
        return open_asio(output, running);
    }
    let _ = (output, running);
    match describe(&id) {
        Some(_) => open_system_settings(),
        None => Err(format!("{id} has no hardware setup to open").into()),
    }
}

#[cfg(all(windows, feature = "asio"))]
fn open_asio(output: Option<&str>, running: Option<&Duplex>) -> Result<(), Box<dyn Error>> {
    use cpal::traits::HostTrait;

    // Audio on ASIO: the panel is the running driver's (listing the drivers now would load
    // them, which ASIO does not allow while one runs).
    if let Some(d) = running.filter(|d| d.info.host.eq_ignore_ascii_case("asio")) {
        let wanted = output.is_none_or(|part| d.info.output.contains(part));
        if !wanted {
            return Err(format!(
                "audio runs on the ASIO driver {}; ASIO opens one driver at a time, so stop                  the audio to set up another",
                d.info.output
            )
            .into());
        }
        return Ok(klirr_asio::open_loaded()?);
    }
    // The driver's name, as cpal lists it (an ASIO "device" is a driver).
    let host = crate::host::host(Some("asio"))?;
    let device = match output {
        Some(part) => crate::host::find(host.output_devices()?, part, "output")?,
        None => host
            .default_output_device()
            .ok_or("no ASIO driver is installed")?,
    };
    Ok(klirr_asio::open(&crate::host::name(&device))?)
}

#[cfg(not(all(windows, feature = "asio")))]
fn open_asio(_output: Option<&str>, _running: Option<&Duplex>) -> Result<(), Box<dyn Error>> {
    Err("this build has no ASIO".into())
}

/// Dispatches the window messages waiting for this thread, so an ASIO control panel the
/// driver opened on it (without waiting for it to close) responds. Call it regularly on the
/// thread that runs [`open`]; it returns at once when there is nothing to do, and does
/// nothing in builds without ASIO.
pub fn pump_messages() {
    #[cfg(all(windows, feature = "asio"))]
    klirr_asio::pump_messages();
}

/// The system's own audio settings.
fn open_system_settings() -> Result<(), Box<dyn Error>> {
    #[cfg(windows)]
    let mut command = {
        // The classic Sound control panel, on its playback tab: unlike the Settings app it
        // shows each device's format and exclusive-mode switches.
        let mut c = std::process::Command::new("control.exe");
        c.arg("mmsys.cpl,,0");
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.args(["-b", "com.apple.audio.AudioMIDISetup"]);
        c
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    return Err("this system has no audio settings to open".into());
    #[cfg(any(windows, target_os = "macos"))]
    {
        command
            .spawn()
            .map_err(|e| format!("could not open the system's audio settings: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_host_says_what_it_opens() {
        assert_eq!(describe("ASIO"), Some("the ASIO driver's control panel"));
        assert_eq!(describe("CoreAudio"), Some("Audio MIDI Setup"));
        assert_eq!(describe("JACK"), None);
        assert_eq!(describe("ALSA"), None);
    }

    #[cfg(windows)]
    #[test]
    fn both_wasapi_modes_open_the_sound_settings() {
        assert_eq!(describe("WASAPI"), Some("the Windows Sound settings"));
        assert_eq!(
            describe(crate::exclusive::HOST_ARG),
            Some("the Windows Sound settings")
        );
    }
}
