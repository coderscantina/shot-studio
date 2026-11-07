//! A light streaming recognizer for voice follow.
//!
//! The take's transcriber (`shot_analyze::Transcriber`) calls a word stable only after two
//! decodes a second apart agree on it, two seconds or more after it was said. That is right for
//! the transcript and too slow for a prompter. The listener decodes the last [`WINDOW`] of mic
//! audio with whisper tiny.en on the CPU every [`HOP`] and hands the whole hypothesis to a
//! callback. It keeps nothing: the words are only good for matching against the script, which
//! [`crate::Prompter::hear`] does. Running on the CPU keeps it off the GPU the transcriber uses.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use shot_analyze::models::ModelSpec;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const RATE: usize = 16_000;
/// Audio decoded each time.
const WINDOW: usize = 3 * RATE;
/// New audio between decodes.
const HOP: usize = RATE / 10;
/// Whisper needs at least a second.
const MIN_AUDIO: usize = RATE;
/// A window is decoded only when its newest part has speech in it.
const RECENT: usize = RATE * 6 / 10;
/// RMS below this in [`RECENT`] counts as silence (about -45 dBFS).
const SILENCE_RMS: f32 = 0.0056;
const THREADS: i32 = 4;

/// The model the listener loads, pinned in `models/manifest.json` (decision 0008).
pub fn follow_model() -> ModelSpec {
    shot_analyze::models::follow_model()
}

/// Streams mic audio into quick, unstable hypotheses for voice follow.
pub struct Listener {
    tx: Sender<Vec<f32>>,
    worker: JoinHandle<()>,
}

impl Listener {
    /// Loads the [`follow_model`] at `model` and starts decoding on a thread of its own.
    /// `on_words` runs there with every new hypothesis of the last few seconds, oldest word first.
    pub fn new(
        model: &Path,
        on_words: impl FnMut(&[String]) + Send + 'static,
    ) -> Result<Self, String> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(false);
        let context = WhisperContext::new_with_params(model, params)
            .map_err(|e| format!("cannot load {}: {e}", model.display()))?;
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("voice-follow".into())
            .spawn(move || {
                if let Err(e) = run(&context, rx, on_words) {
                    eprintln!("voice follow stopped: {e}");
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, worker })
    }

    /// Queues mono samples at `rate` Hz. Never blocks.
    pub fn feed(&self, samples: &[f32], rate: u32) {
        let _ = self.tx.send(resample(samples, rate));
    }

    /// Stops decoding and waits for the thread.
    pub fn finish(self) {
        drop(self.tx);
        let _ = self.worker.join();
    }
}

fn run(
    context: &WhisperContext,
    rx: Receiver<Vec<f32>>,
    mut on_words: impl FnMut(&[String]),
) -> Result<(), String> {
    let mut state = context.create_state().map_err(|e| e.to_string())?;
    let mut audio: Vec<f32> = Vec::with_capacity(2 * WINDOW);
    let mut fresh = 0;
    loop {
        // Wait for a hop of audio, then take whatever else is queued.
        while fresh < HOP {
            let Ok(chunk) = rx.recv() else {
                return Ok(());
            };
            fresh += chunk.len();
            audio.extend(chunk);
        }
        loop {
            match rx.try_recv() {
                Ok(chunk) => audio.extend(chunk),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        fresh = 0;
        if audio.len() > WINDOW {
            audio.drain(..audio.len() - WINDOW);
        }
        if audio.len() < MIN_AUDIO
            || rms(&audio[audio.len().saturating_sub(RECENT)..]) < SILENCE_RMS
        {
            continue;
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_n_threads(THREADS);
        params.set_no_context(true);
        params.set_single_segment(true);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_temperature_inc(0.0);
        params.set_max_tokens(48);
        // The encoder only looks at the audio there is: 50 positions per second.
        params.set_audio_ctx((audio.len() * 50 / RATE) as i32 + 1);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        state.full(params, &audio).map_err(|e| e.to_string())?;
        let mut words = Vec::new();
        for segment in state.as_iter() {
            if let Ok(text) = segment.to_str_lossy() {
                words.extend(text.split_whitespace().map(str::to_owned));
            }
        }
        on_words(&words);
    }
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

/// To 16 kHz by averaging over each output sample's span of input. Coarse, which the matcher
/// does not mind, and stateless per chunk: a chunk boundary costs at most one sample.
fn resample(samples: &[f32], rate: u32) -> Vec<f32> {
    let rate = rate as usize;
    if rate == RATE {
        return samples.to_vec();
    }
    let out_len = samples.len() * RATE / rate;
    (0..out_len)
        .map(|i| {
            let from = i * rate / RATE;
            let to = ((i + 1) * rate / RATE).clamp(from + 1, samples.len());
            samples[from..to].iter().sum::<f32>() / (to - from) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_duration_and_level() {
        let tone: Vec<f32> = vec![0.5; 4_800];
        let out = resample(&tone, 48_000);
        assert_eq!(out.len(), 1_600);
        assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6));
        assert_eq!(resample(&tone[..441], 44_100).len(), 160);
    }
}
