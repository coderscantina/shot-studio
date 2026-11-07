//! A lookahead true-peak limiter.
//!
//! Peaks are found on the signal 4x oversampled, so the peaks between samples that a decoder
//! reconstructs count too. The gain reaches what a peak needs by the time the peak plays, over a
//! linear attack of [`LOOKAHEAD`] samples, and recovers with an exponential release. The output
//! runs [`LATENCY`] samples behind the input.

use std::collections::VecDeque;

use crate::SAMPLE_RATE;

/// Input samples each interpolated value is computed from.
const TAPS: usize = 32;
/// Interpolated values between two samples, the oversampling factor less one.
const PHASES: usize = 3;
/// Samples of lookahead, also the attack: 5 ms.
const LOOKAHEAD: usize = SAMPLE_RATE as usize / 200;
const RELEASE_S: f32 = 0.050;
/// Samples the output is delayed against the input.
pub const LATENCY: usize = TAPS / 2 + LOOKAHEAD;

pub struct TruePeakLimiter {
    limit: f32,
    /// `phases[p][k]` weighs window sample `k` for the value `(p + 1) / 4` past the window's
    /// middle sample.
    phases: [[f32; TAPS]; PHASES],
    /// The last `TAPS` inputs, written twice so the window is one slice.
    window: [f32; 2 * TAPS],
    head: usize,
    /// Peak of the stretch from the middle sample to the next, from the previous input.
    last_peak: f32,
    /// Gain each sample needs, as the lowest of a sliding window: (sample, gain) pairs with
    /// rising gains.
    needed: VecDeque<(u64, f32)>,
    /// Inputs and held gains waiting for their turn, `LATENCY` and `LOOKAHEAD` long.
    delay: VecDeque<f32>,
    held: VecDeque<f32>,
    held_sum: f64,
    released: f32,
    release: f32,
    /// Input samples seen.
    count: u64,
}

impl TruePeakLimiter {
    /// Keeps true peaks at or under `ceiling` (linear).
    pub fn new(ceiling: f32) -> Self {
        let middle = TAPS / 2 - 1;
        let mut phases = [[0.0; TAPS]; PHASES];
        for (p, phase) in phases.iter_mut().enumerate() {
            let fraction = (p + 1) as f64 / (PHASES + 1) as f64;
            for (k, weight) in phase.iter_mut().enumerate() {
                let distance = middle as f64 + fraction - k as f64;
                *weight = (sinc(distance) * kaiser(distance / (TAPS / 2) as f64)) as f32;
            }
            let sum: f32 = phase.iter().sum();
            phase.iter_mut().for_each(|w| *w /= sum);
        }
        Self {
            limit: ceiling,
            phases,
            window: [0.0; 2 * TAPS],
            head: 0,
            last_peak: 0.0,
            needed: VecDeque::with_capacity(LOOKAHEAD + 2),
            delay: VecDeque::from(vec![0.0; LATENCY]),
            held: VecDeque::from(vec![1.0; LOOKAHEAD]),
            held_sum: LOOKAHEAD as f64,
            released: 1.0,
            release: 1.0 - (-1.0 / (RELEASE_S * SAMPLE_RATE as f32)).exp(),
            count: 0,
        }
    }

    /// Limits `samples` in place; state carries across calls. Output sample `i` is input sample
    /// `i - LATENCY`.
    pub fn process(&mut self, samples: &mut [f32]) {
        for s in samples {
            *s = self.step(*s);
        }
    }

    fn step(&mut self, x: f32) -> f32 {
        self.window[self.head] = x;
        self.window[self.head + TAPS] = x;
        self.head = (self.head + 1) % TAPS;
        let window = &self.window[self.head..self.head + TAPS];
        let middle = window[TAPS / 2 - 1];
        let mut peak = middle.abs().max(window[TAPS / 2].abs());
        for phase in &self.phases {
            let value: f32 = window.iter().zip(phase).map(|(x, w)| x * w).sum();
            peak = peak.max(value.abs());
        }
        // The middle sample touches the stretches before and after it.
        let need = (self.limit / peak.max(self.last_peak).max(f32::MIN_POSITIVE)).min(1.0);
        self.last_peak = peak;

        while self.needed.back().is_some_and(|(_, g)| *g >= need) {
            self.needed.pop_back();
        }
        self.needed.push_back((self.count, need));
        while self
            .needed
            .front()
            .is_some_and(|(at, _)| *at + (LOOKAHEAD as u64) < self.count)
        {
            self.needed.pop_front();
        }
        self.count += 1;
        let hold = self.needed.front().map_or(1.0, |(_, g)| *g);
        self.released = hold.min(self.released + self.release * (1.0 - self.released));

        self.held_sum += f64::from(self.released) - f64::from(self.held.pop_front().unwrap_or(1.0));
        self.held.push_back(self.released);
        let gain = (self.held_sum / LOOKAHEAD as f64) as f32;

        self.delay.push_back(x);
        self.delay.pop_front().unwrap_or(0.0) * gain
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let x = std::f64::consts::PI * x;
        x.sin() / x
    }
}

/// The Kaiser window at `x` in -1..1.
fn kaiser(x: f64) -> f64 {
    const BETA: f64 = 7.0;
    bessel_i0(BETA * (1.0 - x * x).max(0.0).sqrt()) / bessel_i0(BETA)
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..32 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    /// True peak of a stream, 16x oversampled by a long sinc, as a reference.
    fn reference_peak(samples: &[f32]) -> f32 {
        let mut peak = 0f32;
        for i in 64..samples.len() - 64 {
            for p in 0..16 {
                let t = i as f64 + p as f64 / 16.0;
                let value: f64 = (i - 63..i + 64)
                    .map(|k| f64::from(samples[k]) * sinc(t - k as f64))
                    .sum();
                peak = peak.max(value.abs() as f32);
            }
        }
        peak
    }

    #[test]
    fn holds_peaks_between_samples_under_the_ceiling() {
        // A tone at a quarter of the rate, sampled 45 degrees off its crests: every sample is
        // at 0.707 of the true peak.
        let len = SAMPLE_RATE as usize / 10;
        let tone = |i: usize| {
            let phase = std::f32::consts::TAU * i as f32 / 4.0 + std::f32::consts::FRAC_PI_4;
            0.95 * phase.sin()
        };
        let input: Vec<f32> = (0..len).map(tone).collect();
        assert!(input.iter().all(|s| s.abs() < 0.68));
        assert!(reference_peak(&input) > 0.94);

        let ceiling = 0.5;
        let mut output = input.clone();
        let mut limiter = TruePeakLimiter::new(ceiling);
        limiter.process(&mut output);
        let peak = reference_peak(&output[LATENCY..]);
        assert!(peak <= ceiling * 1.01, "true peak {peak}");
        assert!(peak >= ceiling * 0.97, "true peak {peak}");
    }

    #[test]
    fn leaves_quiet_signal_alone_but_delayed() {
        let input: Vec<f32> = (0..4800).map(|i| 0.2 * (i as f32 * 0.03).sin()).collect();
        let mut output = input.clone();
        TruePeakLimiter::new(0.9).process(&mut output);
        for (out, x) in output[LATENCY..].iter().zip(&input) {
            assert!((out - x).abs() < 1e-6);
        }
    }
}
