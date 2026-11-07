//! Spectral denoiser: STFT with sqrt-Hann windows, per-bin noise floor by minimum statistics,
//! Wiener-like gain with a floor and temporal smoothing.

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::f32::consts::TAU;
use std::sync::Arc;

const FRAME: usize = 1024;
const HOP: usize = FRAME / 4;
const BINS: usize = FRAME / 2 + 1;
/// Output delay in samples: a frame of lookahead plus the hop the output is buffered by.
pub(crate) const LATENCY: usize = FRAME;

/// Gain floor, -15 dB.
const FLOOR: f32 = 0.177_8;
/// Smoothing of the bin power across frames.
const POWER_SMOOTHING: f32 = 0.15;
/// Share of a falling gain that is kept per frame.
const GAIN_RELEASE: f32 = 0.75;
/// Minimum statistics: about 1.5 s as 6 sub-windows of 47 frames.
const SUB_WINDOWS: usize = 6;
const SUB_FRAMES: usize = 47;
/// Scales the minimum of the smoothed power up to the mean noise power, with some oversubtraction.
const NOISE_SCALE: f32 = 5.0;
/// Keeps digital silence and clean speech gaps from dividing by zero.
const NOISE_EPSILON: f32 = 1e-9;

pub(crate) struct Denoiser {
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    ifft_scratch: Vec<Complex<f32>>,
    window: Vec<f32>,
    time: Vec<f32>,
    spectrum: Vec<Complex<f32>>,

    input: Vec<f32>,
    filled: usize,
    overlap: Vec<f32>,
    ready: Vec<f32>,
    ready_pos: usize,

    power: Vec<f32>,
    gain: Vec<f32>,
    sub_min: Vec<f32>,
    sub_index: usize,
    sub_frames: usize,
    current_min: Vec<f32>,
    past_min: Vec<f32>,
    primed: bool,
}

impl Denoiser {
    pub(crate) fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FRAME);
        let ifft = planner.plan_fft_inverse(FRAME);
        let window = (0..FRAME)
            .map(|i| (0.5 * (1.0 - (TAU * i as f32 / FRAME as f32).cos())).sqrt())
            .collect();
        Self {
            fft_scratch: fft.make_scratch_vec(),
            ifft_scratch: ifft.make_scratch_vec(),
            spectrum: fft.make_output_vec(),
            fft,
            ifft,
            window,
            time: vec![0.0; FRAME],
            input: vec![0.0; FRAME],
            filled: FRAME - HOP,
            overlap: vec![0.0; FRAME],
            ready: vec![0.0; HOP],
            ready_pos: 0,
            power: vec![0.0; BINS],
            gain: vec![1.0; BINS],
            sub_min: vec![f32::INFINITY; SUB_WINDOWS * BINS],
            sub_index: 0,
            sub_frames: 0,
            current_min: vec![f32::INFINITY; BINS],
            past_min: vec![f32::INFINITY; BINS],
            primed: false,
        }
    }

    /// Replaces `samples` with the denoised stream, delayed by [`LATENCY`].
    pub(crate) fn process(&mut self, samples: &mut [f32]) {
        for s in samples {
            let out = self.ready[self.ready_pos];
            self.ready_pos += 1;
            self.input[self.filled] = *s;
            self.filled += 1;
            if self.filled == FRAME {
                self.run_frame();
                self.input.copy_within(HOP.., 0);
                self.filled = FRAME - HOP;
                self.ready_pos = 0;
            }
            *s = out;
        }
    }

    fn run_frame(&mut self) {
        for ((t, x), w) in self.time.iter_mut().zip(&self.input).zip(&self.window) {
            *t = x * w;
        }
        self.fft
            .process_with_scratch(&mut self.time, &mut self.spectrum, &mut self.fft_scratch)
            .expect("buffers have the planned lengths");

        self.update_gains();

        self.ifft
            .process_with_scratch(&mut self.spectrum, &mut self.time, &mut self.ifft_scratch)
            .expect("buffers have the planned lengths");

        // Unnormalized inverse (FRAME) times the overlap-added window energy (2).
        let scale = 1.0 / (2 * FRAME) as f32;
        for ((o, t), w) in self.overlap.iter_mut().zip(&self.time).zip(&self.window) {
            *o += t * w * scale;
        }
        self.ready.copy_from_slice(&self.overlap[..HOP]);
        self.overlap.copy_within(HOP.., 0);
        self.overlap[FRAME - HOP..].fill(0.0);
    }

    fn update_gains(&mut self) {
        let primed = std::mem::replace(&mut self.primed, true);
        for k in 0..BINS {
            let c = self.spectrum[k];
            let p = c.re * c.re + c.im * c.im;
            let s = if primed {
                self.power[k] + POWER_SMOOTHING * (p - self.power[k])
            } else {
                p
            };
            self.power[k] = s;
            let m = self.current_min[k].min(s);
            self.current_min[k] = m;
            let noise = m.min(self.past_min[k]) * NOISE_SCALE + NOISE_EPSILON;

            let target = if s > noise {
                (1.0 - noise / s).max(FLOOR)
            } else {
                FLOOR
            };
            let previous = self.gain[k];
            let g = if target >= previous {
                target
            } else {
                target + (previous - target) * GAIN_RELEASE
            };
            self.gain[k] = g;
            self.spectrum[k] = c * g;
        }

        self.sub_frames += 1;
        if self.sub_frames == SUB_FRAMES {
            self.sub_frames = 0;
            let row = self.sub_index * BINS;
            self.sub_min[row..row + BINS].copy_from_slice(&self.current_min);
            self.sub_index = (self.sub_index + 1) % SUB_WINDOWS;
            self.past_min.fill(f32::INFINITY);
            for rows in self.sub_min.as_chunks::<BINS>().0 {
                for (past, m) in self.past_min.iter_mut().zip(rows) {
                    *past = past.min(*m);
                }
            }
            self.current_min.fill(f32::INFINITY);
        }
    }
}
