//! The finished sound of an output: the cut's voice through the voice chain, its [`Score`] of
//! music ducked under it and sound effects over it, and the whole at -14 LUFS.
//!
//! Loudness needs the whole cut, so the audio runs more than once. Pass 1 measures the chain and
//! score. Trial passes repeat them with the gain to the target and the true-peak limiter, and
//! correct the gain for what the limiter took off. The last pass hands the samples on. Every
//! stage is deterministic, so every pass sees the same signal.

use std::ops::Range;
use std::path::Path;

use shot_audio::{
    CEILING_DBTP, CROSSFADE, Finish, Meter, Mix, Music, SfxCue, TARGET_LUFS, VoiceChain,
};
use shot_media::audio::{AudioError, PcmReader, SAMPLE_RATE};

use crate::audio::{TimelineAudio, TimelineAudioError};

/// Samples read and processed at a time, 0.25 s.
const BLOCK: u64 = SAMPLE_RATE as u64 / 4;
/// A music file is read until a block of this many samples is digital silence.
const MUSIC_END: usize = 10 * SAMPLE_RATE as usize;
/// The longest music file read, 30 min.
const MUSIC_MAX: usize = 30 * 60 * SAMPLE_RATE as usize;
/// Trial passes at most, and how close to the target they stop.
const TRIALS: usize = 3;
const TOLERANCE_LU: f64 = 0.1;

/// What plays with the voice, in output samples: the music cues, the sound effects and the
/// bumper ranges the music ducks under. Each pass mixes it afresh.
#[derive(Debug, Clone)]
pub struct Score {
    pub music: Music,
    pub sfx: Vec<SfxCue>,
    pub bumpers: Vec<Range<u64>>,
}

impl Default for Score {
    fn default() -> Self {
        Self {
            music: Music::new(Vec::new(), CROSSFADE),
            sfx: Vec::new(),
            bumpers: Vec::new(),
        }
    }
}

impl Score {
    fn mix(&self) -> Mix {
        Mix::new(self.music.clone(), self.sfx.clone(), self.bumpers.clone())
    }
}

/// Reads a sound or music file as mono 48 kHz samples. AVFoundation reads past the end as
/// silence, so the file ends where 10 s of digital silence start, and trailing silence is
/// trimmed so a loop is seamless.
pub fn read_samples(path: &Path) -> Result<Vec<f32>, AudioError> {
    let mut reader = PcmReader::open(path)?;
    let mut samples = Vec::new();
    while samples.len() < MUSIC_MAX {
        let at = samples.len();
        reader.read(at as u64, (at + MUSIC_END) as u64, &mut samples)?;
        if samples[at..].iter().all(|s| *s == 0.0) {
            break;
        }
    }
    let end = samples.iter().rposition(|s| *s != 0.0).map_or(0, |i| i + 1);
    samples.truncate(end);
    Ok(samples)
}

/// Loudness of the finished audio, as the last pass measured it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated LUFS before and after the gain, `None` for a silent cut.
    pub before: Option<f64>,
    pub after: Option<f64>,
    pub true_peak: f64,
}

/// Runs the passes over `audio` and hands the finished samples to `sink` in output order,
/// exactly `audio.len()` of them.
pub fn finish<E: From<TimelineAudioError>>(
    audio: &mut TimelineAudio,
    score: &Score,
    mut sink: impl FnMut(&[f32]) -> Result<(), E>,
) -> Result<Loudness, E> {
    let mut meter = Meter::new();
    pass(audio, score, |samples| {
        meter.add(samples);
        Ok::<_, E>(())
    })?;
    let before = meter.loudness();
    let mut gain_db = before.map_or(0.0, |measured| TARGET_LUFS - measured);
    // The limiter takes some loudness off the peaks it holds, so trial passes raise the gain
    // until the finished audio meets the target. Loud sound effects make the limiter take more
    // the higher the gain, so each step follows the slope the last two trials measured.
    if let Some(mut last) = before.map(|b| (0.0, b)) {
        for _ in 0..TRIALS {
            let meter = finished(audio, score, gain_db, |_| Ok::<_, E>(()))?;
            let Some(after) = meter.loudness() else {
                break;
            };
            if (TARGET_LUFS - after).abs() <= TOLERANCE_LU {
                break;
            }
            let slope = (after - last.1) / (gain_db - last.0);
            let slope = if slope.is_finite() {
                slope.clamp(0.25, 1.0)
            } else {
                1.0
            };
            last = (gain_db, after);
            gain_db += (TARGET_LUFS - after) / slope;
        }
    }
    let meter = finished(audio, score, gain_db, &mut sink)?;
    Ok(Loudness {
        before,
        after: meter.loudness(),
        true_peak: meter.true_peak(),
    })
}

/// A pass through [`Finish`] at `gain_db`, measured, with the limiter's latency taken out.
fn finished<E: From<TimelineAudioError>>(
    audio: &mut TimelineAudio,
    score: &Score,
    gain_db: f64,
    mut sink: impl FnMut(&[f32]) -> Result<(), E>,
) -> Result<Meter, E> {
    let mut finish = Finish::new(gain_db, CEILING_DBTP);
    let mut meter = Meter::new();
    let mut skip = Finish::LATENCY;
    let mut out = |samples: &[f32]| {
        let drop = skip.min(samples.len());
        skip -= drop;
        meter.add(&samples[drop..]);
        sink(&samples[drop..])
    };
    pass(audio, score, |samples| {
        finish.process(samples);
        out(samples)
    })?;
    let mut tail = Vec::with_capacity(Finish::LATENCY);
    finish.flush(&mut tail);
    out(&tail)?;
    Ok(meter)
}

/// One pass of the chain and score over the whole output, aligned for the chain's latency.
fn pass<E: From<TimelineAudioError>>(
    audio: &mut TimelineAudio,
    score: &Score,
    mut out: impl FnMut(&mut [f32]) -> Result<(), E>,
) -> Result<(), E> {
    let mut chain = VoiceChain::new();
    let mut mix = score.mix();
    let mut skip = VoiceChain::LATENCY;
    let bleeps = audio.bleeps().to_vec();
    let mut emitted = 0u64;
    let mut emit = |samples: &mut [f32]| -> Result<(), E> {
        let drop = skip.min(samples.len());
        skip -= drop;
        let samples = &mut samples[drop..];
        if samples.is_empty() {
            return Ok(());
        }
        let at = emitted;
        emitted += samples.len() as u64;
        for bleep in &bleeps {
            let from = bleep.start.max(at);
            let to = bleep.end.min(emitted);
            for n in from..to {
                samples[(n - at) as usize] =
                    crate::overlay::redact::tone(n, n - bleep.start, bleep.end - bleep.start);
            }
        }
        mix.mix(samples);
        out(samples)
    };
    let total = audio.len();
    let mut block = Vec::with_capacity(BLOCK as usize);
    let mut at = 0;
    while at < total {
        let end = (at + BLOCK).min(total);
        block.clear();
        audio.read(at..end, &mut block)?;
        chain.process(&mut block);
        emit(&mut block)?;
        at = end;
    }
    block.clear();
    chain.flush(&mut block);
    emit(&mut block)
}
