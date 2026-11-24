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
    /// What [`crate::setup::open`] opens for it ("the ASIO driver's control panel"); None
    /// where there is nothing to open.
    pub hardware_setup: Option<&'static str>,
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

/// The buffer sizes and sample rates a host and its devices allow, for the audio settings.
///
/// Klirr only asks for a size or rate when it opens its own streams; it never changes a
/// setting that belongs to the system (the Windows audio engine, a JACK server) or to the
/// interface for every program (an ASIO driver's sample rate). Where the size is not
/// Klirr's, [`BufferChoice::sizes`] is empty and [`BufferChoice::set_by`] says who sets
/// it; where the rate is not, [`BufferChoice::rates`] is empty. (Some ASIO drivers remember
/// the last size a program asked for, as they do for every ASIO program.)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BufferChoice {
    /// The sample rate the devices would open at; the sizes count frames at this rate.
    pub sample_rate: u32,
    /// Sample rates Klirr can ask for, lowest first. Empty when the rate is not Klirr's to
    /// choose (it runs at [`BufferChoice::sample_rate`], set by the system or the driver).
    pub rates: Vec<u32>,
    /// The size the devices run at when Klirr asks for none, in frames, where it is known
    /// (an ASIO driver's own setting is not).
    pub default: Option<u32>,
    /// Sizes Klirr can ask for, in frames, smallest first. Empty when the size is not
    /// Klirr's to choose.
    pub sizes: Vec<u32>,
    /// Who sets the size when Klirr cannot, for the user ("the Windows audio engine").
    pub set_by: Option<&'static str>,
}

/// The largest buffer offered: beyond this a guitar amp is unplayable anyway.
pub(crate) const LARGEST_OFFERED: u32 = 4096;

/// Sizes to offer between `min` and `max` frames: the powers of two in between (every
/// driver takes those), and `min` itself, the smallest the devices allow.
pub(crate) fn offered_sizes(min: u32, max: u32) -> Vec<u32> {
    let max = max.min(LARGEST_OFFERED);
    if min == 0 || min > max {
        return Vec::new();
    }
    let mut sizes = vec![min];
    let mut size = min.next_power_of_two();
    while size <= max {
        sizes.push(size);
        size *= 2;
    }
    sizes.dedup();
    sizes
}

/// The range both ends allow, if either reports one.
fn common_range(
    a: &cpal::SupportedBufferSize,
    b: &cpal::SupportedBufferSize,
) -> Option<(u32, u32)> {
    use cpal::SupportedBufferSize::{Range, Unknown};
    match (a, b) {
        (Range { min: a0, max: a1 }, Range { min: b0, max: b1 }) => {
            Some(((*a0).max(*b0), (*a1).min(*b1)))
        }
        (Range { min, max }, Unknown) | (Unknown, Range { min, max }) => Some((*min, *max)),
        (Unknown, Unknown) => None,
    }
}

/// What buffer sizes and sample rates `host_id` (as [`HostInfo::id`]; None for the default
/// host) allows on the devices whose names contain `input` and `output` (None for the
/// defaults). `rate` is the sample rate Klirr would ask for (None: the devices' own); the
/// sizes are counted at it where the rate is Klirr's to choose.
///
/// # Errors
///
/// If the host or a device cannot be found or does not report its configuration.
pub fn buffer_choice(
    host_id: Option<&str>,
    input: Option<&str>,
    output: Option<&str>,
    rate: Option<u32>,
) -> Result<BufferChoice, Box<dyn Error>> {
    #[cfg(windows)]
    if host_id.is_some_and(crate::exclusive::is_host) {
        return crate::exclusive::buffer_choice(input, output, rate);
    }
    // On cpal's hosts the rate is the system's or the driver's for every program.
    let _ = rate;
    let h = host::host(host_id)?;
    let (input, output) = host::pick_devices(&h, input, output)?;
    let in_config = input.default_input_config()?;
    let out_config = output.default_output_config()?;
    let range = common_range(in_config.buffer_size(), out_config.buffer_size());
    let mut choice = BufferChoice {
        sample_rate: out_config.sample_rate(),
        ..BufferChoice::default()
    };
    let name = h.id().name();
    if name.eq_ignore_ascii_case("wasapi") {
        // The engine's period sets the latency; a client buffer cannot go below it.
        choice.set_by = Some("the Windows audio engine");
        choice.default = range.and_then(|(min, max)| (min == max).then_some(min));
        return Ok(choice);
    }
    match range {
        Some((min, max)) if min < max => choice.sizes = offered_sizes(min, max),
        Some((min, max)) if min == max => {
            choice.default = Some(min);
            choice.set_by = Some(if name.eq_ignore_ascii_case("jack") {
                "the JACK server"
            } else {
                "the driver"
            });
        }
        _ => choice.set_by = Some("the driver"),
    }
    if choice.sizes.is_empty() && choice.set_by.is_none() {
        choice.set_by = Some("the driver");
    }
    Ok(choice)
}

/// Whether the sample rate of `host_id` (as [`HostInfo::id`]; None for the default host) is
/// Klirr's to choose: only in WASAPI exclusive mode, where the device runs at Klirr's rate
/// while Klirr holds it. Elsewhere asking for a rate would change it for every program (an
/// ASIO driver) or is not possible at all.
pub fn chooses_rate(host_id: Option<&str>) -> bool {
    #[cfg(windows)]
    return host_id.is_some_and(crate::exclusive::is_host);
    #[cfg(not(windows))]
    {
        let _ = host_id;
        false
    }
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
            hardware_setup: crate::setup::describe(id.name()),
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
                hardware_setup: crate::setup::describe(crate::exclusive::HOST_ARG),
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

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::SupportedBufferSize::{Range, Unknown};

    #[test]
    fn offered_sizes_are_the_smallest_then_powers_of_two() {
        assert_eq!(offered_sizes(32, 512), [32, 64, 128, 256, 512]);
        assert_eq!(offered_sizes(144, 1024), [144, 256, 512, 1024]);
        assert_eq!(offered_sizes(64, 64), [64]);
    }

    #[test]
    fn offered_sizes_stop_at_the_largest_offered() {
        assert_eq!(offered_sizes(2048, 1 << 20), [2048, 4096]);
        assert!(offered_sizes(8192, 16384).is_empty());
        assert!(offered_sizes(0, 512).is_empty());
        assert!(offered_sizes(512, 256).is_empty());
    }

    #[test]
    fn the_common_range_is_what_both_ends_allow() {
        let a = Range { min: 32, max: 2048 };
        let b = Range { min: 64, max: 1024 };
        assert_eq!(common_range(&a, &b), Some((64, 1024)));
        assert_eq!(common_range(&a, &Unknown), Some((32, 2048)));
        assert_eq!(common_range(&Unknown, &Unknown), None);
    }
}
