//! Where a take's voice speaks, and how loud, streaming like transcription.
//!
//! Capture feeds mic audio as it arrives. A background thread resamples it to 16 kHz, keeps a
//! loudness level every 20 ms, and runs Silero voice activity detection (whisper.cpp's ggml
//! model, the one transcription uses) over blocks of about 30 s. Speech that reaches into the
//! last seconds of a block is detected again with the next block, so only the final block is left
//! to detect when recording stops.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use shot_core::{Micros, TimeRange};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::Error;
use crate::resample::{Timeline16k, WHISPER_RATE};

const SECOND: u64 = WHISPER_RATE as u64;
/// Audio gathered before a detection run.
const BLOCK: usize = 30 * SECOND as usize;
/// Speech reaching into this tail of a block is detected again with the next block.
const TAIL: u64 = 2 * SECOND;
/// Speech that started at most this long before the block end is carried whole.
const CARRY: u64 = 10 * SECOND;
/// Audio kept in front of carried speech.
const PAD: u64 = SECOND / 5;
/// Speech ranges closer than this join, so a block edge leaves no seam.
const JOIN: Micros = Micros(100_000);
/// Length of one loudness level.
pub const LEVEL_STEP: Micros = Micros(20_000);
const LEVEL_SAMPLES: usize = (SECOND * LEVEL_STEP.0 / 1_000_000) as usize;
/// The quietest level kept, in dBFS.
const FLOOR_DB: f32 = -100.0;

/// Speech ranges and loudness levels of one take, in its media time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Speech {
    /// Speech, in order and not overlapping.
    pub ranges: Vec<TimeRange>,
    /// Loudness in whole dBFS for every [`LEVEL_STEP`] from `levels_from`.
    pub levels: Vec<i8>,
    pub levels_from: Micros,
}

impl Speech {
    /// Power mean of the levels over `range`, in dBFS. `None` when no level covers it.
    pub fn level(&self, range: TimeRange) -> Option<f32> {
        let index = |t: Micros| (t.0.saturating_sub(self.levels_from.0) / LEVEL_STEP.0) as usize;
        let from = index(range.from()).min(self.levels.len());
        let to = index(range.to()).max(from + 1).min(self.levels.len());
        let levels = self.levels.get(from..to).filter(|l| !l.is_empty())?;
        let power: f32 = levels
            .iter()
            .map(|&db| 10f32.powf(f32::from(db) / 10.0))
            .sum::<f32>()
            / levels.len() as f32;
        Some(10.0 * power.max(1e-10).log10())
    }

    /// Whether any speech overlaps `range`.
    pub fn speaks_in(&self, range: TimeRange) -> bool {
        let first = self.ranges.partition_point(|s| s.to() <= range.from());
        self.ranges.get(first).is_some_and(|s| s.overlaps(range))
    }
}

enum Message {
    Audio {
        t: Micros,
        samples: Vec<f32>,
        rate: u32,
    },
    Finish,
}

/// Streams one take's mic audio into its [`Speech`].
pub struct SpeechDetector {
    tx: Sender<Message>,
    worker: JoinHandle<Result<Speech, Error>>,
}

impl SpeechDetector {
    /// Loads the VAD model ([`crate::models::vad_model`]) and starts the detecting thread.
    pub fn new(vad: &Path) -> Result<Self, Error> {
        whisper_rs::install_logging_hooks();
        let path = vad
            .to_str()
            .ok_or_else(|| Error::Whisper(format!("{} is not UTF-8", vad.display())))?;
        let mut params = WhisperVadContextParams::new();
        params.set_n_threads(2);
        let vad = WhisperVadContext::new(path, params)
            .map_err(|e| Error::Whisper(format!("cannot load {}: {e}", vad.display())))?;
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("speech".into())
            .spawn(move || Worker::new(vad).run(rx))?;
        Ok(Self { tx, worker })
    }

    /// Queues mono `samples` at `sample_rate` whose first sample is at media time `t`. Never
    /// blocks.
    pub fn feed(&self, t: Micros, samples: &[f32], sample_rate: u32) {
        // A send only fails when the worker stopped on an error, which `finish` returns.
        let _ = self.tx.send(Message::Audio {
            t,
            samples: samples.to_vec(),
            rate: sample_rate,
        });
    }

    /// Detects the queued audio and returns the take's speech.
    pub fn finish(self) -> Result<Speech, Error> {
        let _ = self.tx.send(Message::Finish);
        self.worker
            .join()
            .unwrap_or_else(|_| Err(Error::Whisper("the speech thread panicked".into())))
    }
}

struct Worker {
    vad: WhisperVadContext,
    timeline: Timeline16k,
    window: Vec<f32>,
    /// Timeline index of `window[0]`.
    window_start: u64,
    /// Samples not yet in a level.
    pending: Vec<f32>,
    speech: Speech,
}

impl Worker {
    fn new(vad: WhisperVadContext) -> Self {
        Self {
            vad,
            timeline: Timeline16k::default(),
            window: Vec::new(),
            window_start: 0,
            pending: Vec::new(),
            speech: Speech::default(),
        }
    }

    fn run(mut self, rx: Receiver<Message>) -> Result<Speech, Error> {
        let mut fresh = Vec::new();
        loop {
            fresh.clear();
            let finishing = match rx.recv() {
                Ok(Message::Audio { t, samples, rate }) => {
                    if self.speech.levels.is_empty() && self.pending.is_empty() {
                        self.speech.levels_from = t;
                    }
                    self.timeline.push(t, &samples, rate, &mut fresh);
                    false
                }
                Ok(Message::Finish) | Err(_) => {
                    self.timeline.flush(&mut fresh);
                    true
                }
            };
            self.levels(&fresh);
            self.window.extend_from_slice(&fresh);
            if finishing {
                self.detect(true)?;
                return Ok(self.speech);
            }
            if self.window.len() >= BLOCK {
                self.detect(false)?;
            }
        }
    }

    fn levels(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        let full = self.pending.len() / LEVEL_SAMPLES * LEVEL_SAMPLES;
        for chunk in self.pending[..full].as_chunks::<LEVEL_SAMPLES>().0 {
            let power = chunk.iter().map(|s| s * s).sum::<f32>() / LEVEL_SAMPLES as f32;
            let db = (10.0 * power.max(1e-12).log10()).clamp(FLOOR_DB, 0.0);
            self.speech.levels.push(db.round() as i8);
        }
        self.pending.drain(..full);
    }

    /// Detects speech in the window and commits what the next block cannot change. `last`
    /// commits everything.
    fn detect(&mut self, last: bool) -> Result<(), Error> {
        if self.window.is_empty() {
            return Ok(());
        }
        let start = self.window_start;
        let end = start + self.window.len() as u64;
        let found: Vec<(u64, u64)> = self
            .vad
            .segments_from_samples(WhisperVadParams::new(), &self.window)
            .map_err(|e| Error::Whisper(e.to_string()))?
            .map(|s| {
                let at = |cs: f32| (start + (cs.max(0.0) * SECOND as f32 / 100.0) as u64).min(end);
                (at(s.start), at(s.end))
            })
            .filter(|(from, to)| from < to)
            .collect();
        let cut = if last {
            end
        } else {
            let tail = end - TAIL;
            match found.iter().find(|(_, to)| *to > tail) {
                Some(&(from, _)) if from + CARRY > end => from.saturating_sub(PAD).max(start),
                _ => tail,
            }
        };
        for (from, to) in found {
            if from < cut {
                self.commit(from, to.min(cut));
            }
        }
        self.window.drain(..(cut - start) as usize);
        self.window_start = cut;
        Ok(())
    }

    fn commit(&mut self, from: u64, to: u64) {
        let Some(range) = TimeRange::new(self.timeline.time(from), self.timeline.time(to)) else {
            return;
        };
        match self.speech.ranges.last_mut() {
            Some(last) if range.from().0 <= last.to().0 + JOIN.0 => {
                *last = TimeRange::new(last.from(), range.to().max(last.to()))
                    .expect("joined ranges are not empty");
            }
            _ => self.speech.ranges.push(range),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    #[test]
    fn levels_average_power_over_a_range() {
        let speech = Speech {
            ranges: vec![range(1_000_000, 2_000_000)],
            levels: vec![-20, -20, -100, -100],
            levels_from: Micros(1_000_000),
        };
        let both = speech.level(range(1_000_000, 1_080_000)).unwrap();
        assert!((both - -23.0).abs() < 0.1, "{both}");
        assert_eq!(speech.level(range(1_000_000, 1_020_000)), Some(-20.0));
        assert!(speech.speaks_in(range(1_900_000, 2_500_000)));
        assert!(!speech.speaks_in(range(2_000_000, 2_500_000)));
    }
}
