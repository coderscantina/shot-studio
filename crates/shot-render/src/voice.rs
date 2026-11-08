//! The finished sound of an output: the cut's voice through the voice chain, its [`Score`] of
//! music ducked under it and sound effects over it, and the whole at -14 LUFS.
//!
//! Loudness needs the whole cut, so the chain and score run once into memory ([`Mixed`]) and are
//! measured there. Trial passes over those samples apply the gain to the target and the
//! true-peak limiter, and correct the gain for what the limiter took off; the first trial on
//! target is the result. A 17 min cut holds about 180 MB of samples twice over while it
//! finishes.

use std::ops::Range;
use std::path::Path;
use std::sync::mpsc;

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
/// Trial passes at most, the last one kept whatever it measures, and how close to the target
/// they stop.
const TRIALS: usize = 4;
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

    /// The mix from output sample `at` on, as the whole output's mix plays it there except for
    /// the music's ducking, which starts released: start it a little early and drop the lead.
    pub fn mix_from(&self, at: u64) -> Mix {
        self.mix().starting_at(at)
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
}

/// Runs the passes over `audio` and hands the finished samples to `sink`, exactly
/// `audio.len()` of them.
pub fn finish<E: From<TimelineAudioError>>(
    audio: &mut TimelineAudio,
    score: &Score,
    mut sink: impl FnMut(&[f32]) -> Result<(), E>,
) -> Result<Loudness, E> {
    let mut mixed = Mixed::new(audio, score)?;
    let finished = mixed.finish(CEILING_DBTP);
    sink(&finished.samples)?;
    Ok(finished.loudness)
}

/// The voice of a cut through the chain with its score mixed in, whole and measured, so the
/// finish can run over it as often as the gain search and the encoder's peaks need.
pub struct Mixed {
    samples: Vec<f32>,
    before: Option<f64>,
    /// Where the next gain search starts: the gain the last one ended on.
    gain_db: f64,
}

/// Finished samples, one per output sample, and their loudness.
pub struct Finished {
    pub samples: Vec<f32>,
    pub loudness: Loudness,
}

impl Mixed {
    /// One pass of the chain and score over the whole output of `audio`.
    pub fn new(audio: &mut TimelineAudio, score: &Score) -> Result<Self, TimelineAudioError> {
        let mut samples = Vec::with_capacity(audio.len() as usize);
        let ((), before) = measuring(|measure| {
            pass(audio, score, |block| {
                samples.extend_from_slice(block);
                measure(block);
                Ok(())
            })
        })?;
        Ok(Self {
            samples,
            before,
            gain_db: before.map_or(0.0, |measured| TARGET_LUFS - measured),
        })
    }

    /// The samples at the target loudness with true peaks under `ceiling_dbtp` less the codec's
    /// headroom. A lower ceiling than [`CEILING_DBTP`] is for an encode that overshot it.
    pub fn finish(&mut self, ceiling_dbtp: f64) -> Finished {
        let mut out = Vec::with_capacity(self.samples.len());
        let Some(before) = self.before else {
            let after = self.limited(self.gain_db, ceiling_dbtp, &mut out);
            return Finished {
                samples: out,
                loudness: Loudness {
                    before: None,
                    after,
                },
            };
        };
        // The limiter takes some loudness off the peaks it holds, so trials raise the gain
        // until the finished audio meets the target. Loud sound effects make the limiter take
        // more the higher the gain, so each step follows the slope the last two trials measured.
        let mut last = (0.0, before);
        let mut after = None;
        for trial in 0..TRIALS {
            after = self.limited(self.gain_db, ceiling_dbtp, &mut out);
            let Some(measured) = after else {
                break;
            };
            if (TARGET_LUFS - measured).abs() <= TOLERANCE_LU || trial == TRIALS - 1 {
                break;
            }
            let slope = (measured - last.1) / (self.gain_db - last.0);
            let slope = if slope.is_finite() {
                slope.clamp(0.25, 1.0)
            } else {
                1.0
            };
            last = (self.gain_db, measured);
            self.gain_db += (TARGET_LUFS - measured) / slope;
        }
        Finished {
            samples: out,
            loudness: Loudness {
                before: Some(before),
                after,
            },
        }
    }

    /// The samples through [`Finish`] at `gain_db` and `ceiling_dbtp` into `out`, with the
    /// limiter's latency taken out, and their loudness.
    fn limited(&self, gain_db: f64, ceiling_dbtp: f64, out: &mut Vec<f32>) -> Option<f64> {
        out.clear();
        out.extend_from_slice(&self.samples);
        let mut finish = Finish::new(gain_db, ceiling_dbtp);
        let mut tail = Vec::with_capacity(Finish::LATENCY);
        let Ok(((), loudness)) = measuring(|measure| {
            let mut skip = Finish::LATENCY;
            for block in out.chunks_mut(BLOCK as usize) {
                finish.process(block);
                let drop = skip.min(block.len());
                skip -= drop;
                measure(&block[drop..]);
            }
            finish.flush(&mut tail);
            measure(&tail[skip..]);
            Ok::<_, std::convert::Infallible>(())
        });
        out.extend_from_slice(&tail);
        out.drain(..Finish::LATENCY);
        loudness
    }
}

/// Runs `produce`, which hands the samples of a stream to its argument block by block, and
/// measures their integrated loudness on another thread meanwhile.
fn measuring<T, E>(
    produce: impl FnOnce(&mut dyn FnMut(&[f32])) -> Result<T, E>,
) -> Result<(T, Option<f64>), E> {
    std::thread::scope(|scope| {
        let (blocks, incoming) = mpsc::sync_channel::<Vec<f32>>(32);
        let meter = scope.spawn(move || {
            let mut meter = Meter::loudness_only();
            for block in incoming {
                meter.add(&block);
            }
            meter.loudness()
        });
        let produced = produce(&mut |block| {
            // The meter only stops early by panicking, which the join below reports.
            let _ = blocks.send(block.to_vec());
        });
        drop(blocks);
        let loudness = meter.join().expect("loudness meter panicked");
        produced.map(|value| (value, loudness))
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioSegment;
    use shot_core::Id;

    /// A mono 16-bit WAV of `samples` at 48 kHz.
    fn wav(path: &Path, samples: &[f32]) {
        let data = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        let mut put = |b: &[u8]| bytes.extend_from_slice(b);
        put(b"RIFF");
        put(&(36 + data).to_le_bytes());
        put(b"WAVEfmt ");
        put(&16u32.to_le_bytes());
        put(&1u16.to_le_bytes());
        put(&1u16.to_le_bytes());
        put(&SAMPLE_RATE.to_le_bytes());
        put(&(SAMPLE_RATE * 2).to_le_bytes());
        put(&2u16.to_le_bytes());
        put(&16u16.to_le_bytes());
        put(b"data");
        put(&data.to_le_bytes());
        for s in samples {
            put(&((s * f32::from(i16::MAX)) as i16).to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn finishing_again_lower_keeps_the_target_and_lowers_the_peaks() {
        // Syllables of a few tones with a loud click every half second, so the limiter works.
        let samples: Vec<f32> = (0..5 * SAMPLE_RATE as usize)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                let tones: f32 = [(140.0, 0.3), (420.0, 0.2), (1300.0, 0.1)]
                    .iter()
                    .map(|(freq, amp)| amp * (std::f32::consts::TAU * freq * t).sin())
                    .sum();
                let syllable = (std::f32::consts::TAU * 2.5 * t).sin().powi(2);
                if i % 24_000 < 48 {
                    0.95
                } else {
                    tones * syllable
                }
            })
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let mic = dir.path().join("mic.wav");
        wav(&mic, &samples);
        let mut audio = TimelineAudio::new(
            [AudioSegment {
                take: Id::new("t01").unwrap(),
                source_frames: 0..300,
            }],
            60,
            move |_| mic.clone(),
        );
        let mut mixed = Mixed::new(&mut audio, &Score::default()).unwrap();
        let peak = |finished: &Finished| {
            let mut meter = Meter::new();
            meter.add(&finished.samples);
            meter.true_peak()
        };
        let first = mixed.finish(CEILING_DBTP);
        let lower = mixed.finish(CEILING_DBTP - 1.0);
        for finished in [&first, &lower] {
            assert_eq!(finished.samples.len() as u64, audio.len());
            let after = finished.loudness.after.unwrap();
            assert!((after - TARGET_LUFS).abs() <= 0.5, "{after} LUFS");
        }
        let (first, lower) = (peak(&first), peak(&lower));
        assert!(lower <= first - 0.9, "{first} dBTP, then {lower} dBTP");
    }
}
