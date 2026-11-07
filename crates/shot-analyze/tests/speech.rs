//! Transcription quality on the generated speech clips. Needs `$SHOT_FIXTURES` and the whisper
//! model (`shot models fetch`), so it is ignored by default:
//! `cargo test --release -p shot-analyze --test speech -- --ignored --nocapture`

use std::path::{Path, PathBuf};

use shot_analyze::{Transcriber, decode_mono, models, normalize};
use shot_core::{Id, Micros, Transcript};

fn fixtures() -> PathBuf {
    PathBuf::from(std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is set")).join("speech")
}

/// Streams a clip in 10 ms chunks, as capture delivers it, and returns the transcript.
fn transcribe(wav: &Path) -> Transcript {
    let dir = models::models_dir();
    let (whisper, vad) = (
        models::whisper_model().path(&dir),
        models::vad_model().path(&dir),
    );
    let transcriber = Transcriber::new(&whisper, &vad, Id::new("t01").unwrap(), |_| {}).unwrap();
    decode_mono(wav, |t, samples, rate| {
        let chunk = rate as usize / 100;
        for (i, part) in samples.chunks(chunk).enumerate() {
            let offset = i as u64 * chunk as u64 * 1_000_000 / u64::from(rate);
            transcriber.feed(Micros(t.0 + offset), part, rate);
        }
    })
    .unwrap();
    transcriber.finish().unwrap()
}

/// Lowercase words without punctuation, hyphens split.
fn words(text: &str) -> Vec<String> {
    text.replace('-', " ")
        .split_whitespace()
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect()
}

fn word_error_rate(reference: &[String], hypothesis: &[String]) -> f64 {
    let mut row: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, r) in reference.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, h) in hypothesis.iter().enumerate() {
            let substitute = diagonal + usize::from(r != h);
            diagonal = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[hypothesis.len()] as f64 / reference.len() as f64
}

/// Starts of speech after at least 300 ms below -60 dBFS, in 10 ms frames.
fn onsets(wav: &Path) -> Vec<Micros> {
    let mut samples = Vec::new();
    let mut frame = 0;
    decode_mono(wav, |_, chunk, rate| {
        frame = rate as usize / 100;
        samples.extend_from_slice(chunk);
    })
    .unwrap();
    let mut onsets = Vec::new();
    let mut silent = 30;
    for (i, f) in samples.chunks(frame).enumerate() {
        if f.iter().map(|s| s * s).sum::<f32>() / (f.len() as f32) < 1e-6 {
            silent += 1;
        } else {
            if silent >= 30 {
                onsets.push(Micros(i as u64 * 10_000));
            }
            silent = 0;
        }
    }
    onsets
}

#[test]
#[ignore = "needs $SHOT_FIXTURES and the whisper model"]
fn speech_clips_word_error_rate_and_onset_timing() {
    let mut clips: Vec<PathBuf> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    clips.sort();
    let mut errors = Vec::new();
    let mut timing_errors = Vec::new();
    for wav in &clips {
        let transcript = transcribe(wav);
        let reference = words(&std::fs::read_to_string(wav.with_extension("txt")).unwrap());
        let text: Vec<&str> = transcript.words().iter().map(|w| w.text.as_str()).collect();
        let error = word_error_rate(&reference, &words(&text.join(" ")));
        errors.push(error);

        let mut clip_timing = Vec::new();
        for onset in onsets(wav) {
            if let Some(word) = transcript
                .words()
                .iter()
                .min_by_key(|w| w.range.from().0.abs_diff(onset.0))
            {
                clip_timing.push(word.range.from().0 as i64 - onset.0 as i64);
            }
        }
        println!(
            "{}: WER {:.1}%, word start minus speech onset {:?} ms\n  {}",
            wav.file_stem().unwrap().to_string_lossy(),
            error * 100.0,
            clip_timing.iter().map(|d| d / 1000).collect::<Vec<_>>(),
            transcript
                .words()
                .iter()
                .map(|w| format!(
                    "{}@{:.2}-{:.2}",
                    w.text,
                    w.range.from().0 as f64 / 1e6,
                    w.range.to().0 as f64 / 1e6
                ))
                .collect::<Vec<_>>()
                .join(" ")
        );
        timing_errors.extend(clip_timing);
    }
    // Mean, not worst: whisper writes spoken numbers as digits ("13.53"), which no word-level
    // normalization maps back to "thirteen fifty-three".
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    assert!(mean <= 0.1, "mean WER {:.1}%", mean * 100.0);
    assert!(
        timing_errors.iter().all(|d| d.abs() <= 200_000),
        "a phrase starts more than 200 ms off its speech onset: {timing_errors:?}"
    );
}
