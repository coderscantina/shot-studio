//! The preview's finished sound, made like export's ([`shot_render::voice`]): the cut's voice
//! through the voice chain, bleeps over it, the music bed and the lock's music and sound
//! effects in a [`Mix`], and the gain to the loudness target with the true-peak limiter.
//!
//! Export runs the whole cut from its first sample. The preview starts anywhere, so it starts
//! the chain, the mix and the limiter a second early and drops that second: the compressor, the
//! ducking and a music cue's fade-in are settled where the sound is heard. The gain comes from
//! a pass over the whole cut on a thread of its own, export's first pass.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use shot_audio::sound::{Library, Sound, Source};
use shot_audio::{
    Anchor, CEILING_DBTP, CROSSFADE, Finish, Meter, Mix, Music, MusicCue, Play, SfxCue,
    TARGET_LUFS, Track, VoiceChain,
};
use shot_core::{Applied, Id, Lock, Micros};
use shot_media::audio::SAMPLE_RATE;
use shot_render::audio::{AudioSegment, TimelineAudio, TimelineAudioError};

use crate::model::{EditMap, FPS};

/// How early the chain starts before the first heard sample.
const PREROLL: u64 = CROSSFADE;
/// Samples processed per step of the gain pass.
const PASS_BLOCK: usize = SAMPLE_RATE as usize / 4;

/// What plays with the voice, as the files and the lock name it, still in take time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cues {
    /// Where `music` files are relative to and the sound library's folders.
    dir: PathBuf,
    brand: Option<PathBuf>,
    /// The cut's music bed: file, gain and duck in dB.
    bed: Option<(String, i16, i16)>,
    music: Vec<(Id, Micros, String, i16, i16)>,
    sfx: Vec<(Id, Micros, Id, i16)>,
}

impl Cues {
    /// The music bed of `cut.kdl` and the `music` and `sfx` instances of the lock, for the
    /// video in `dir`.
    pub fn of(dir: &Path, bed: Option<&shot_core::Music>, lock: Option<&Lock>) -> Self {
        let mut cues = Self {
            dir: dir.to_owned(),
            brand: channel_brand(dir),
            bed: bed.map(|m| (m.file.clone(), m.gain, m.duck)),
            ..Self::default()
        };
        for take in lock.iter().flat_map(|l| &l.takes) {
            for instance in &take.instances {
                match &instance.applied {
                    Applied::Music {
                        file,
                        at,
                        gain,
                        duck,
                    } => cues
                        .music
                        .push((take.take.clone(), *at, file.clone(), *gain, *duck)),
                    Applied::Sfx { sound, at, gain } => {
                        cues.sfx
                            .push((take.take.clone(), *at, sound.clone(), *gain));
                    }
                    _ => {}
                }
            }
        }
        cues
    }
}

/// The channel's `brand/` folder when the video is `<repo>/videos/<slug>`, as export finds it.
fn channel_brand(dir: &Path) -> Option<PathBuf> {
    let brand = dir.parent()?.parent()?.join("brand");
    brand.is_dir().then_some(brand)
}

/// Decoded sounds by file or name, kept across plans so an edit does not decode them again.
#[derive(Default)]
pub struct Sounds {
    files: HashMap<PathBuf, Sound>,
    named: HashMap<(Option<PathBuf>, Id), Sound>,
}

impl Sounds {
    fn file(&mut self, path: &Path) -> Result<Sound, String> {
        if let Some(sound) = self.files.get(path) {
            return Ok(sound.clone());
        }
        let samples = shot_render::voice::read_samples(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let sound = Sound::new(samples);
        self.files.insert(path.to_owned(), sound.clone());
        Ok(sound)
    }

    fn named(&mut self, cues: &Cues, name: &Id) -> Result<Sound, String> {
        let key = (cues.brand.clone(), name.clone());
        if let Some(sound) = self.named.get(&key) {
            return Ok(sound.clone());
        }
        let sound = match Library::new(&cues.dir, cues.brand.as_deref()).find(name) {
            Some(Source::File(path)) => self.file(&path)?,
            Some(Source::Bundled(bundled)) => bundled.decode(),
            None => return Err(format!("no sound named {name}")),
        };
        self.named.insert(key, sound.clone());
        Ok(sound)
    }
}

/// A sound placed at an output sample.
#[derive(Clone)]
struct Placed {
    at: u64,
    sound: Sound,
    gain_db: f32,
    duck_db: f32,
}

/// The music and sound effects of one plan in output samples, and its bleeps.
#[derive(Clone, Default)]
pub struct Score {
    music: Vec<Placed>,
    sfx: Vec<Placed>,
    pub bleeps: Vec<(Id, shot_core::TimeRange)>,
}

impl Score {
    /// Places `cues` where `map` plays their moments, like export does: the bed from the start,
    /// each cue once for every span that shows its take time.
    pub fn place(map: &EditMap, cues: &Cues, sounds: &mut Sounds) -> Result<Self, String> {
        let mut score = Self::default();
        if let Some((file, gain, duck)) = &cues.bed {
            score.music.push(Placed {
                at: 0,
                sound: sounds.file(&cues.dir.join(file))?,
                gain_db: f32::from(*gain),
                duck_db: f32::from(*duck),
            });
        }
        for (take, at, file, gain, duck) in &cues.music {
            let places = output_samples(map, take, *at);
            if places.is_empty() {
                continue;
            }
            let sound = sounds.file(&cues.dir.join(file))?;
            score.music.extend(places.into_iter().map(|at| Placed {
                at,
                sound: sound.clone(),
                gain_db: f32::from(*gain),
                duck_db: f32::from(*duck),
            }));
        }
        for (take, at, name, gain) in &cues.sfx {
            let places = output_samples(map, take, *at);
            if places.is_empty() {
                continue;
            }
            let sound = sounds.named(cues, name)?;
            score.sfx.extend(places.into_iter().map(|at| Placed {
                at,
                sound: sound.clone(),
                gain_db: f32::from(*gain),
                duck_db: 0.0,
            }));
        }
        score.music.sort_by_key(|p| p.at);
        let mut bleeps: Vec<(Id, shot_core::TimeRange)> = map
            .spans
            .iter()
            .flat_map(|span| {
                shot_render::overlay::redact::bleeps(&span.overlays)
                    .into_iter()
                    .map(|range| (span.take.clone(), range))
            })
            .collect();
        bleeps.sort_by_key(|(take, range)| (take.to_string(), range.from()));
        bleeps.dedup();
        score.bleeps = bleeps;
        Ok(score)
    }

    /// The mix as export's would be at output sample `start`, with `start` as its sample 0. A
    /// music cue playing there starts from the place in its track export has reached; its
    /// fade-in falls in the preroll. Sound effects that started before are cut to their rest.
    fn mix_from(&self, start: u64) -> Mix {
        let track = |sound: Sound| {
            Track::new(
                sound,
                shot_audio::beats::Beats {
                    bpm: 0.0,
                    at: Vec::new(),
                },
            )
        };
        let playing = self.music.partition_point(|p| p.at <= start);
        let mut music = Vec::new();
        if let Some(cue) = playing.checked_sub(1).map(|i| &self.music[i]) {
            let samples = cue.sound.samples();
            if !samples.is_empty() {
                let offset = ((start - cue.at) % samples.len() as u64) as usize;
                let rotated: Vec<f32> = samples[offset..]
                    .iter()
                    .chain(&samples[..offset])
                    .copied()
                    .collect();
                music.push(MusicCue {
                    at: 0,
                    play: Some(Play {
                        track: track(Sound::new(rotated)),
                        gain_db: cue.gain_db,
                        duck_db: cue.duck_db,
                    }),
                });
            }
        }
        music.extend(self.music[playing..].iter().map(|cue| MusicCue {
            at: cue.at - start,
            play: Some(Play {
                track: track(cue.sound.clone()),
                gain_db: cue.gain_db,
                duck_db: cue.duck_db,
            }),
        }));
        let sfx = self
            .sfx
            .iter()
            .filter(|cue| cue.at + cue.sound.len() as u64 > start)
            .map(|cue| {
                let (sound, at) = match start.checked_sub(cue.at) {
                    Some(gone) if gone > 0 => {
                        (Sound::new(cue.sound.samples()[gone as usize..].to_vec()), 0)
                    }
                    _ => (cue.sound.clone(), cue.at - start),
                };
                SfxCue::new(sound, Anchor::Start(at), cue.gain_db)
            })
            .collect();
        Mix::new(Music::new(music, CROSSFADE), sfx, [])
    }
}

/// Where take time `at` of `take` plays, in output samples, once per span showing it.
fn output_samples(map: &EditMap, take: &Id, at: Micros) -> Vec<u64> {
    let per_frame = u64::from(SAMPLE_RATE / FPS);
    let t = at.0 * u64::from(SAMPLE_RATE) / 1_000_000;
    map.spans
        .iter()
        .filter(|s| s.take == *take)
        .filter_map(|s| {
            let start = s.source_frames.start * per_frame;
            let end = s.source_frames.end * per_frame;
            (start..end)
                .contains(&t)
                .then(|| s.output_frame * per_frame + t - start)
        })
        .collect()
}

/// The voice of `map`, cut like export's, with bleeps over the ranges of `score`.
pub fn timeline_audio(map: &EditMap, media: PathBuf, score: &Score) -> TimelineAudio {
    TimelineAudio::new(
        map.spans.iter().map(|span| AudioSegment {
            take: span.take.clone(),
            source_frames: span.source_frames.clone(),
        }),
        FPS,
        move |take| media.join(take.as_str()).join("mic.mp4"),
    )
    .with_bleeps(&score.bleeps)
}

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

    #[test]
    fn a_mix_started_late_matches_the_whole_mix_after_the_preroll() {
        let bed: Vec<f32> = (0..30_000)
            .map(|i| (i as f32 * 0.003).sin() * 0.2)
            .collect();
        let hit: Vec<f32> = (0..90_000)
            .map(|i| ((i % 97) as f32 - 48.0) / 200.0)
            .collect();
        let placed = |at, samples: &[f32]| Placed {
            at,
            sound: Sound::new(samples.to_vec()),
            gain_db: -6.0,
            duck_db: -12.0,
        };
        let score = Score {
            music: vec![placed(0, &bed), placed(200_000, &bed)],
            sfx: vec![placed(100_000, &hit)],
            bleeps: Vec::new(),
        };
        let voice = |n: u64| if n % 40_000 < 20_000 { 0.3 } else { 0.0 };
        let run = |start: u64, len: u64| {
            let mut out: Vec<f32> = (start..start + len).map(voice).collect();
            score.mix_from(start).mix(&mut out);
            out
        };
        let whole = run(0, 400_000);
        for at in [150_000, 230_000] {
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
