//! Music cues and sound effects mixed under and over the voice ([0021], [0023]).
//!
//! Times are output samples at 48 kHz: the caller places take times in the output. A music cue
//! plays its track, looped, until the next cue, and the two cross over [`CROSSFADE`] samples
//! from the next cue's start on equal-power curves. Music ducks under the voice and under the
//! bumpers with the [`MusicBed`](crate::MusicBed)'s sidechain. Sound effects are not ducked.
//! The mix comes before the loudness pass, which measures the whole.
//!
//! [0021]: ../../../docs/decisions/0021-sound-transitions-and-bookends.md
//! [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md

use std::f32::consts::FRAC_PI_2;
use std::ops::Range;

use crate::SAMPLE_RATE;
use crate::beats::{Beats, SNAP_WINDOW};
use crate::ducking::{Ducker, db_to_linear};
use crate::sound::Sound;

/// The default crossfade between music cues, 1 s.
pub const CROSSFADE: u64 = SAMPLE_RATE as u64;

/// A music file with its beats.
#[derive(Debug, Clone)]
pub struct Track {
    sound: Sound,
    beats: Vec<u64>,
}

impl Track {
    /// `beats` are [`crate::beats::detect`]'s for `sound`, cached by the caller.
    pub fn new(sound: Sound, beats: Beats) -> Self {
        Self {
            sound,
            beats: beats.at,
        }
    }
}

/// A track played by a cue, with its gains in dB, both at most 0 (as in `music gain= duck=`).
#[derive(Debug, Clone)]
pub struct Play {
    pub track: Track,
    pub gain_db: f32,
    pub duck_db: f32,
}

/// A music cue at an output sample: a track until the next cue, or `None` to stop.
#[derive(Debug, Clone)]
pub struct MusicCue {
    pub at: u64,
    pub play: Option<Play>,
}

/// The music of an output: its cues in order and the crossfade between them.
#[derive(Debug, Clone)]
pub struct Music {
    cues: Vec<MusicCue>,
    /// Each cue's gain and duck, linear.
    gains: Vec<(f32, f32)>,
    crossfade: u64,
}

impl Music {
    pub fn new(mut cues: Vec<MusicCue>, crossfade: u64) -> Self {
        cues.sort_by_key(|c| c.at);
        let gains = cues
            .iter()
            .map(|c| {
                c.play.as_ref().map_or((0.0, 0.0), |p| {
                    (db_to_linear(p.gain_db), db_to_linear(p.duck_db))
                })
            })
            .collect();
        Self {
            cues,
            gains,
            crossfade,
        }
    }

    /// `at` moved to the nearest beat of the music playing there within [`SNAP_WINDOW`],
    /// rounded to the nearest multiple of `frame` samples. Unchanged without such a beat.
    pub fn snap(&self, at: u64, frame: u64) -> u64 {
        assert!(frame > 0, "a frame has samples");
        let i = self.cues.partition_point(|c| c.at <= at);
        let Some(cue) = i.checked_sub(1).map(|i| &self.cues[i]) else {
            return at;
        };
        let Some(play) = &cue.play else {
            return at;
        };
        let len = play.track.sound.len() as u64;
        if len == 0 {
            return at;
        }
        let end = self.cues.get(i).map_or(u64::MAX, |c| c.at);
        let lap = (at - cue.at) / len;
        let nearest = (lap.saturating_sub(1)..=lap + 1)
            .flat_map(|lap| play.track.beats.iter().map(move |b| cue.at + lap * len + b))
            .filter(|t| (cue.at..end).contains(t) && t.abs_diff(at) <= SNAP_WINDOW)
            .min_by_key(|t| (t.abs_diff(at), *t));
        nearest.map_or(at, |beat| (beat + frame / 2) / frame * frame)
    }

    /// The music at sample `n`, ducked by `amount` from 0 to 1.
    fn sample(&self, n: u64, amount: f32) -> f32 {
        let started = self.cues.partition_point(|c| c.at <= n);
        let mut out = 0.0;
        for i in (0..started).rev() {
            let fade_out = match self.cues.get(i + 1) {
                Some(next) if next.at <= n => {
                    let since = n - next.at;
                    if since >= self.crossfade {
                        break;
                    }
                    (self.fade(since)).cos()
                }
                _ => 1.0,
            };
            let cue = &self.cues[i];
            let Some(play) = &cue.play else {
                continue;
            };
            let samples = play.track.sound.samples();
            if samples.is_empty() {
                continue;
            }
            let since = n - cue.at;
            let fade_in = if since < self.crossfade {
                self.fade(since).sin()
            } else {
                1.0
            };
            let (gain, duck) = self.gains[i];
            let gain = gain + amount * (duck - gain);
            out += samples[(since % samples.len() as u64) as usize] * gain * fade_in * fade_out;
        }
        out
    }

    /// The angle of an equal-power fade `since` samples in.
    fn fade(&self, since: u64) -> f32 {
        FRAC_PI_2 * since as f32 / self.crossfade as f32
    }
}

/// Where a sound effect plays: starting at a sample, or with its peak on one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start(u64),
    Peak(u64),
}

/// A sound effect placed in the output at a gain in dB, at most 0.
#[derive(Debug, Clone)]
pub struct SfxCue {
    sound: Sound,
    /// First sample in the output, negative when a peak anchor cuts off the head.
    start: i64,
    gain: f32,
}

impl SfxCue {
    pub fn new(sound: Sound, anchor: Anchor, gain_db: f32) -> Self {
        let start = match anchor {
            Anchor::Start(at) => at as i64,
            Anchor::Peak(at) => at as i64 - sound.peak() as i64,
        };
        Self {
            sound,
            start,
            gain: db_to_linear(gain_db),
        }
    }

    fn end(&self) -> i64 {
        self.start + self.sound.len() as i64
    }
}

/// The voice's company: music ducked under it and sound effects over it, streaming.
pub struct Mix {
    music: Music,
    sfx: Vec<SfxCue>,
    bumpers: Vec<Range<u64>>,
    ducker: Ducker,
    pos: u64,
}

impl Mix {
    /// `bumpers` are the output ranges the intro and outro bumpers play over.
    pub fn new(
        music: Music,
        mut sfx: Vec<SfxCue>,
        bumpers: impl IntoIterator<Item = Range<u64>>,
    ) -> Self {
        sfx.sort_by_key(|s| s.start);
        let mut bumpers: Vec<_> = bumpers.into_iter().collect();
        bumpers.sort_by_key(|b| b.start);
        Self {
            music,
            sfx,
            bumpers,
            ducker: Ducker::new(),
            pos: 0,
        }
    }

    /// The same mix with its next sample at output sample `at`, for a stream that starts there.
    /// The ducking starts released.
    pub fn starting_at(mut self, at: u64) -> Self {
        self.pos = at;
        self
    }

    /// Adds the next `voice.len()` samples of music and sound effects to `voice`, ducking the
    /// music on `voice` itself. The result does not depend on how the stream is split.
    pub fn mix(&mut self, voice: &mut [f32]) {
        let start = self.pos;
        let end = start + voice.len() as u64;
        let mut bumper = self.bumpers.partition_point(|b| b.end <= start);
        for (n, v) in (start..).zip(voice.iter_mut()) {
            while self.bumpers.get(bumper).is_some_and(|b| b.end <= n) {
                bumper += 1;
            }
            let under_bumper = self.bumpers.get(bumper).is_some_and(|b| b.start <= n);
            let amount = self.ducker.next(*v, under_bumper);
            *v += self.music.sample(n, amount);
        }

        let (start, end) = (start as i64, end as i64);
        let last = self.sfx.partition_point(|s| s.start < end);
        for cue in self.sfx[..last].iter().filter(|s| s.end() > start) {
            let from = cue.start.max(start);
            let to = cue.end().min(end);
            let sound =
                &cue.sound.samples()[(from - cue.start) as usize..(to - cue.start) as usize];
            for (v, s) in voice[(from - start) as usize..(to - start) as usize]
                .iter_mut()
                .zip(sound)
            {
                *v += s * cue.gain;
            }
        }
        self.pos = end as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(samples: Vec<f32>, beats: Vec<u64>) -> Track {
        Track::new(
            Sound::new(samples),
            Beats {
                bpm: 0.0,
                at: beats,
            },
        )
    }

    fn cue(at: u64, track: Track) -> MusicCue {
        MusicCue {
            at,
            play: Some(Play {
                track,
                gain_db: 0.0,
                duck_db: -20.0,
            }),
        }
    }

    #[test]
    fn crossfade_starts_and_ends_on_the_sample() {
        let a = track(vec![1.0; 100], vec![]);
        let b = track(vec![0.5; 100], vec![]);
        let fade = 480;
        let music = Music::new(vec![cue(0, a), cue(10_000, b)], fade);
        let mut out = vec![0.0; 12_000];
        Mix::new(music, vec![], []).mix(&mut out);

        assert!((out[fade as usize] - 1.0).abs() < 1e-6, "a has faded in");
        assert!((out[9_999] - 1.0).abs() < 1e-6);
        assert_eq!(out[10_000], 1.0, "a alone at the cue");
        let theta = |since: u64| FRAC_PI_2 * since as f32 / fade as f32;
        for since in [1, fade / 2, fade - 1] {
            let want = theta(since).cos() + 0.5 * theta(since).sin();
            let got = out[(10_000 + since) as usize];
            assert!(
                (got - want).abs() < 1e-6,
                "{since} into the fade: {got}, not {want}"
            );
        }
        assert_eq!(out[10_000 + fade as usize], 0.5, "b alone after the fade");
    }

    #[test]
    fn a_mix_started_late_plays_what_the_whole_mix_plays_there() {
        let music = || {
            let a = track((0..700).map(|i| (i as f32 * 0.1).sin()).collect(), vec![]);
            Music::new(vec![cue(0, a.clone()), cue(9_000, a)], 480)
        };
        let sfx = || {
            vec![SfxCue::new(
                Sound::new(vec![0.5; 3_000]),
                Anchor::Start(8_000),
                0.0,
            )]
        };
        let mut whole = vec![0.0; 12_000];
        Mix::new(music(), sfx(), []).mix(&mut whole);
        let mut late = vec![0.0; 3_500];
        Mix::new(music(), sfx(), [])
            .starting_at(8_500)
            .mix(&mut late);
        assert_eq!(late, whole[8_500..]);
    }

    #[test]
    fn music_ducks_under_voice_and_bumpers_and_sfx_do_not() {
        let len = 6 * SAMPLE_RATE as usize;
        let music = Music::new(vec![cue(0, track(vec![0.1; 1000], vec![]))], 1);
        let bumper = 5 * SAMPLE_RATE as u64..len as u64;
        let mut voice = vec![0.0; len];
        for s in &mut voice[SAMPLE_RATE as usize..2 * SAMPLE_RATE as usize] {
            *s = 0.2;
        }
        let sfx = SfxCue::new(Sound::new(vec![0.3; 10]), Anchor::Start(70_000), 0.0);
        let mut out = voice.clone();
        Mix::new(music, vec![sfx], Some(bumper)).mix(&mut out);
        let bed = |n: usize| out[n] - voice[n];

        assert!((bed(40_000) - 0.1).abs() < 1e-6, "full before the voice");
        assert!(
            (bed(90_000) - 0.01).abs() < 1e-4,
            "ducked 20 dB under the voice"
        );
        assert!(
            (bed(70_005) - (0.01 + 0.3)).abs() < 1e-4,
            "sfx at full gain"
        );
        assert!((bed(220_000) - 0.1).abs() < 1e-3, "back up between");
        assert!(
            (bed(280_000) - 0.01).abs() < 1e-4,
            "ducked under the bumper"
        );
    }

    #[test]
    fn peak_anchors_land_the_loudest_moment_and_pieces_equal_one_call() {
        let mut hit = vec![0.01; 4_000];
        hit[3_000..3_100].fill(0.9);
        let sound = Sound::new(hit);
        let cue = SfxCue::new(sound.clone(), Anchor::Peak(2_000), -6.0);
        assert_eq!(cue.start, 2_000 - 3_000);
        let music = Music::new(vec![cue_at_with_beats(500)], 300);
        let input: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.01).sin() * 0.1).collect();

        let mut whole = input.clone();
        Mix::new(music.clone(), vec![cue.clone()], Some(5_000..6_000)).mix(&mut whole);
        let mut split = input;
        let mut mix = Mix::new(music, vec![cue], Some(5_000..6_000));
        for piece in split.chunks_mut(777) {
            mix.mix(piece);
        }
        assert_eq!(whole, split);
        assert_eq!(sound.peak(), 3_000);
    }

    fn cue_at_with_beats(at: u64) -> MusicCue {
        cue(at, track(vec![0.2; 2_000], vec![100, 1_100]))
    }

    #[test]
    fn snapping_finds_the_nearest_beat_of_the_playing_cue_and_rounds_to_a_frame() {
        let beat = SAMPLE_RATE as u64 / 2;
        let len = 4 * beat;
        let beats = (0..4).map(|k| k * beat + 1_000).collect();
        let a = track(vec![0.1; len as usize], beats);
        let cut_in = 10 * SAMPLE_RATE as u64;
        let music = Music::new(
            vec![
                cue(SAMPLE_RATE as u64, a),
                MusicCue {
                    at: cut_in,
                    play: None,
                },
            ],
            CROSSFADE,
        );
        let frame = SAMPLE_RATE as u64 / 60;
        let start = SAMPLE_RATE as u64;

        // Third lap, second beat: 1 s + 2 * 2 s + 0.5 s + 1000 samples.
        let beat_at = start + 2 * len + beat + 1_000;
        let snapped = music.snap(beat_at + 4_000, frame);
        assert_eq!(snapped % frame, 0);
        assert!(
            snapped.abs_diff(beat_at) <= frame / 2,
            "{snapped} vs {beat_at}"
        );
        assert_eq!(
            music.snap(beat_at + SNAP_WINDOW + 100, frame),
            beat_at + SNAP_WINDOW + 100
        );
        assert_eq!(music.snap(500, frame), 500, "no music yet");
        assert_eq!(
            music.snap(cut_in + 1_000, frame),
            cut_in + 1_000,
            "music stopped"
        );
    }
}
