//! Feed-forward compressor with an RMS detector and a soft knee.

use crate::SAMPLE_RATE;

const THRESHOLD_DB: f32 = -24.0;
const RATIO: f32 = 3.0;
const KNEE_DB: f32 = 6.0;
const ATTACK_S: f32 = 0.005;
const RELEASE_S: f32 = 0.120;
const DETECTOR_S: f32 = 0.005;
/// The gain is computed once per block and interpolated, which saves a log and exp per sample.
const BLOCK: usize = 16;

pub(crate) struct Compressor {
    power: f32,
    reduction_db: f32,
    gain: f32,
    gain_step: f32,
    pos: usize,
    detector_coef: f32,
    attack_coef: f32,
    release_coef: f32,
}

fn one_pole(seconds: f32, samples: usize) -> f32 {
    1.0 - (-(samples as f32) / (seconds * SAMPLE_RATE as f32)).exp()
}

impl Compressor {
    pub(crate) fn new() -> Self {
        Self {
            power: 0.0,
            reduction_db: 0.0,
            gain: 1.0,
            gain_step: 0.0,
            pos: 0,
            detector_coef: one_pole(DETECTOR_S, 1),
            attack_coef: one_pole(ATTACK_S, BLOCK),
            release_coef: one_pole(RELEASE_S, BLOCK),
        }
    }

    pub(crate) fn process(&mut self, samples: &mut [f32]) {
        for s in samples {
            if self.pos == BLOCK {
                self.retarget();
                self.pos = 0;
            }
            self.pos += 1;
            self.power += self.detector_coef * (*s * *s - self.power);
            self.gain += self.gain_step;
            *s *= self.gain;
        }
    }

    fn retarget(&mut self) {
        let over = 10.0 * (self.power + 1e-12).log10() - THRESHOLD_DB;
        let slope = 1.0 - 1.0 / RATIO;
        let target = if 2.0 * over < -KNEE_DB {
            0.0
        } else if 2.0 * over.abs() <= KNEE_DB {
            -slope * (over + KNEE_DB / 2.0).powi(2) / (2.0 * KNEE_DB)
        } else {
            -slope * over
        };
        let coef = if target < self.reduction_db {
            self.attack_coef
        } else {
            self.release_coef
        };
        self.reduction_db += coef * (target - self.reduction_db);
        let next = (self.reduction_db * std::f32::consts::LN_10 / 20.0).exp();
        self.gain_step = (next - self.gain) / BLOCK as f32;
    }
}
