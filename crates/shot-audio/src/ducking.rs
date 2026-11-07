//! Music bed with sidechain ducking keyed on the voice.

use crate::SAMPLE_RATE;

const VOICE_THRESHOLD: f32 = 0.005_6; // -45 dBFS
const DETECT_ATTACK_S: f32 = 0.005;
const DETECT_RELEASE_S: f32 = 0.050;
const HOLD_S: f32 = 0.200;
const DUCK_ATTACK_S: f32 = 0.030;
const DUCK_RELEASE_S: f32 = 0.400;

fn one_pole(seconds: f32) -> f32 {
    1.0 - (-1.0 / (seconds * SAMPLE_RATE as f32)).exp()
}

fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// A music bed under the voice: looped, at `gain_db`, ducked to `duck_db` while the voice speaks.
pub struct MusicBed {
    music: Vec<f32>,
    pos: usize,
    gain: f32,
    duck: f32,
    envelope: f32,
    hold: usize,
    /// 0 while the bed plays at full gain, 1 while fully ducked.
    amount: f32,
    detect_attack: f32,
    detect_release: f32,
    duck_attack: f32,
    duck_release: f32,
}

impl MusicBed {
    /// `music` is mono 48 kHz. Both gains are dB at most 0 (as in cut.kdl `music gain= duck=`).
    pub fn new(music: Vec<f32>, gain_db: f32, duck_db: f32) -> Self {
        Self {
            music,
            pos: 0,
            gain: db_to_linear(gain_db),
            duck: db_to_linear(duck_db),
            envelope: 0.0,
            hold: 0,
            amount: 0.0,
            detect_attack: one_pole(DETECT_ATTACK_S),
            detect_release: one_pole(DETECT_RELEASE_S),
            duck_attack: one_pole(DUCK_ATTACK_S),
            duck_release: one_pole(DUCK_RELEASE_S),
        }
    }

    /// Adds the next `voice.len()` samples of the bed under `voice`, keyed on `voice` itself.
    pub fn mix(&mut self, voice: &mut [f32]) {
        if self.music.is_empty() {
            return;
        }
        let hold_samples = (HOLD_S * SAMPLE_RATE as f32) as usize;
        for v in voice {
            let level = v.abs();
            let coef = if level > self.envelope {
                self.detect_attack
            } else {
                self.detect_release
            };
            self.envelope += coef * (level - self.envelope);
            if self.envelope > VOICE_THRESHOLD {
                self.hold = hold_samples;
            }
            let (goal, coef) = if self.hold > 0 {
                self.hold -= 1;
                (1.0, self.duck_attack)
            } else {
                (0.0, self.duck_release)
            };
            self.amount += coef * (goal - self.amount);

            let bed_gain = self.gain + self.amount * (self.duck - self.gain);
            *v += self.music[self.pos] * bed_gain;
            self.pos += 1;
            if self.pos == self.music.len() {
                self.pos = 0;
            }
        }
    }
}
