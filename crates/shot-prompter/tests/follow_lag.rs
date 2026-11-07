//! Voice-follow lag on the `say` fixtures, fed in real time (decision 0010: under 300 ms at the
//! median).
//!
//! A word's spoken time is its end in the take transcriber's offline transcript, which places a
//! word's edges halfway into the pauses around it, so the end is late rather than early. Its
//! highlight time is when the prompter first counts it as said. Both the listener and, for comparison,
//! the transcriber's stable words drive a prompter of their own, at the same time, as while
//! recording.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shot_analyze::{Transcriber, decode_mono, models, normalize};
use shot_core::{Id, Micros, PrompterMode, Script};
use shot_prompter::{Listener, Prompter, follow_model};

const CHUNK: usize = 480;
const STABLE_WORDS: usize = 8;

struct Follow {
    prompter: Prompter,
    /// When each script word was first counted as said.
    highlighted: Vec<Option<Instant>>,
}

impl Follow {
    fn new(text: &str) -> Arc<Mutex<Self>> {
        let script = Script::parse(&format!("# T\n\n## C\n\n- Beat\n  {text}\n")).unwrap();
        let prompter = Prompter::new(&script, PrompterMode::FullScript);
        let words = text.split_whitespace().count();
        Arc::new(Mutex::new(Self {
            prompter,
            highlighted: vec![None; words],
        }))
    }

    fn hear(&mut self, words: &[String]) {
        let before = self.prompter.position();
        self.prompter.hear(words);
        let now = Instant::now();
        for slot in &mut self.highlighted[before..self.prompter.position()] {
            slot.get_or_insert(now);
        }
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is set")).join("speech")
}

fn audio(path: &Path) -> (Vec<f32>, u32) {
    let mut samples = Vec::new();
    let mut rate = 0;
    decode_mono(path, |_, s, r| {
        samples.extend_from_slice(s);
        rate = r;
    })
    .unwrap();
    (samples, rate)
}

/// Start and end of each script word in the offline transcript, where the transcript has it.
fn spoken_ends(
    text: &str,
    samples: &[f32],
    rate: u32,
    whisper: &Path,
    vad: &Path,
) -> Vec<Option<(Micros, Micros)>> {
    let transcriber = Transcriber::new(whisper, vad, Id::new("t01").unwrap(), |_| {}).unwrap();
    transcriber.feed(Micros(0), samples, rate);
    transcriber.feed(
        Micros(samples.len() as u64 * 1_000_000 / u64::from(rate)),
        &vec![0.0; rate as usize],
        rate,
    );
    let transcript = transcriber.finish().unwrap();
    let heard: Vec<(String, (Micros, Micros))> = transcript
        .words()
        .iter()
        .map(|w| (normalize(&w.text), (w.range.from(), w.range.to())))
        .collect();
    let mut cursor = 0;
    text.split_whitespace()
        .map(|word| {
            let key = normalize(word);
            let found = heard[cursor..(cursor + 3).min(heard.len())]
                .iter()
                .position(|(w, _)| *w == key)?;
            cursor += found + 1;
            Some(heard[cursor - 1].1)
        })
        .collect()
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(f64::NAN)
}

#[test]
#[ignore = "needs the speech fixtures and the whisper models, runs in real time"]
fn voice_follow_lag_on_say_fixtures() {
    let dir = models::models_dir();
    let whisper = models::whisper_model().ensure(&dir, |_| {}).unwrap();
    let vad = models::vad_model().ensure(&dir, |_| {}).unwrap();
    let tiny = follow_model().ensure(&dir, |_| {}).unwrap();

    let mut names: Vec<_> = std::fs::read_dir(fixtures())
        .unwrap()
        .filter_map(|e| {
            let path = e.ok()?.path();
            (path.extension()? == "wav").then_some(path)
        })
        .collect();
    names.sort();

    let (mut fast_all, mut stable_all) = (Vec::new(), Vec::new());
    let mut said = (0, 0, 0);
    let mut from_start = Vec::new();
    for wav in names {
        let text = std::fs::read_to_string(wav.with_extension("txt")).unwrap();
        let (samples, rate) = audio(&wav);
        let ends = spoken_ends(&text, &samples, rate, &whisper, &vad);

        let fast = Follow::new(&text);
        let stable = Follow::new(&text);
        let listener = Listener::new(&tiny, {
            let fast = fast.clone();
            move |words| fast.lock().unwrap().hear(words)
        })
        .unwrap();
        let transcriber = Transcriber::new(&whisper, &vad, Id::new("t01").unwrap(), {
            let stable = stable.clone();
            let mut recent: Vec<String> = Vec::new();
            move |word| {
                recent.push(word.text.clone());
                let from = recent.len().saturating_sub(STABLE_WORDS);
                stable.lock().unwrap().hear(&recent[from..]);
            }
        })
        .unwrap();

        let silence = vec![0.0; rate as usize * 3 / 2];
        let start = Instant::now();
        for (k, chunk) in samples
            .chunks(CHUNK)
            .chain(silence.chunks(CHUNK))
            .enumerate()
        {
            let due =
                start + Duration::from_micros((k * CHUNK) as u64 * 1_000_000 / u64::from(rate));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let t = Micros((k * CHUNK) as u64 * 1_000_000 / u64::from(rate));
            listener.feed(chunk, rate);
            transcriber.feed(t, chunk, rate);
        }
        listener.finish();
        transcriber.finish().unwrap();

        let lags = |follow: &Mutex<Follow>, edge: fn(&(Micros, Micros)) -> Micros| -> Vec<f64> {
            let follow = follow.lock().unwrap();
            follow
                .highlighted
                .iter()
                .zip(&ends)
                .filter_map(|(at, word)| {
                    let at = at.as_ref()?.duration_since(start).as_secs_f64();
                    Some((at - edge(word.as_ref()?).0 as f64 / 1e6) * 1000.0)
                })
                .collect()
        };
        let (mut fast_lags, mut stable_lags) = (lags(&fast, |w| w.1), lags(&stable, |w| w.1));
        from_start.extend(lags(&fast, |w| w.0));
        let words = ends.len();
        let known = ends.iter().flatten().count();
        println!(
            "{}: {words} words, {known} timed, fast {} highlighted median {:.0} ms, stable {} highlighted median {:.0} ms",
            wav.file_stem().unwrap().to_string_lossy(),
            fast_lags.len(),
            median(&mut fast_lags),
            stable_lags.len(),
            median(&mut stable_lags),
        );
        said.0 += known;
        said.1 += fast_lags.len();
        said.2 += stable_lags.len();
        fast_all.extend(fast_lags);
        stable_all.extend(stable_lags);
    }
    let mut sorted = fast_all.clone();
    sorted.sort_by(f64::total_cmp);
    let p90 = sorted[sorted.len() * 9 / 10];
    let fast_median = median(&mut fast_all);
    println!(
        "all: {} timed words, fast {} highlighted median {fast_median:.0} ms p90 {p90:.0} ms \
         ({:.0} ms after the word starts), stable {} highlighted median {:.0} ms",
        said.0,
        said.1,
        median(&mut from_start),
        said.2,
        median(&mut stable_all)
    );
    assert!(fast_median < 300.0, "median lag {fast_median:.0} ms");
}
