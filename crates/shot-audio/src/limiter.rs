//! A lookahead true-peak limiter.
//!
//! Peaks are found on the signal 4x oversampled, so the peaks between samples that a decoder
//! reconstructs count too. The gain reaches what a peak needs by the time the peak plays, over a
//! linear attack of [`LOOKAHEAD`] samples, and recovers with an exponential release. The output
//! runs [`LATENCY`] samples behind the input.
//!
//! The interpolation runs on blocks of [`BLOCK`] samples, eight outputs at a time with every
//! phase in registers, so it vectorizes. Each value is summed in tap order as one sample at a
//! time would, so the output does not depend on how the input is split.

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
/// Samples interpolated at a time.
const BLOCK: usize = 1024;
/// Interpolated values computed side by side.
const LANES: usize = 8;
/// Room for the sliding window's `LOOKAHEAD + 1` needed gains, a power of two to wrap cheaply.
const NEEDED: usize = (LOOKAHEAD + 1).next_power_of_two();

pub struct TruePeakLimiter {
    limit: f32,
    /// `phases[p][k]` weighs window sample `k` for the value `(p + 1) / 4` past the window's
    /// middle sample.
    phases: [[f32; TAPS]; PHASES],
    /// The last `TAPS - 1` inputs, then the block being processed.
    input: Box<[f32; TAPS - 1 + BLOCK]>,
    /// Peak of the stretch from each block sample's window middle to the next.
    peaks: Box<[f32; BLOCK]>,
    /// Peak of the stretch from the middle sample to the next, from the previous input.
    last_peak: f32,
    /// Gain each sample needs, as the lowest of a sliding window: (sample, gain) pairs with
    /// rising gains, in a ring of `needed_len` from `needed_head`.
    needed: Box<[(u64, f32); NEEDED]>,
    needed_head: usize,
    needed_len: usize,
    /// Held gains of the last `LOOKAHEAD` samples and inputs of the last `LATENCY`, as rings.
    held: Box<[f32; LOOKAHEAD]>,
    delay: Box<[f32; LATENCY]>,
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
            input: Box::new([0.0; TAPS - 1 + BLOCK]),
            peaks: Box::new([0.0; BLOCK]),
            last_peak: 0.0,
            needed: Box::new([(0, 1.0); NEEDED]),
            needed_head: 0,
            needed_len: 0,
            held: Box::new([1.0; LOOKAHEAD]),
            delay: Box::new([0.0; LATENCY]),
            held_sum: LOOKAHEAD as f64,
            released: 1.0,
            release: 1.0 - (-1.0 / (RELEASE_S * SAMPLE_RATE as f32)).exp(),
            count: 0,
        }
    }

    /// Limits `samples` in place; state carries across calls. Output sample `i` is input sample
    /// `i - LATENCY`.
    pub fn process(&mut self, samples: &mut [f32]) {
        for block in samples.chunks_mut(BLOCK) {
            let len = block.len();
            self.input[TAPS - 1..TAPS - 1 + len].copy_from_slice(block);
            interpolate(
                &self.phases,
                &self.input[..TAPS - 1 + len],
                &mut self.peaks[..len],
            );
            for (j, s) in block.iter_mut().enumerate() {
                *s = self.step(*s, self.peaks[j]);
            }
            self.input.copy_within(len..len + TAPS - 1, 0);
        }
    }

    /// Takes input `x`, whose window peaks at `peak`, and returns the input `LATENCY` samples
    /// back at the gain it needs.
    fn step(&mut self, x: f32, peak: f32) -> f32 {
        // The middle sample touches the stretches before and after it.
        let need = (self.limit / peak.max(self.last_peak).max(f32::MIN_POSITIVE)).min(1.0);
        self.last_peak = peak;

        let wrap = |i: usize| i & (NEEDED - 1);
        while self.needed_len > 0
            && self.needed[wrap(self.needed_head + self.needed_len - 1)].1 >= need
        {
            self.needed_len -= 1;
        }
        self.needed[wrap(self.needed_head + self.needed_len)] = (self.count, need);
        self.needed_len += 1;
        while self.needed[self.needed_head].0 + (LOOKAHEAD as u64) < self.count {
            self.needed_head = wrap(self.needed_head + 1);
            self.needed_len -= 1;
        }
        let hold = self.needed[self.needed_head].1;
        self.released = hold.min(self.released + self.release * (1.0 - self.released));

        let slot = (self.count % LOOKAHEAD as u64) as usize;
        self.held_sum += f64::from(self.released) - f64::from(self.held[slot]);
        self.held[slot] = self.released;
        let gain = (self.held_sum / LOOKAHEAD as f64) as f32;

        let slot = (self.count % LATENCY as u64) as usize;
        let out = std::mem::replace(&mut self.delay[slot], x) * gain;
        self.count += 1;
        out
    }
}

/// Fills `peaks`: for sample `j`, whose window is `input[j..j + TAPS]`, the largest of the
/// window's two middle samples and the values interpolated between them.
fn interpolate(phases: &[[f32; TAPS]; PHASES], input: &[f32], peaks: &mut [f32]) {
    assert_eq!(
        input.len(),
        peaks.len() + TAPS - 1,
        "a window for every peak"
    );
    #[cfg(target_arch = "aarch64")]
    let done = {
        let (lanes, _) = peaks.as_chunks_mut::<LANES>();
        for (c, out) in lanes.iter_mut().enumerate() {
            let window = input[c * LANES..][..TAPS + LANES - 1]
                .try_into()
                .expect("a window for every lane");
            neon::lanes(phases, window, out);
        }
        lanes.len() * LANES
    };
    #[cfg(not(target_arch = "aarch64"))]
    let done = 0;
    for (j, peak) in peaks.iter_mut().enumerate().skip(done) {
        let window = &input[j..j + TAPS];
        let middle = window[TAPS / 2 - 1].abs().max(window[TAPS / 2].abs());
        *peak = phases.iter().fold(middle, |peak, phase| {
            let value = window
                .iter()
                .zip(phase)
                .fold(0f32, |sum, (x, w)| sum + x * w);
            peak.max(value.abs())
        });
    }
}

#[cfg(target_arch = "aarch64")]
mod neon {
    use std::arch::aarch64::*;

    use super::{LANES, PHASES, TAPS};

    /// [`super::interpolate`] for `LANES` peaks side by side, every phase in registers. Each
    /// value is summed in tap order without fused multiply-adds, as the scalar path does.
    pub(super) fn lanes(
        phases: &[[f32; TAPS]; PHASES],
        window: &[f32; TAPS + LANES - 1],
        out: &mut [f32; LANES],
    ) {
        let x = window.as_ptr();
        // SAFETY: NEON is part of every aarch64 target. Each load reads 4 samples from at most
        // `TAPS - 1 + 4`, so the last one read is the window's last, `TAPS + LANES - 2`, and the
        // stores write the `LANES` outputs.
        unsafe {
            let mut acc = [[vdupq_n_f32(0.0); 2]; PHASES];
            for k in 0..TAPS {
                let lo = vld1q_f32(x.add(k));
                let hi = vld1q_f32(x.add(k + 4));
                for (acc, phase) in acc.iter_mut().zip(phases) {
                    acc[0] = vaddq_f32(acc[0], vmulq_n_f32(lo, phase[k]));
                    acc[1] = vaddq_f32(acc[1], vmulq_n_f32(hi, phase[k]));
                }
            }
            for half in 0..2 {
                let at = half * 4;
                let mut peak = vmaxq_f32(
                    vabsq_f32(vld1q_f32(x.add(at + TAPS / 2 - 1))),
                    vabsq_f32(vld1q_f32(x.add(at + TAPS / 2))),
                );
                for acc in &acc {
                    peak = vmaxq_f32(peak, vabsq_f32(acc[half]));
                }
                vst1q_f32(out.as_mut_ptr().add(at), peak);
            }
        }
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
