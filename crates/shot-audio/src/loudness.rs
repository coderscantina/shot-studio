//! EBU R128 measurement, gain to the target loudness and the true-peak limiter.

use crate::SAMPLE_RATE;
use crate::limiter::{self, TruePeakLimiter};
use ebur128::{EbuR128, Mode};

pub const TARGET_LUFS: f64 = -14.0;
pub const CEILING_DBTP: f64 = -1.0;

/// Margin under the ceiling for the AAC encode, whose output peaks higher than its input.
pub const CODEC_HEADROOM_DB: f64 = 1.0;

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

/// Gain, then the [`TruePeakLimiter`] at the ceiling less [`CODEC_HEADROOM_DB`], streaming. The
/// output runs [`Finish::LATENCY`] samples behind the input, like the voice chain's.
pub struct Finish {
    gain: f32,
    limiter: TruePeakLimiter,
}

impl Finish {
    /// Samples the output is delayed against the input.
    pub const LATENCY: usize = limiter::LATENCY;

    pub fn new(gain_db: f64, ceiling_dbtp: f64) -> Self {
        Self {
            gain: 10f64.powf(gain_db / 20.0) as f32,
            limiter: TruePeakLimiter::new(
                10f64.powf((ceiling_dbtp - CODEC_HEADROOM_DB) / 20.0) as f32
            ),
        }
    }

    /// Processes `samples` in place; state carries across calls. Output sample `i` is input
    /// sample `i - LATENCY`.
    pub fn process(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            *s *= self.gain;
        }
        self.limiter.process(samples);
    }

    /// Appends the final `LATENCY` samples of the stream to `out`.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let start = out.len();
        out.resize(start + Self::LATENCY, 0.0);
        self.process(&mut out[start..]);
    }
}
