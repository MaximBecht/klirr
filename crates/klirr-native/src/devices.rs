//! What can be opened: the audio hosts (APIs) and their devices, as data for a user interface.

use std::error::Error;

use cpal::traits::{DeviceTrait, HostTrait};

use crate::host;

/// An audio host (API) the native host can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    /// The name [`crate::host::Options::host`] takes.
    pub id: String,
    /// A name to show.
    pub name: String,
    /// Whether this is the platform's default host.
    pub is_default: bool,
}

/// An input or output device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The device's name; [`crate::host::Options`] takes it to open the device.
    pub name: String,
    /// Channels of its default configuration (0 if it reports none).
    pub channels: u16,
    /// Whether it is the host's default device.
    pub is_default: bool,
}

/// The devices of one host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceList {
    /// Input devices.
    pub inputs: Vec<DeviceInfo>,
    /// Output devices.
    pub outputs: Vec<DeviceInfo>,
}

/// The hosts available on this platform (and in this build: ASIO needs the `asio` feature),
/// the default first.
pub fn hosts() -> Vec<HostInfo> {
    let default = cpal::default_host().id();
    let mut hosts: Vec<HostInfo> = cpal::available_hosts()
        .into_iter()
        .map(|id| HostInfo {
            id: id.name().to_owned(),
            name: if id.name().eq_ignore_ascii_case("wasapi") {
                "WASAPI shared".to_owned()
            } else {
                id.name().to_owned()
            },
            is_default: id == default,
        })
        .collect();
    hosts.sort_by_key(|h| !h.is_default);
    #[cfg(windows)]
    {
        // Right after shared WASAPI, the two being the same devices.
        let at = hosts.iter().position(|h| h.is_default).map_or(0, |i| i + 1);
        hosts.insert(
            at,
            HostInfo {
                id: crate::exclusive::HOST_ARG.to_owned(),
                name: crate::exclusive::HOST_NAME.to_owned(),
                is_default: false,
            },
        );
    }
    hosts
}

/// Lists the devices of a host (by [`HostInfo::id`]; None for the default host).
///
/// # Errors
///
/// If the host is unknown or cannot list its devices.
pub fn list(host_id: Option<&str>) -> Result<DeviceList, Box<dyn Error>> {
    #[cfg(windows)]
    if host_id.is_some_and(crate::exclusive::is_host) {
        return crate::exclusive::devices();
    }
    let h = host::host(host_id)?;
    let default_in = h.default_input_device().map(|d| host::name(&d));
    let default_out = h.default_output_device().map(|d| host::name(&d));
    let describe = |device: &cpal::Device, input: bool, default: Option<&String>| {
        let config = if input {
            device.default_input_config()
        } else {
            device.default_output_config()
        };
        let name = host::name(device);
        DeviceInfo {
            is_default: Some(&name) == default,
            name,
            channels: config.map_or(0, |c| c.channels()),
        }
    };
    Ok(DeviceList {
        inputs: h
            .input_devices()?
            .map(|d| describe(&d, true, default_in.as_ref()))
            .collect(),
        outputs: h
            .output_devices()?
            .map(|d| describe(&d, false, default_out.as_ref()))
            .collect(),
    })
}
