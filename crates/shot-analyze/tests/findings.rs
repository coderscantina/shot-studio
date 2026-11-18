//! The retake and misspoken detectors on the generated speech clips. Needs `$SHOT_FIXTURES` and
//! the models (`shot models fetch`), so it is ignored by default:
//! `cargo test --release -p shot-analyze --test findings -- --ignored --nocapture`

use std::path::{Path, PathBuf};

use shot_analyze::elements::findings::{self, TakeFacts, Voice};
use shot_analyze::{Speech, SpeechDetector, Transcriber, decode_mono, models};
use shot_core::{ElementKind, Id, Micros, Transcript, Word};

/// What a cut applies right away.
const SURE: u8 = 85;

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

#[test]
#[ignore = "needs $SHOT_FIXTURES and the models"]
fn sure_findings_leave_the_clean_version_of_each_clip() {
    let expected = [
        (
            "retake",
            "Welcome back. The first thing we do is create a new project from the tutorial preset.",
        ),
        (
            "self-correction",
            "the build ran at 13.35, and it finished in two seconds.",
        ),
    ];
    for (name, clean) in expected {
        let (transcript, speech, duration) = analyze(&clip(name));
        let facts = TakeFacts {
            transcript: &transcript,
            speech: &speech,
            still: &[],
            again: &[],
            duration,
        };
        let sure: Vec<_> = findings::find(&facts, &Voice::default(), SURE, Micros(u64::MAX))
            .into_iter()
            .filter(|f| f.confidence >= SURE)
            .collect();
        let kept: Vec<&str> = transcript
            .words()
            .iter()
            .filter(|w| {
                let middle = Micros((w.range.from().0 + w.range.to().0) / 2);
                !sure
                    .iter()
                    .any(|f| f.range.from() <= middle && middle < f.range.to())
            })
            .map(|w| w.text.as_str())
            .collect();
        assert_eq!(kept.join(" "), clean, "{name}");
    }
}

/// The words a finding removes, as they were said.
fn said(words: &[Word], finding: &findings::Finding) -> String {
    words
        .iter()
        .filter(|w| {
            let middle = Micros((w.range.from().0 + w.range.to().0) / 2);
            finding.range.from() <= middle && middle < finding.range.to()
        })
        .map(|w| w.text.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
#[ignore = "needs $SHOT_FIXTURES and the models"]
fn one_take_finds_its_restarts_and_cue_and_asks_about_emphasis() {
    let wav = PathBuf::from(std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
        .join("takes/one-take.wav");
    let (transcript, speech, duration) = analyze(&wav);
    let facts = TakeFacts {
        transcript: &transcript,
        speech: &speech,
        still: &[],
        again: &[],
        duration,
    };
    let words = transcript.words();
    let retakes: Vec<(String, u8)> =
        findings::find(&facts, &Voice::default(), SURE, Micros(u64::MAX))
            .iter()
            .filter(|f| f.kind == ElementKind::Retake)
            .map(|f| (said(words, f).to_lowercase(), f.confidence))
            .collect();
    println!("{retakes:#?}");
    let sure = |needle: &str| {
        retakes
            .iter()
            .any(|(text, confidence)| text.contains(needle) && *confidence >= SURE)
    };
    let doubled = retakes
        .iter()
        .filter(|(text, confidence)| text.trim_matches(',') == "the" && *confidence >= SURE)
        .count();
    assert_eq!(doubled, 2, "both doubled words: {retakes:?}");
    assert!(sure("this makes the project"), "{retakes:?}");
    assert!(sure("let me say that again"), "{retakes:?}");
    assert!(
        sure("it blocks"),
        "the first attempt goes with the cue: {retakes:?}"
    );
    assert!(
        retakes
            .iter()
            .filter(|(text, _)| text.contains("very"))
            .all(|(_, confidence)| *confidence < SURE),
        "{retakes:?}"
    );
    assert!(shot_analyze::elements::urls(words).is_empty());
}
