//! Amp channels: clean, crunch and high gain.
//!
//! Each channel is `pre-EQ → drive → waveshaper(s) at 4× rate → DC blocker`. The output
//! level depends only on the settings, never on the signal in the current block, so playing
//! dynamics survive. All curves are smooth (no kinks), which keeps
//! the harmonic series falling off quickly and aliasing low.

use crate::biquad::{Biquad, Coeffs};
use crate::oversample::{FACTOR, Oversampler4x};
use crate::shaper::{AdaaTanh, curve_value};
use crate::util::db_to_gain;

/// The amp channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Channel {
    /// Clean with gentle tube-like warmth when pushed.
    Clean = 0,
    /// Asymmetric, mid-forward overdrive.
    Crunch = 1,
    /// Two cascaded gain stages with a tight low end.
    HighGain = 2,
}

impl Channel {
    /// All channels, in index order.
    pub const ALL: [Self; 3] = [Self::Clean, Self::Crunch, Self::HighGain];

    /// Maps a parameter value (rounded) to a channel.
    pub fn from_value(value: f32) -> Self {
        match value.round() as i32 {
            i32::MIN..=0 => Self::Clean,
            1 => Self::Crunch,
            _ => Self::HighGain,
        }
    }
}

/// A first-order DC blocker (high-pass at ~10 Hz).
#[derive(Clone, Debug)]
pub struct DcBlocker {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    /// Creates a DC blocker for `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            r: 1.0 - 2.0 * std::f32::consts::PI * 10.0 / sample_rate,
            x1: 0.0,
            y1: 0.0,
        }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = crate::util::flush_denormal(y);
        y
    }
}

/// Static design values for one channel.
#[derive(Clone, Copy, Debug)]
struct Voicing {
    /// Drive at gain 0 and the extra drive at gain 1, in dB.
    drive_db: (f32, f32),
    bias: f32,
    /// Second stage drive (high gain only); `None` for single-stage channels.
    stage2_drive: Option<f32>,
    /// Output level correction, in dB, applied per unit of drive dB.
    makeup_slope: f32,
    makeup_db: f32,
}

impl Voicing {
    fn of(channel: Channel) -> Self {
        match channel {
            Channel::Clean => Self {
                drive_db: (0.0, 18.0),
                bias: 0.1,
                stage2_drive: None,
                makeup_slope: -0.5,
                makeup_db: 0.0,
            },
            Channel::Crunch => Self {
                drive_db: (6.0, 30.0),
                bias: 0.2,
                stage2_drive: None,
                makeup_slope: 0.0,
                makeup_db: -6.0,
            },
            Channel::HighGain => Self {
                drive_db: (12.0, 30.0),
                bias: 0.3,
                stage2_drive: Some(4.0),
                makeup_slope: 0.0,
                makeup_db: -7.0,
            },
        }
    }
}

/// One amp channel with its own filters and oversampler.
#[derive(Clone, Debug)]
pub struct AmpChannel {
    voicing: Voicing,
    /// `tanh(bias)`: subtracted so silence stays silence. Biasing the curve makes the two
    /// half-waves clip differently, adding the even harmonics typical of tube stages.
    bias_offset: f32,
    stage1: AdaaTanh,
    stage2: AdaaTanh,
    pre: [Biquad; 2],
    oversampler: Oversampler4x,
    /// Filters between the two gain stages, running at the oversampled rate.
    inter: [Biquad; 2],
    dc: DcBlocker,
}

impl AmpChannel {
    /// Creates the channel for `sample_rate` (the base rate).
    pub fn new(channel: Channel, sample_rate: f32) -> Self {
        let os_rate = sample_rate * FACTOR as f32;
        let pre = match channel {
            Channel::Clean => [Biquad::default(), Biquad::default()],
            Channel::Crunch => [
                Biquad::new(Coeffs::highpass(110.0, 0.707, sample_rate)),
                Biquad::new(Coeffs::peaking(720.0, 0.8, 6.0, sample_rate)),
            ],
            Channel::HighGain => [
                Biquad::new(Coeffs::highpass(180.0, 0.707, sample_rate)),
                Biquad::new(Coeffs::peaking(1000.0, 0.7, 6.0, sample_rate)),
            ],
        };
        let inter = match channel {
            Channel::HighGain => [
                // Coupling capacitor (removes stage-1 DC) and a treble roll-off, like the
                // Miller capacitance between real preamp stages.
                Biquad::new(Coeffs::highpass(30.0, 0.707, os_rate)),
                Biquad::new(Coeffs::lowpass(7000.0, 0.707, os_rate)),
            ],
            _ => [Biquad::default(), Biquad::default()],
        };
        let voicing = Voicing::of(channel);
        Self {
            voicing,
            bias_offset: curve_value(voicing.bias),
            stage1: AdaaTanh::at_rest(voicing.bias),
            stage2: AdaaTanh::new(),
            pre,
            oversampler: Oversampler4x::new(),
            inter,
            dc: DcBlocker::new(sample_rate),
        }
    }

    /// Clears all filter state.
    pub fn reset(&mut self) {
        for f in self.pre.iter_mut().chain(self.inter.iter_mut()) {
            f.reset();
        }
        self.oversampler.reset();
        self.stage1.reset();
        self.stage2.reset();
        self.dc.reset();
    }

    /// Drive factor and output makeup for a gain knob value in `0..=1`.
    #[inline]
    pub fn drive_and_makeup(&self, gain: f32) -> (f32, f32) {
        let (base, range) = self.voicing.drive_db;
        let drive_db = base + range * gain;
        let makeup_db = self.voicing.makeup_db + self.voicing.makeup_slope * drive_db;
        (db_to_gain(drive_db), db_to_gain(makeup_db))
    }

    /// Processes one sample. `drive` and `makeup` come from [`Self::drive_and_makeup`].
    #[inline]
    pub fn process(&mut self, x: f32, drive: f32, makeup: f32) -> f32 {
        let x = self.pre[0].process(x);
        let x = self.pre[1].process(x);
        let bias = self.voicing.bias;
        let offset = self.bias_offset;
        let mut block = self.oversampler.upsample(x * drive);
        match self.voicing.stage2_drive {
            None => {
                for v in &mut block {
                    *v = self.stage1.process(*v + bias) - offset;
                }
            }
            Some(drive2) => {
                for v in &mut block {
                    let s1 = self.stage1.process(*v + bias) - offset;
                    let s1 = self.inter[0].process(s1);
                    let s1 = self.inter[1].process(s1);
                    *v = self.stage2.process(s1 * drive2);
                }
            }
        }
        let y = self.oversampler.downsample(block);
        self.dc.process(y) * makeup
    }
}
