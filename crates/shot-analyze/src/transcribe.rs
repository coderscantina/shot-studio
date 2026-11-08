//! Streaming transcription of a take's mic track with whisper.
//!
//! Capture feeds mic audio as it arrives. A background thread resamples it to 16 kHz and decodes
//! a sliding window of up to 28 s whenever at least 1 s of new audio is in, taking up to 8 s at a
//! time when it falls behind. A word is stable once two consecutive decodes agree on it (local
//! agreement), and only stable words reach the transcript and the `on_word` callback. The words
//! after them, which the next decode may still change, go to an `on_tentative` callback after
//! every decode, for readers that cannot wait. Once the window passes 15 s it is cut at the end
//! of the last stable word, and the stable text before the window is the decoder's prompt.
//!
//! Silero voice activity detection runs on the window before each decode. A window without
//! speech is dropped undecoded, silence in front of the first speech is cut off, and words that
//! do not overlap speech are discarded. Whisper otherwise fills silence with invented text and
//! repeats it through the prompt.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use shot_core::{Id, Micros, TimeRange, Transcript, Word};
use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters, WhisperState, WhisperVadContext, WhisperVadContextParams,
    WhisperVadParams,
};

use crate::Error;
use crate::resample::{Timeline16k, WHISPER_RATE};

const SECOND: usize = WHISPER_RATE as usize;
const MIN_STEP: usize = SECOND;
const MAX_STEP: usize = 8 * SECOND;
const MAX_WINDOW: usize = 28 * SECOND;
const TRIM_AFTER: usize = 15 * SECOND;
/// With less room than one step left, the window commits what it has and moves on.
const FORCE_AT: usize = MAX_WINDOW - MAX_STEP;
/// Words may start this much before the last stable word ends and still count as new.
const SLACK: u64 = 100_000;
const PROMPT_WORDS: usize = 40;
/// Audio kept before speech starts, in 16 kHz samples.
const LEAD: u64 = SECOND as u64 / 2;
/// A word counts as speech when it comes this close to a VAD speech segment.
const SPEECH_PAD: u64 = 200_000;
/// How far a token reaches from its DTW time.
const REACH: u64 = 200_000;
/// Spoken language of the takes.
const LANGUAGE: &str = "en";

enum Message {
    Audio {
        t: Micros,
        samples: Vec<f32>,
        rate: u32,
    },
    Finish,
}

/// Streams one take's mic audio into a word-timed [`Transcript`].
pub struct Transcriber {
    tx: Sender<Message>,
    worker: JoinHandle<Result<Transcript, Error>>,
}

impl Transcriber {
    /// Loads the whisper and VAD models ([`crate::models::whisper_model`] and
    /// [`crate::models::vad_model`]) and starts the decoding thread. `on_word` runs on that
    /// thread for each word as it becomes stable.
    pub fn new(
        whisper: &Path,
        vad: &Path,
        take: Id,
        on_word: impl FnMut(&Word) + Send + 'static,
    ) -> Result<Self, Error> {
        Self::with_tentative(whisper, vad, take, on_word, |_| {})
    }

    /// [`Transcriber::new`] that also hands `on_tentative` the words after the stable ones,
    /// after every decode and on that thread: empty once they are all stable.
    pub fn with_tentative(
        whisper: &Path,
        vad: &Path,
        take: Id,
        on_word: impl FnMut(&Word) + Send + 'static,
        on_tentative: impl FnMut(&[Word]) + Send + 'static,
    ) -> Result<Self, Error> {
        whisper_rs::install_logging_hooks();
        let mut params = WhisperContextParameters::default();
        // Word times come from DTW on the alignment heads, which rules out flash attention.
        // Whisper's own token timestamps spread words over pauses.
        params.dtw_parameters(DtwParameters {
            mode: DtwMode::ModelPreset {
                model_preset: DtwModelPreset::LargeV3Turbo,
            },
            ..DtwParameters::default()
        });
        let context = WhisperContext::new_with_params(whisper, params).map_err(whisper_error)?;
        let vad = WhisperVadContext::new(
            vad.to_str()
                .ok_or_else(|| Error::Whisper(format!("{} is not UTF-8", vad.display())))?,
            WhisperVadContextParams::new(),
        )
        .map_err(|e| Error::Whisper(format!("cannot load {}: {e}", vad.display())))?;
        let state = context.create_state().map_err(whisper_error)?;
        let eot = context.token_eot();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("transcribe".into())
            .spawn(move || {
                Worker {
                    state,
                    vad,
                    eot,
                    on_word: Box::new(on_word),
                    on_tentative: Box::new(on_tentative),
                    transcript: Transcript::new(take),
                    agreement: Agreement::default(),
                    timeline: Timeline16k::default(),
                    incoming: Vec::new(),
                    window: Vec::new(),
                    window_start: 0,
                    decoded_len: 0,
                }
                .run(rx)
            })?;
        Ok(Self { tx, worker })
    }

    /// Queues mono `samples` at `sample_rate` whose first sample is at media time `t`. Never
    /// blocks. A gap in `t` is filled with silence so word times stay in media time.
    pub fn feed(&self, t: Micros, samples: &[f32], sample_rate: u32) {
        // A send only fails when the worker stopped on an error, which `finish` returns.
        let _ = self.tx.send(Message::Audio {
            t,
            samples: samples.to_vec(),
            rate: sample_rate,
        });
    }

    /// Decodes the queued audio, commits the remaining words and returns the transcript.
    pub fn finish(self) -> Result<Transcript, Error> {
        let _ = self.tx.send(Message::Finish);
        self.worker
            .join()
            .unwrap_or_else(|_| Err(Error::Whisper("the decoding thread panicked".into())))
    }
}

fn whisper_error(e: whisper_rs::WhisperError) -> Error {
    Error::Whisper(e.to_string())
}

type OnTentative = dyn FnMut(&[Word]) + Send;

struct Worker {
    state: WhisperState,
    vad: WhisperVadContext,
    eot: i32,
    on_word: Box<dyn FnMut(&Word) + Send>,
    on_tentative: Box<OnTentative>,
    transcript: Transcript,
    agreement: Agreement,
    timeline: Timeline16k,
    incoming: Vec<f32>,
    window: Vec<f32>,
    /// Timeline index of `window[0]`.
    window_start: u64,
    /// Window length at the last decode.
    decoded_len: usize,
}

impl Worker {
    fn run(mut self, rx: Receiver<Message>) -> Result<Transcript, Error> {
        let mut finishing = false;
        loop {
            while !finishing && self.incoming.len() < MIN_STEP {
                match rx.recv() {
                    Ok(Message::Audio { t, samples, rate }) => self.accept(t, &samples, rate),
                    Ok(Message::Finish) | Err(_) => finishing = true,
                }
            }
            while !finishing && self.incoming.len() < MAX_STEP {
                match rx.try_recv() {
                    Ok(Message::Audio { t, samples, rate }) => self.accept(t, &samples, rate),
                    Ok(Message::Finish) | Err(TryRecvError::Disconnected) => finishing = true,
                    Err(TryRecvError::Empty) => break,
                }
            }
            if finishing {
                self.timeline.flush(&mut self.incoming);
            }

            let take = self
                .incoming
                .len()
                .min(MAX_STEP)
                .min(MAX_WINDOW - self.window.len());
            self.window.extend(self.incoming.drain(..take));

            let last = finishing && self.incoming.is_empty();
            if self.window.len() > self.decoded_len {
                self.step()?;
            }
            if last {
                let rest = self.agreement.flush();
                self.commit(rest);
                (self.on_tentative)(&[]);
                return Ok(self.transcript);
            }
        }
    }

    /// Decodes the window if it holds speech and commits the words two decodes agree on.
    fn step(&mut self) -> Result<(), Error> {
        self.decoded_len = self.window.len();
        let speech = self.speech()?;
        let Some(first) = speech.first() else {
            let rest = self.agreement.flush();
            if !rest.is_empty() {
                self.commit(rest);
                (self.on_tentative)(&[]);
            }
            self.keep_from(self.window_end().saturating_sub(LEAD));
            return Ok(());
        };
        let first = self.index(first.from()).saturating_sub(LEAD);
        if first > self.window_start + LEAD && self.agreement.previous.is_empty() {
            self.keep_from(first);
        }
        let mut hypothesis = self.decode()?;
        hypothesis.retain(|w| {
            speech
                .iter()
                .any(|s| w.from.0 < s.to().0 + SPEECH_PAD && w.to.0 + SPEECH_PAD > s.from().0)
        });
        let stable = self.agreement.agree(hypothesis);
        self.commit(stable);
        self.trim();
        let tentative = self.agreement.tentative();
        (self.on_tentative)(&tentative);
        Ok(())
    }

    /// Speech in the window, in media time.
    fn speech(&mut self) -> Result<Vec<TimeRange>, Error> {
        let start = self.time(self.window_start);
        let segments = self
            .vad
            .segments_from_samples(WhisperVadParams::new(), &self.window)
            .map_err(whisper_error)?;
        Ok(segments
            .filter_map(|s| {
                let at = |cs: f32| Micros(start.0 + (cs.max(0.0) * 10_000.0) as u64);
                TimeRange::new(at(s.start), at(s.end))
            })
            .collect())
    }

    fn window_end(&self) -> u64 {
        self.window_start + self.window.len() as u64
    }

    /// Timeline index of a media time.
    fn index(&self, t: Micros) -> u64 {
        self.timeline.index(t)
    }

    /// Drops the window audio before timeline index `cut`.
    fn keep_from(&mut self, cut: u64) {
        let cut = cut.clamp(self.window_start, self.window_end());
        let drop = (cut - self.window_start) as usize;
        self.window.drain(..drop);
        self.window_start = cut;
        self.decoded_len = self.decoded_len.saturating_sub(drop);
    }

    fn accept(&mut self, t: Micros, samples: &[f32], rate: u32) {
        self.timeline.push(t, samples, rate, &mut self.incoming);
    }

    fn time(&self, index: u64) -> Micros {
        self.timeline.time(index)
    }

    fn decode(&mut self) -> Result<Vec<Hyp>, Error> {
        let window_start = self.time(self.window_start);
        let window_end = self.time(self.window_end());
        let prompt = self.prompt(window_start);

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(LANGUAGE));
        params.set_n_threads(4);
        params.set_no_context(true);
        params.set_suppress_nst(true);
        params.set_temperature_inc(0.0);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        if !prompt.is_empty() {
            params.set_initial_prompt(&prompt);
        }
        self.state
            .full(params, &self.window)
            .map_err(whisper_error)?;

        let at = |cs: i64| Micros((window_start.0 + cs.max(0) as u64 * 10_000).min(window_end.0));
        let mut tokens = Vec::new();
        for segment in self.state.as_iter() {
            for i in 0..segment.n_tokens() {
                let Some(token) = segment.get_token(i) else {
                    continue;
                };
                let data = token.token_data();
                // whisper.cpp numbers every special token (timestamps, language) from eot on.
                if data.id >= self.eot {
                    continue;
                }
                if let Ok(text) = token.to_str_lossy() {
                    tokens.push((text.into_owned(), at(data.t_dtw), data.p));
                }
            }
        }
        let window = TimeRange::new(window_start, window_end).expect("the window holds audio");
        Ok(group_words(&tokens, window))
    }

    /// Stable text before the window, as decoder context.
    fn prompt(&self, window_start: Micros) -> String {
        let words = self.transcript.words();
        let before = words.partition_point(|w| w.range.to() <= window_start);
        let from = before.saturating_sub(PROMPT_WORDS);
        words[from..before]
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn commit(&mut self, words: Vec<Word>) {
        for word in words {
            (self.on_word)(&word);
            self.transcript
                .push(word)
                .expect("agreement keeps words in order");
        }
    }

    fn trim(&mut self) {
        if self.window.len() <= TRIM_AFTER {
            return;
        }
        let full = self.window.len() >= FORCE_AT;
        if full {
            let rest = self.agreement.flush();
            self.commit(rest);
        }
        let mut cut = self.agreement.last_end.map_or(0, |end| self.index(end));
        if full {
            cut = cut.max(self.window_end() - 2 * SECOND as u64);
        }
        self.keep_from(cut);
    }
}

/// Groups decoded tokens `(text, time, probability)` into words; a token starting with a space
/// starts a word. A DTW time lands near the middle of its token, so a token spans halfway to its
/// neighbours' times, at most [`REACH`] to each side, which leaves pauses outside the words.
fn group_words(tokens: &[(String, Micros, f32)], window: TimeRange) -> Vec<Hyp> {
    let half = |a: Micros, b: Micros| Micros((a.0 + b.0) / 2);
    let mut words: Vec<Hyp> = Vec::new();
    for (i, (text, at, p)) in tokens.iter().enumerate() {
        let before = i
            .checked_sub(1)
            .map_or(window.from(), |j| half(tokens[j].1, *at));
        let after = tokens
            .get(i + 1)
            .map_or(window.to(), |next| half(*at, next.1));
        let from = before.max(Micros(at.0.saturating_sub(REACH)));
        let to = after.min(Micros(at.0 + REACH)).max(from);
        match words.last_mut() {
            Some(word) if !text.starts_with(' ') => {
                word.text.push_str(text);
                word.to = to;
                word.p_sum += p;
                word.tokens += 1;
            }
            _ => words.push(Hyp {
                text: text.trim_start().to_owned(),
                from,
                to,
                p_sum: *p,
                tokens: 1,
            }),
        }
    }
    words.retain(|w| !w.text.is_empty() && !is_annotation(&w.text));
    words
}

/// `[BLANK_AUDIO]`, `(music)` and the like.
fn is_annotation(text: &str) -> bool {
    let text = text.trim();
    (text.starts_with('[') && text.ends_with(']')) || (text.starts_with('(') && text.ends_with(')'))
}

/// A word of one decode, not yet stable.
#[derive(Debug, Clone)]
struct Hyp {
    text: String,
    from: Micros,
    to: Micros,
    p_sum: f32,
    tokens: u32,
}

/// Local agreement between consecutive decodes of the window.
#[derive(Default)]
struct Agreement {
    previous: Vec<Hyp>,
    /// Normalized text of the last stable words, to drop a repeat at the window start.
    tail: Vec<String>,
    last_end: Option<Micros>,
}

impl Agreement {
    /// Takes one decode and returns the words it agrees on with the decode before.
    fn agree(&mut self, hypothesis: Vec<Hyp>) -> Vec<Word> {
        let hypothesis = self.new_part(hypothesis);
        let agreed = self
            .previous
            .iter()
            .zip(&hypothesis)
            .take_while(|(a, b)| normalize(&a.text) == normalize(&b.text))
            .count();
        let mut hypothesis = hypothesis;
        self.previous = hypothesis.split_off(agreed);
        hypothesis.into_iter().map(|h| self.stable(h)).collect()
    }

    /// Commits the last decode as is, at the end of the take or when the window is full.
    fn flush(&mut self) -> Vec<Word> {
        std::mem::take(&mut self.previous)
            .into_iter()
            .map(|h| self.stable(h))
            .collect()
    }

    /// The words after the stable ones, without a repeat of the stable tail.
    fn new_part(&self, hypothesis: Vec<Hyp>) -> Vec<Hyp> {
        let after = self.last_end.map_or(0, |end| end.0.saturating_sub(SLACK));
        let mut words: Vec<Hyp> = hypothesis
            .into_iter()
            .filter(|w| w.from.0 >= after)
            .collect();
        let max = self.tail.len().min(words.len()).min(5);
        if let Some(n) = (1..=max).rev().find(|&n| {
            self.tail[self.tail.len() - n..]
                .iter()
                .zip(&words[..n])
                .all(|(a, b)| *a == normalize(&b.text))
        }) {
            words.drain(..n);
        }
        words
    }

    fn stable(&mut self, h: Hyp) -> Word {
        let word = h.word(self.last_end);
        self.last_end = Some(word.range.to());
        self.tail.push(normalize(&word.text));
        if self.tail.len() > 5 {
            self.tail.remove(0);
        }
        word
    }

    /// The last decode's words after the stable ones, as they would commit now.
    fn tentative(&self) -> Vec<Word> {
        let mut last_end = self.last_end;
        self.previous
            .iter()
            .map(|h| {
                let word = h.clone().word(last_end);
                last_end = Some(word.range.to());
                word
            })
            .collect()
    }
}

impl Hyp {
    /// The word, starting no earlier than `after`.
    fn word(self, after: Option<Micros>) -> Word {
        let from = after.map_or(self.from, |end| self.from.max(end));
        let to = self.to.max(Micros(from.0 + 10_000));
        Word {
            confidence: (self.p_sum / self.tokens as f32 * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8,
            text: self.text,
            range: TimeRange::new(from, to).expect("to is after from"),
        }
    }
}

/// Lowercase letters, digits and apostrophes, for comparing words across decodes.
pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'')
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hyp(words: &[(&str, u64)]) -> Vec<Hyp> {
        words
            .iter()
            .map(|&(text, from_ms)| Hyp {
                text: text.into(),
                from: Micros(from_ms * 1000),
                to: Micros((from_ms + 300) * 1000),
                p_sum: 0.9,
                tokens: 1,
            })
            .collect()
    }

    fn texts(words: &[Word]) -> Vec<&str> {
        words.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn commits_only_what_two_decodes_agree_on() {
        let mut agreement = Agreement::default();
        assert!(agreement.agree(hyp(&[("Ship", 0), ("it", 400)])).is_empty());
        let stable = agreement.agree(hyp(&[("ship", 0), ("it.", 400), ("Then", 800)]));
        assert_eq!(texts(&stable), ["ship", "it."]);
        assert_eq!(stable[0].confidence, 90);
        let stable = agreement.agree(hyp(&[("it", 400), ("then", 800), ("test", 1200)]));
        assert_eq!(texts(&stable), ["then"]);
        assert_eq!(texts(&agreement.tentative()), ["test"]);
        assert_eq!(texts(&agreement.flush()), ["test"]);
        assert!(agreement.tentative().is_empty());
    }

    #[test]
    fn drops_a_repeat_of_the_stable_tail_and_keeps_times_in_order() {
        let mut agreement = Agreement::default();
        agreement.agree(hyp(&[("we", 0), ("ship", 400)]));
        agreement.agree(hyp(&[("we", 0), ("ship", 400)]));
        agreement.agree(hyp(&[("ship", 650), ("today", 900)]));
        let stable = agreement.agree(hyp(&[("ship", 650), ("today", 650)]));
        assert_eq!(texts(&stable), ["today"]);
        assert!(stable[0].range.from() >= Micros(700_000));
    }

    #[test]
    fn groups_tokens_into_words_reaching_halfway_to_their_neighbours() {
        let ms = |ms: u64| Micros(ms * 1000);
        let tokens = [
            (" That", ms(140), 0.8),
            (" setup", ms(840), 0.9),
            (".", ms(1420), 0.7),
            (" Now", ms(9340), 1.0),
            (" let", ms(9560), 1.0),
            ("'s", ms(9640), 1.0),
            (" [BLANK_AUDIO]", ms(9700), 1.0),
        ]
        .map(|(text, at, p)| (text.to_owned(), at, p));
        let words = group_words(&tokens, TimeRange::new(ms(0), ms(12_000)).unwrap());
        let spans: Vec<_> = words
            .iter()
            .map(|w| (w.text.as_str(), w.from.0 / 1000, w.to.0 / 1000))
            .collect();
        assert_eq!(
            spans,
            [
                ("That", 0, 340),
                ("setup.", 640, 1620),
                ("Now", 9140, 9450),
                ("let's", 9450, 9670),
            ]
        );
    }

    #[test]
    fn normalizes_case_and_punctuation() {
        assert_eq!(normalize("Don't,"), "don't");
        assert_eq!(normalize("4K."), "4k");
    }
}
