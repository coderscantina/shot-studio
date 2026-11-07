//! EBU R128 measurement, gain to the target loudness and a peak limiter.

use crate::SAMPLE_RATE;
use ebur128::{EbuR128, Mode};

pub const TARGET_LUFS: f64 = -14.0;
pub const CEILING_DBTP: f64 = -1.0;

/// Margin under the ceiling for inter-sample peaks the sample-domain limiter cannot see.
const ISP_MARGIN_DB: f64 = 0.5;
const RELEASE_S: f32 = 0.050;

/// Integrated loudness and true peak of a stream, EBU R128.
pub struct Meter(EbuR128);

impl Meter {
    pub fn new() -> Self {
        Self(
            EbuR128::new(1, SAMPLE_RATE, Mode::I | Mode::TRUE_PEAK)
                .expect("mono 48 kHz is a valid ebur128 configuration"),
        )
    }

    pub fn add(&mut self, samples: &[f32]) {
        self.0
            .add_frames_f32(samples)
            .expect("mono frames always match the channel count");
    }

    /// Integrated loudness in LUFS, `None` when nothing above the gate was heard.
    pub fn loudness(&self) -> Option<f64> {
        self.0.loudness_global().ok().filter(|l| l.is_finite())
    }

    /// Highest true peak so far in dBTP.
    pub fn true_peak(&self) -> f64 {
        20.0 * self.0.true_peak(0).unwrap_or(0.0).log10()
    }
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

/// Gain to the target loudness, then a peak limiter (instant attack, smooth release, no
/// lookahead) at the ceiling.
pub struct Finish {
    gain: f32,
    limit: f32,
    reduction: f32,
    release: f32,
}

impl Finish {
    /// `measured` is the pass-1 loudness of the chain's output. A non-finite value applies no gain.
    pub fn new(measured: f64, target_lufs: f64, ceiling_dbtp: f64) -> Self {
        let gain_db = if measured.is_finite() {
            target_lufs - measured
        } else {
            0.0
        };
        Self {
            gain: 10f64.powf(gain_db / 20.0) as f32,
            limit: 10f64.powf((ceiling_dbtp - ISP_MARGIN_DB) / 20.0) as f32,
            reduction: 1.0,
            release: 1.0 - (-1.0 / (RELEASE_S * SAMPLE_RATE as f32)).exp(),
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for s in samples {
            let x = *s * self.gain;
            self.reduction += self.release * (1.0 - self.reduction);
            let needed = self.limit / x.abs().max(f32::MIN_POSITIVE);
            self.reduction = self.reduction.min(needed);
            *s = x * self.reduction;
        }
    }
}
