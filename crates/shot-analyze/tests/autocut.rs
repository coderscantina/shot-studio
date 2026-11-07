//! The rough cut on the generated speech clips. Needs `$SHOT_FIXTURES` and the models
//! (`shot models fetch`), so it is ignored by default:
//! `cargo test --release -p shot-analyze --test autocut -- --ignored --nocapture`

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use shot_analyze::autocut::{self, TakeFacts};
use shot_analyze::{Speech, SpeechDetector, Transcriber, decode_mono, models};
use shot_core::{Cut, FormatKind, Id, Micros, RangeEdit, TimeRange, Transcript};

fn clip(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
        .join("speech")
        .join(format!("{name}.wav"))
}

/// Streams a clip in 10 ms chunks into both detectors, as capture delivers it.
fn analyze(wav: &Path) -> (Transcript, Speech, Micros) {
    let dir = models::models_dir();
    let vad = models::vad_model().path(&dir);
    let transcriber = Transcriber::new(
        &models::whisper_model().path(&dir),
        &vad,
        Id::new("t01").unwrap(),
        |_| {},
    )
    .unwrap();
    let detector = SpeechDetector::new(&vad).unwrap();
    let duration = decode_mono(wav, |t, samples, rate| {
        let chunk = rate as usize / 100;
        for (i, part) in samples.chunks(chunk).enumerate() {
            let at = Micros(t.0 + i as u64 * chunk as u64 * 1_000_000 / u64::from(rate));
            transcriber.feed(at, part, rate);
            detector.feed(at, part, rate);
        }
    })
    .unwrap();
    (
        transcriber.finish().unwrap(),
        detector.finish().unwrap(),
        duration,
    )
}

/// The words the rough cut keeps, by their midpoint.
fn kept_text(cut: &Cut, transcript: &Transcript) -> String {
    let ranges = &cut.chapters[0].takes[0].ranges;
    transcript
        .words()
        .iter()
        .filter(|w| {
            let middle = Micros((w.range.from().0 + w.range.to().0) / 2);
            ranges.iter().any(|piece| {
                matches!(piece, RangeEdit::Keep(r) if r.from() <= middle && middle < r.to())
            })
        })
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
#[ignore = "needs $SHOT_FIXTURES and the models"]
fn the_rough_cut_keeps_the_clean_version_of_each_clip() {
    let format = FormatKind::Opinion.preset();
    let expected = [
        ("clean", None),
        ("filler", None),
        ("long-silence", None),
        ("mid-pause", None),
        (
            "retake",
            Some(
                "Welcome back. The first thing we do is create a new project from the tutorial preset.",
            ),
        ),
        (
            "self-correction",
            Some("the build ran at 13.35, and it finished in two seconds."),
        ),
    ];
    for (name, clean) in expected {
        let (transcript, speech, duration) = analyze(&clip(name));
        let empty = Cut::default().append_take(
            Id::new("t01").unwrap(),
            "Take",
            TimeRange::new(Micros(0), duration).unwrap(),
        );
        let facts = TakeFacts {
            transcript: &transcript,
            speech: &speech,
            still: &[],
            again: &[],
            duration,
        };
        let cut = autocut::rough_cut(&empty, &format, &facts, &HashMap::new());
        let counts: Vec<String> = autocut::rule_counts(&cut)
            .iter()
            .filter(|c| c.removed + c.queued > 0)
            .map(|c| {
                format!(
                    "{} {} removed ({:.2} s), {} queued",
                    c.rule,
                    c.removed,
                    c.removed_time.0 as f64 / 1e6,
                    c.queued
                )
            })
            .collect();
        let kept = kept_text(&cut, &transcript);
        println!(
            "{name}: {}\n  kept: {kept}\n{}",
            counts.join("; "),
            cut.to_kdl()
        );
        if let Some(clean) = clean {
            assert_eq!(kept, clean, "{name}");
        }
    }
}
