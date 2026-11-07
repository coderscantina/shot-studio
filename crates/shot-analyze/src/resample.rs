//! Streaming windowed-sinc resampler from any rate to whisper's 16 kHz.

use std::f64::consts::PI;

pub const WHISPER_RATE: u32 = 16_000;

/// Zero crossings of the sinc on each side, at the output rate.
const ZEROS: f64 = 16.0;

pub struct Resampler {
    rate: u32,
    /// Input step per output sample.
    step: f64,
    /// Low-pass cutoff relative to the input Nyquist frequency.
    cutoff: f64,
    /// Filter half width in input samples.
    half: usize,
    input: Vec<f32>,
    /// Position of the next output sample in `input`.
    pos: f64,
}

impl Resampler {
    pub fn new(rate: u32) -> Self {
        let step = f64::from(rate) / f64::from(WHISPER_RATE);
        let cutoff = (1.0 / step).min(1.0) * 0.95;
        let half = (ZEROS / cutoff).ceil() as usize;
        Self {
            rate,
            step,
            cutoff,
            half,
            input: vec![0.0; half],
            pos: half as f64,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Appends the 16 kHz output for `samples` to `out`. Output lags the input by the filter's
    /// half width until [`Self::flush`].
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        self.input.extend_from_slice(samples);
        while self.pos + self.half as f64 + 1.0 <= self.input.len() as f64 {
            out.push(self.sample_at(self.pos));
            self.pos += self.step;
        }
        let keep_from = (self.pos as usize).saturating_sub(self.half);
        self.input.drain(..keep_from);
        self.pos -= keep_from as f64;
    }

    /// Emits the delayed tail.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let consumed = self.input.len() as f64;
        self.input.extend(std::iter::repeat_n(0.0, self.half + 1));
        while self.pos < consumed {
            out.push(self.sample_at(self.pos));
            self.pos += self.step;
        }
        self.input.truncate(consumed as usize);
    }

    fn sample_at(&self, pos: f64) -> f32 {
        let center = pos.floor() as isize;
        let half = self.half as isize;
        let mut sum = 0.0;
        for i in (center - half + 1)..=(center + half) {
            let d = pos - i as f64;
            let x = d * self.cutoff;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (PI * x).sin() / (PI * x)
            };
            let window = 0.5 + 0.5 * (PI * d / (self.half as f64 + 1.0)).cos();
            sum += f64::from(self.input[i as usize]) * self.cutoff * sinc * window;
        }
        sum as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_tone_and_the_duration_in_chunks() {
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * 440.0 * f64::from(i) / 48_000.0).sin() as f32 * 0.5)
            .collect();
        let mut resampler = Resampler::new(48_000);
        let mut out = Vec::new();
        for chunk in tone.chunks(997) {
            resampler.push(chunk, &mut out);
        }
        resampler.flush(&mut out);
        assert!(out.len().abs_diff(16_000) <= 1, "{}", out.len());
        let error = out[100..15_900]
            .iter()
            .enumerate()
            .map(|(i, &y)| {
                let t = (i + 100) as f64 / 16_000.0;
                (y - (2.0 * PI * 440.0 * t).sin() as f32 * 0.5).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(error < 0.01, "{error}");
    }
}
