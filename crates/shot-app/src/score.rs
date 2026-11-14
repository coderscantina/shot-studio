//! The preview's finished sound, made by export's [`Pass`]: the plan's voice
//! ([`shot_render::export::plan_audio`]) through the voice chain, bleeps over it, export's
//! [`Score`] of music, sound effects, the brand's sounds and the bumpers' own sound
//! ([`shot_render::export::preview_score`]), and the gain to the loudness target with the
//! true-peak limiter.
//!
//! Export runs the whole cut from its first sample. The preview starts anywhere, so it starts
//! the chain, the mix and the limiter a second early and drops that second: the compressor, the
//! ducking and a music cue's fade-in are settled where the sound is heard. The gain comes from
//! a pass over the whole cut on a thread of its own, export's first pass, and glides in when it
//! lands, so the sound does not jump.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use shot_audio::{CEILING_DBTP, CROSSFADE, Finish, Meter, TARGET_LUFS, VoiceChain};
use shot_media::audio::SAMPLE_RATE;
use shot_render::audio::{TimelineAudio, TimelineAudioError};
use shot_render::voice::{Pass, Score};

/// How early the chain starts before the first heard sample.
const PREROLL: u64 = CROSSFADE;
/// Samples processed per step of the gain pass.
const PASS_BLOCK: usize = SAMPLE_RATE as usize / 4;
/// How fast a new gain glides in: the share of the way left covered each sample, a time
/// constant of 50 ms.
const GLIDE: f32 = 1.0 / (SAMPLE_RATE as f32 / 20.0);

/// The finished sound from an output sample on, read piece by piece.
pub struct Finished {
    voice: Pass,
    /// The limiter alone: the gain to the target is [`Finished::gain`], so it can change.
    finish: Finish,
    /// Finished output still to drop: the limiter's lead-in, then the preroll.
    skip: usize,
    buffer: Vec<f32>,
    /// The linear gain applied now and the one it glides to.
    gain: f32,
    target: f32,
}

impl Finished {
    /// The finished sound from output sample `at`, at `gain_db` to the target.
    pub fn new(audio: &TimelineAudio, score: &Score, gain_db: f64, at: u64) -> Self {
        let start = at.saturating_sub(PREROLL);
        let gain = linear(gain_db);
        Self {
            voice: Pass::new(audio, score, start),
            finish: Finish::new(0.0, CEILING_DBTP),
            skip: Finish::LATENCY + (at - start) as usize,
            buffer: Vec::new(),
            gain,
            target: gain,
        }
    }

    /// Glides to `gain_db` to the target from the next sample read on.
    pub fn set_gain(&mut self, gain_db: f64) {
        self.target = linear(gain_db);
    }

    /// Appends the next `len` samples to `out`.
    pub fn read(
        &mut self,
        audio: &mut TimelineAudio,
        len: usize,
        out: &mut Vec<f32>,
    ) -> Result<(), TimelineAudioError> {
        while self.buffer.len() < len {
            let begin = self.buffer.len();
            self.voice.step(audio, len, &mut self.buffer)?;
            for sample in &mut self.buffer[begin..] {
                self.gain += (self.target - self.gain) * GLIDE;
                *sample *= self.gain;
            }
            self.finish.process(&mut self.buffer[begin..]);
            let drop = self.skip.min(self.buffer.len() - begin);
            self.skip -= drop;
            self.buffer.drain(begin..begin + drop);
        }
        out.extend(self.buffer.drain(..len));
        Ok(())
    }
}

fn linear(gain_db: f64) -> f32 {
    10f64.powf(gain_db / 20.0) as f32
}

/// The gain export's first pass gives the whole output: to the loudness target from what the
/// chain and mix measure. `None` when `cancel` moved past `generation` or the cut is silent.
pub fn gain(
    audio: &mut TimelineAudio,
    score: &Score,
    cancel: &Arc<AtomicU64>,
    generation: u64,
) -> Result<Option<f64>, TimelineAudioError> {
    let mut voice = Pass::new(audio, score, 0);
    let mut meter = Meter::new();
    let total = audio.len() + VoiceChain::LATENCY as u64;
    let mut block = Vec::with_capacity(PASS_BLOCK);
    while voice.read_at() < total {
        if cancel.load(Ordering::Relaxed) != generation {
            return Ok(None);
        }
        block.clear();
        let len = PASS_BLOCK.min((total - voice.read_at()) as usize);
        voice.step(audio, len, &mut block)?;
        meter.add(&block);
    }
    Ok(meter.loudness().map(|measured| TARGET_LUFS - measured))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_audio::sound::Sound;
    use shot_audio::{Anchor, Music, MusicCue, Play, SfxCue, Track};

    #[test]
    fn a_placed_sound_is_heard_where_it_plays_and_a_new_gain_glides_in_without_a_seek() {
        let pop = shot_audio::sound::bundled("pop").unwrap().decode();
        let at = u64::from(SAMPLE_RATE);
        let score = Score {
            sfx: vec![SfxCue::new(pop.clone(), Anchor::Start(at), 0.0)],
            ..Score::default()
        };
        let silent = TimelineAudio::new([], 60, |_| std::path::PathBuf::new());
        // Read in the feeder's blocks; `set` changes the gain half a second before the sound.
        let finished = |gain_db: f64, set: Option<f64>| {
            let mut audio = TimelineAudio::new([], 60, |_| std::path::PathBuf::new());
            let mut finished = Finished::new(&silent, &score, gain_db, 0);
            let mut out = Vec::new();
            while out.len() < 2 * at as usize {
                if out.len() == 23 * 1024
                    && let Some(gain_db) = set
                {
                    finished.set_gain(gain_db);
                }
                finished.read(&mut audio, 1024, &mut out).unwrap();
            }
            out
        };
        let peak = |samples: &[f32]| samples.iter().fold(0f32, |a, s| a.max(s.abs()));

        let heard = finished(0.0, None);
        let hit = at as usize..at as usize + pop.len();
        assert_eq!(peak(&heard[..hit.start]), 0.0, "silent before the sound");
        assert!(
            peak(&heard[hit.clone()]) > 0.1,
            "the sound plays at its place"
        );

        let quiet = finished(-20.0, None);
        let raised = finished(-20.0, Some(0.0));
        assert!(peak(&quiet[hit.clone()]) < 0.2 * peak(&heard[hit.clone()]));
        let off = heard[hit.clone()]
            .iter()
            .zip(&raised[hit])
            .fold(0f32, |a, (x, y)| a.max((x - y).abs()));
        assert!(
            off < 1e-3,
            "the new gain plays a second later, off by {off}"
        );
    }

    #[test]
    fn a_mix_started_late_matches_the_whole_mix_after_the_preroll() {
        let bed: Vec<f32> = (0..30_000)
            .map(|i| (i as f32 * 0.003).sin() * 0.2)
            .collect();
        let hit: Vec<f32> = (0..90_000)
            .map(|i| ((i % 97) as f32 - 48.0) / 200.0)
            .collect();
        let cue = |at| MusicCue {
            at,
            play: Some(Play {
                track: Track::new(
                    Sound::new(bed.clone()),
                    shot_audio::beats::Beats {
                        bpm: 0.0,
                        at: Vec::new(),
                    },
                ),
                gain_db: -6.0,
                duck_db: -12.0,
            }),
        };
        let score = Score {
            music: Music::new(vec![cue(0), cue(200_000)], CROSSFADE),
            sfx: vec![SfxCue::new(Sound::new(hit), Anchor::Start(100_000), -6.0)],
            bumpers: std::iter::once(300_000..320_000).collect(),
        };
        let voice = |n: u64| if n % 40_000 < 20_000 { 0.3 } else { 0.0 };
        let run = |start: u64, len: u64| {
            let mut out: Vec<f32> = (start..start + len).map(voice).collect();
            score.mix_from(start).mix(&mut out);
            out
        };
        let whole = run(0, 400_000);
        for at in [150_000, 230_000, 310_000] {
            let start = at - PREROLL;
            let late = run(start, 400_000 - start);
            let (late, whole) = (&late[PREROLL as usize..], &whole[at as usize..]);
            let worst = late
                .iter()
                .zip(whole)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(worst < 1e-4, "from {at}: off by {worst}");
        }
    }
}
