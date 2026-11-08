//! The preview's finished sound, made like export's ([`shot_render::voice`]): the plan's voice
//! ([`shot_render::export::plan_audio`]) through the voice chain, bleeps over it, export's
//! [`Score`] of music, sound effects, the brand's sounds and the bumpers' own sound
//! ([`shot_render::export::preview_score`]), and the gain to the loudness target with the
//! true-peak limiter.
//!
//! Export runs the whole cut from its first sample. The preview starts anywhere, so it starts
//! the chain, the mix and the limiter a second early and drops that second: the compressor, the
//! ducking and a music cue's fade-in are settled where the sound is heard. The gain comes from
//! a pass over the whole cut on a thread of its own, export's first pass.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use shot_audio::{CEILING_DBTP, CROSSFADE, Finish, Meter, Mix, TARGET_LUFS, VoiceChain};
use shot_media::audio::SAMPLE_RATE;
use shot_render::audio::{TimelineAudio, TimelineAudioError};
use shot_render::voice::Score;

/// How early the chain starts before the first heard sample.
const PREROLL: u64 = CROSSFADE;
/// Samples processed per step of the gain pass.
const PASS_BLOCK: usize = SAMPLE_RATE as usize / 4;

/// Voice chain, bleeps and mix from output sample `start` on, export's `pass`.
struct Voice {
    chain: VoiceChain,
    mix: Mix,
    bleeps: Vec<Range<u64>>,
    /// Chain output still to drop, its lead-in.
    skip: usize,
    /// The output sample the next chain output is.
    at: u64,
    /// The next input sample to read.
    read: u64,
}

impl Voice {
    fn new(audio: &TimelineAudio, score: &Score, start: u64) -> Self {
        Self {
            chain: VoiceChain::new(),
            mix: score.mix_from(start),
            bleeps: audio.bleeps().to_vec(),
            skip: VoiceChain::LATENCY,
            at: start,
            read: start,
        }
    }

    /// Reads `len` more input samples through the chain and mix, appends what comes out to
    /// `out`, which lags the input by the chain's latency.
    fn step(
        &mut self,
        audio: &mut TimelineAudio,
        len: usize,
        out: &mut Vec<f32>,
    ) -> Result<(), TimelineAudioError> {
        let begin = out.len();
        audio.read(self.read..self.read + len as u64, out)?;
        self.read += len as u64;
        self.chain.process(&mut out[begin..]);
        let drop = self.skip.min(out.len() - begin);
        self.skip -= drop;
        out.drain(begin..begin + drop);
        let samples = &mut out[begin..];
        let (from, to) = (self.at, self.at + samples.len() as u64);
        for bleep in &self.bleeps {
            for n in bleep.start.max(from)..bleep.end.min(to) {
                samples[(n - from) as usize] =
                    shot_render::overlay::redact::tone(n, n - bleep.start, bleep.end - bleep.start);
            }
        }
        self.mix.mix(samples);
        self.at = to;
        Ok(())
    }
}

/// The finished sound from an output sample on, read piece by piece.
pub struct Finished {
    voice: Voice,
    finish: Finish,
    /// Finished output still to drop: the limiter's lead-in, then the preroll.
    skip: usize,
    buffer: Vec<f32>,
}

impl Finished {
    /// The finished sound from output sample `at`, at `gain_db` to the target.
    pub fn new(audio: &TimelineAudio, score: &Score, gain_db: f64, at: u64) -> Self {
        let start = at.saturating_sub(PREROLL);
        Self {
            voice: Voice::new(audio, score, start),
            finish: Finish::new(gain_db, CEILING_DBTP),
            skip: Finish::LATENCY + (at - start) as usize,
            buffer: Vec::new(),
        }
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
            self.finish.process(&mut self.buffer[begin..]);
            let drop = self.skip.min(self.buffer.len() - begin);
            self.skip -= drop;
            self.buffer.drain(begin..begin + drop);
        }
        out.extend(self.buffer.drain(..len));
        Ok(())
    }
}

/// The gain export's first pass gives the whole output: to the loudness target from what the
/// chain and mix measure. `None` when `cancel` moved past `generation` or the cut is silent.
pub fn gain(
    audio: &mut TimelineAudio,
    score: &Score,
    cancel: &Arc<AtomicU64>,
    generation: u64,
) -> Result<Option<f64>, TimelineAudioError> {
    let mut voice = Voice::new(audio, score, 0);
    let mut meter = Meter::new();
    let total = audio.len() + VoiceChain::LATENCY as u64;
    let mut block = Vec::with_capacity(PASS_BLOCK);
    while voice.read < total {
        if cancel.load(Ordering::Relaxed) != generation {
            return Ok(None);
        }
        block.clear();
        let len = PASS_BLOCK.min((total - voice.read) as usize);
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
