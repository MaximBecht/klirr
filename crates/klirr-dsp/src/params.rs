//! Engine parameters: identifiers, ranges and defaults.

/// A parameter of the [`crate::Engine`].
///
/// The numeric values are stable: hosts (the WebAssembly bridge, the desktop app, presets)
/// address parameters by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Param {
    /// Input trim in dB.
    InputGainDb = 0,
    /// Noise gate threshold in dBFS; at the minimum the gate is off.
    GateThresholdDb = 1,
    /// Amp channel: 0 = clean, 1 = crunch, 2 = high gain.
    Channel = 2,
    /// Amp gain (drive), 0 – 1.
    Gain = 3,
    /// Bass in dB.
    BassDb = 4,
    /// Mid in dB.
    MidDb = 5,
    /// Treble in dB.
    TrebleDb = 6,
    /// Cabinet: 0 = off, 1 = 1×12 combo, 2 = 4×12 stack, 3 = custom IR.
    Cabinet = 7,
    /// Master volume in dB.
    MasterDb = 8,
    /// Delay time in milliseconds.
    DelayTimeMs = 9,
    /// Delay feedback, 0 – 0.95.
    DelayFeedback = 10,
    /// Delay level, 0 – 1.
    DelayMix = 11,
    /// Reverb room size, 0 – 1.
    ReverbSize = 12,
    /// Reverb high-frequency damping, 0 – 1.
    ReverbDamping = 13,
    /// Reverb level, 0 – 1.
    ReverbMix = 14,
    /// Bypass: 0 = amp on, 1 = dry signal.
    Bypass = 15,
}

/// Range, default and display information for a [`Param`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamInfo {
    /// Stable machine name (for presets and hosts).
    pub name: &'static str,
    /// Smallest value.
    pub min: f32,
    /// Largest value.
    pub max: f32,
    /// Default value.
    pub default: f32,
    /// Unit shown next to the value (may be empty).
    pub unit: &'static str,
    /// `true` for parameters that only take whole numbers (selectors, switches).
    pub stepped: bool,
}

impl Param {
    /// All parameters, in index order.
    pub const ALL: [Self; 16] = [
        Self::InputGainDb,
        Self::GateThresholdDb,
        Self::Channel,
        Self::Gain,
        Self::BassDb,
        Self::MidDb,
        Self::TrebleDb,
        Self::Cabinet,
        Self::MasterDb,
        Self::DelayTimeMs,
        Self::DelayFeedback,
        Self::DelayMix,
        Self::ReverbSize,
        Self::ReverbDamping,
        Self::ReverbMix,
        Self::Bypass,
    ];

    /// Number of parameters.
    pub const COUNT: usize = Self::ALL.len();

    /// Looks up a parameter by its stable index.
    pub fn from_index(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }

    /// The parameter's stable index.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Range and default.
    pub const fn info(self) -> ParamInfo {
        const fn p(
            name: &'static str,
            min: f32,
            max: f32,
            default: f32,
            unit: &'static str,
        ) -> ParamInfo {
            ParamInfo {
                name,
                min,
                max,
                default,
                unit,
                stepped: false,
            }
        }
        const fn s(name: &'static str, max: f32, default: f32) -> ParamInfo {
            ParamInfo {
                name,
                min: 0.0,
                max,
                default,
                unit: "",
                stepped: true,
            }
        }
        match self {
            Self::InputGainDb => p("input_gain_db", -24.0, 24.0, 0.0, "dB"),
            Self::GateThresholdDb => {
                p("gate_threshold_db", crate::gate::OFF_DB, -20.0, -70.0, "dB")
            }
            Self::Channel => s("channel", 2.0, 0.0),
            Self::Gain => p("gain", 0.0, 1.0, 0.5, ""),
            Self::BassDb => p("bass_db", -12.0, 12.0, 0.0, "dB"),
            Self::MidDb => p("mid_db", -12.0, 12.0, 0.0, "dB"),
            Self::TrebleDb => p("treble_db", -12.0, 12.0, 0.0, "dB"),
            Self::Cabinet => s("cabinet", 3.0, 1.0),
            Self::MasterDb => p("master_db", -60.0, 12.0, -6.0, "dB"),
            Self::DelayTimeMs => p(
                "delay_time_ms",
                10.0,
                crate::delay::MAX_TIME_MS,
                300.0,
                "ms",
            ),
            Self::DelayFeedback => p("delay_feedback", 0.0, crate::delay::MAX_FEEDBACK, 0.35, ""),
            Self::DelayMix => p("delay_mix", 0.0, 1.0, 0.0, ""),
            Self::ReverbSize => p("reverb_size", 0.0, 1.0, 0.5, ""),
            Self::ReverbDamping => p("reverb_damping", 0.0, 1.0, 0.5, ""),
            Self::ReverbMix => p("reverb_mix", 0.0, 1.0, 0.0, ""),
            Self::Bypass => s("bypass", 1.0, 0.0),
        }
    }

    /// Clamps (and for stepped parameters rounds) `value` into range.
    /// Returns `None` for NaN, which callers ignore.
    pub fn sanitise(self, value: f32) -> Option<f32> {
        if value.is_nan() {
            return None;
        }
        let info = self.info();
        let v = value.clamp(info.min, info.max);
        Some(if info.stepped { v.round() } else { v })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_are_stable_and_roundtrip() {
        for (i, p) in Param::ALL.iter().enumerate() {
            assert_eq!(p.index(), i);
            assert_eq!(Param::from_index(i as u32), Some(*p));
        }
        assert_eq!(Param::from_index(Param::COUNT as u32), None);
    }

    #[test]
    fn defaults_are_in_range() {
        for p in Param::ALL {
            let i = p.info();
            assert!(i.min <= i.default && i.default <= i.max, "{}", i.name);
        }
    }

    #[test]
    fn sanitise_clamps_rounds_and_rejects_nan() {
        assert_eq!(Param::Gain.sanitise(2.0), Some(1.0));
        assert_eq!(Param::Channel.sanitise(1.4), Some(1.0));
        assert_eq!(Param::MasterDb.sanitise(f32::NAN), None);
        assert_eq!(Param::MasterDb.sanitise(f32::NEG_INFINITY), Some(-60.0));
    }
}
