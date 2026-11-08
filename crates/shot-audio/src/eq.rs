//! Voice EQ: high-pass, low-mid cut and presence boost as RBJ biquads.

use crate::SAMPLE_RATE;
use std::f64::consts::TAU;

struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b: [b[0] / a[0], b[1] / a[0], b[2] / a[0]],
            a: [a[1] / a[0], a[2] / a[0]],
            z: [0.0; 2],
        }
    }

    fn high_pass(freq: f64, q: f64) -> Self {
        let (sin, cos) = (TAU * freq / SAMPLE_RATE as f64).sin_cos();
        let alpha = sin / (2.0 * q);
        Self::new(
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    fn peaking(freq: f64, q: f64, gain_db: f64) -> Self {
        let (sin, cos) = (TAU * freq / SAMPLE_RATE as f64).sin_cos();
        let alpha = sin / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);
        Self::new(
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        )
    }

    fn step(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y as f32
    }
}

pub(crate) struct Eq([Biquad; 3]);

impl Eq {
    pub(crate) fn new() -> Self {
        Self([
            Biquad::high_pass(80.0, std::f64::consts::FRAC_1_SQRT_2),
            Biquad::peaking(300.0, 0.7, -2.0),
            Biquad::peaking(4000.0, 0.8, 2.5),
        ])
    }

    /// Runs the stages one sample at a time, so the processor overlaps their dependency chains.
    pub(crate) fn process(&mut self, samples: &mut [f32]) {
        let [first, second, third] = &mut self.0;
        for s in samples {
            *s = third.step(second.step(first.step(*s)));
        }
    }
}
