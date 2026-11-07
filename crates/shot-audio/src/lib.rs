//! Voice chain, ducking and loudness.
//!
//! Export renders a cut's voice as one 48 kHz mono stream and runs it twice: pass 1 measures the
//! [`VoiceChain`] output with a [`Meter`], pass 2 repeats the chain and applies [`Finish`].
//! Every stage is deterministic and streaming: the output does not depend on how the input is
//! split into pieces.

mod compressor;
mod denoise;
mod ducking;
mod eq;
mod loudness;

use compressor::Compressor;
use denoise::Denoiser;
use eq::Eq;

pub use ducking::MusicBed;
pub use loudness::{CEILING_DBTP, Finish, Meter, TARGET_LUFS};

pub const SAMPLE_RATE: u32 = 48_000;

/// Denoise, EQ and compressor for one voice, streaming.
///
/// The spectral denoiser needs [`VoiceChain::LATENCY`] samples of lookahead, so the output runs
/// that many samples behind the input: the first `LATENCY` output samples are lead-in to drop,
/// and [`VoiceChain::flush`] returns the last `LATENCY` samples after the final piece.
pub struct VoiceChain {
    denoiser: Denoiser,
    eq: Eq,
    compressor: Compressor,
}

impl VoiceChain {
    /// Samples the output is delayed against the input.
    pub const LATENCY: usize = denoise::LATENCY;

    pub fn new() -> Self {
        Self {
            denoiser: Denoiser::new(),
            eq: Eq::new(),
            compressor: Compressor::new(),
        }
    }

    /// Processes `samples` in place; state carries across calls. Output sample `i` is input
    /// sample `i - LATENCY`.
    pub fn process(&mut self, samples: &mut [f32]) {
        self.denoiser.process(samples);
        self.eq.process(samples);
        self.compressor.process(samples);
    }

    /// Appends the final `LATENCY` samples of the stream to `out`.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let start = out.len();
        out.resize(start + Self::LATENCY, 0.0);
        self.process(&mut out[start..]);
    }
}

impl Default for VoiceChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    struct Rng(u32);

    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0
        }

        /// Uniform in [-1, 1).
        fn unit(&mut self) -> f32 {
            (self.next() >> 8) as f32 / (1 << 23) as f32 - 1.0
        }
    }

    fn sine(freq: f32, amp: f32, range: std::ops::Range<usize>, out: &mut [f32]) {
        for i in range {
            out[i] += amp * (TAU * freq * i as f32 / SAMPLE_RATE as f32).sin();
        }
    }

    fn rms_db(samples: &[f32]) -> f32 {
        let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
        10.0 * power.max(1e-20).log10()
    }

    fn secs(s: f32) -> usize {
        (s * SAMPLE_RATE as f32) as usize
    }

    /// Speech-like test voice: syllable-modulated tones over a noise floor.
    fn voice(len: usize, noise: f32, seed: u32) -> Vec<f32> {
        let mut rng = Rng(seed);
        let mut out: Vec<f32> = (0..len).map(|_| noise * rng.unit()).collect();
        let mut tones = vec![0.0; len];
        for (freq, amp) in [(140.0, 0.3), (420.0, 0.2), (1300.0, 0.12), (3100.0, 0.05)] {
            sine(freq, amp, 0..len, &mut tones);
        }
        for (i, (o, t)) in out.iter_mut().zip(&tones).enumerate() {
            let t_s = i as f32 / SAMPLE_RATE as f32;
            *o += t * (TAU * 2.5 * t_s).sin().abs().powi(2);
        }
        out
    }

    fn in_pieces(rng: &mut Rng, data: &mut [f32], mut f: impl FnMut(&mut [f32])) {
        let mut rest = data;
        while !rest.is_empty() {
            let n = (1 + rng.next() as usize % 5000).min(rest.len());
            let (head, tail) = rest.split_at_mut(n);
            f(head);
            rest = tail;
        }
    }

    #[test]
    fn pieces_equal_one_call() {
        let input = voice(secs(3.0), 0.01, 7);
        let mut rng = Rng(99);

        let mut whole = input.clone();
        VoiceChain::new().process(&mut whole);
        let mut split = input.clone();
        let mut chain = VoiceChain::new();
        in_pieces(&mut rng, &mut split, |p| chain.process(p));
        assert_eq!(whole, split);

        let music: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let mut whole = input.clone();
        MusicBed::new(music.clone(), -12.0, -24.0).mix(&mut whole);
        let mut split = input.clone();
        let mut bed = MusicBed::new(music, -12.0, -24.0);
        in_pieces(&mut rng, &mut split, |p| bed.mix(p));
        assert_eq!(whole, split);

        let mut whole = input.clone();
        Finish::new(-30.0, TARGET_LUFS, CEILING_DBTP).process(&mut whole);
        let mut split = input;
        let mut finish = Finish::new(-30.0, TARGET_LUFS, CEILING_DBTP);
        in_pieces(&mut rng, &mut split, |p| finish.process(p));
        assert_eq!(whole, split);
    }

    #[test]
    fn denoiser_cuts_noise_and_keeps_tone() {
        let len = secs(8.0);
        let mut input = vec![0.0; len];
        let mut rng = Rng(5);
        for s in &mut input {
            *s = 0.003 * rng.unit();
        }
        for k in 0..8 {
            let start = secs(k as f32 + 0.3);
            sine(1000.0, 0.2, start..start + secs(0.4), &mut input);
        }
        let mut out = input.clone();
        denoise::Denoiser::new().process(&mut out);

        let lag = VoiceChain::LATENCY;
        let gap = secs(6.8)..secs(7.2);
        let reduction = rms_db(&input[gap.clone()]) - rms_db(&out[gap.start + lag..gap.end + lag]);
        assert!(reduction >= 10.0, "noise reduced by {reduction} dB");

        let burst = secs(7.35)..secs(7.65);
        let change = rms_db(&out[burst.start + lag..burst.end + lag]) - rms_db(&input[burst]);
        assert!(change.abs() <= 1.0, "tone changed by {change} dB");
    }

    #[test]
    fn compressor_narrows_loud_to_quiet() {
        let len = secs(2.0);
        let mut input = vec![0.0; len];
        sine(500.0, 0.5, 0..secs(1.0), &mut input);
        sine(500.0, 0.02, secs(1.0)..len, &mut input);
        let mut out = input.clone();
        compressor::Compressor::new().process(&mut out);

        let loud = secs(0.5)..secs(1.0);
        let quiet = secs(1.5)..len;
        let before = rms_db(&input[loud.clone()]) - rms_db(&input[quiet.clone()]);
        let after = rms_db(&out[loud]) - rms_db(&out[quiet]);
        assert!(
            after < before - 6.0,
            "difference {before} dB became {after} dB"
        );
    }

    #[test]
    fn bed_ducks_under_voice() {
        let len = secs(8.0);
        let mut music = vec![0.0; 48_000];
        sine(220.0, 0.3, 0..48_000, &mut music);
        let music_db = rms_db(&music);
        let mut voice = vec![0.0; len];
        sine(500.0, 0.1, secs(3.0)..secs(6.0), &mut voice);
        let mut mixed = voice.clone();
        MusicBed::new(music, -12.0, -24.0).mix(&mut mixed);
        let bed: Vec<f32> = mixed.iter().zip(&voice).map(|(m, v)| m - v).collect();

        let before = rms_db(&bed[secs(1.0)..secs(2.5)]) - music_db;
        let during = rms_db(&bed[secs(4.5)..secs(5.9)]) - music_db;
        assert!(
            (before + 12.0).abs() < 0.5,
            "bed away from voice at {before} dB"
        );
        assert!(
            (during + 24.0).abs() < 1.0,
            "bed under voice at {during} dB"
        );
    }

    #[test]
    fn finish_hits_the_loudness_target() {
        let mut signal = voice(secs(20.0), 0.01, 11);
        let mut meter = Meter::new();
        meter.add(&signal);
        let gain = 10f32.powf((-30.0 - meter.loudness().unwrap() as f32) / 20.0);
        signal.iter_mut().for_each(|s| *s *= gain);

        let mut meter = Meter::new();
        meter.add(&signal);
        let measured = meter.loudness().unwrap();
        assert!((measured + 30.0).abs() < 0.1);

        Finish::new(measured, TARGET_LUFS, CEILING_DBTP).process(&mut signal);
        let mut meter = Meter::new();
        meter.add(&signal);
        let loudness = meter.loudness().unwrap();
        assert!((loudness - TARGET_LUFS).abs() <= 0.5, "loudness {loudness}");
        assert!(
            meter.true_peak() <= CEILING_DBTP,
            "true peak {}",
            meter.true_peak()
        );
    }

    /// Release-mode speed check: `cargo test -p shot-audio --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn speed_of_17_minutes() {
        let piece_len = 4096;
        let pieces = 17 * 60 * SAMPLE_RATE as usize / piece_len;
        let template = voice(piece_len, 0.01, 3);
        let music = vec![0.1; 10 * SAMPLE_RATE as usize];
        let mut chain = VoiceChain::new();
        let mut bed = MusicBed::new(music, -18.0, -28.0);
        let mut finish = Finish::new(-24.0, TARGET_LUFS, CEILING_DBTP);
        let mut meter = Meter::new();
        let mut piece = template.clone();
        let start = std::time::Instant::now();
        for _ in 0..pieces {
            piece.copy_from_slice(&template);
            chain.process(&mut piece);
            bed.mix(&mut piece);
            meter.add(&piece);
            finish.process(&mut piece);
        }
        println!("17 min of audio in {:?}", start.elapsed());
    }
}
