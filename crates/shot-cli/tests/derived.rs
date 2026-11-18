//! `shot derived remove` composes the main cut that the removed Short kept from composing.

use std::process::Command;

use shot_core::{Cut, Id, Micros, TimeRange, Transcript, Word};

/// Writes the transcript of `take`: a word of `text` each second.
fn heard(dir: &std::path::Path, take: &str, text: &str) {
    let mut transcript = Transcript::new(Id::new(take).unwrap());
    for (i, word) in text.split(' ').enumerate() {
        let from = i as u64 * 1_000_000;
        transcript
            .push(Word {
                text: word.into(),
                range: TimeRange::new(Micros(from), Micros(from + 800_000)).unwrap(),
                confidence: 90,
            })
            .unwrap();
    }
    shot_analyze::cache::write_cached(dir, &transcript).unwrap();
}

#[test]
fn removing_the_short_that_kept_the_cut_from_composing_composes_it() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path();
    let file = |name: &str, text: &str| std::fs::write(video.join(name), text).unwrap();
    file(
        "video.kdl",
        "title \"A\"\nstatus recorded\nformat opinion\n",
    );
    file("script.md", "# A\n\n## Hook\n\n- Hello there world.\n");
    file(
        "format.kdl",
        &shot_core::FormatKind::Opinion.preset().to_kdl(),
    );
    // The second take says the one beat again, and the Short uses the first.
    file(
        "cut.kdl",
        "chapter \"Take t01\" {\n    take t01 {\n        keep from=0 to=3_000_000\n    }\n}\n\nchapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=3_000_000\n    }\n}\n\nshort hello {\n    clip t01 from=0 to=3_000_000\n}\n",
    );
    for take in ["t01", "t02"] {
        std::fs::create_dir_all(video.join("media").join(take)).unwrap();
        heard(video, take, "Hello there world.");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_shot"))
        .args(["derived", "remove", "hello", "--video"])
        .arg(video)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cut = Cut::parse(&std::fs::read_to_string(video.join("cut.kdl")).unwrap()).unwrap();
    let blocks: Vec<&str> = cut
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .map(|edit| edit.take.as_str())
        .collect();
    assert_eq!(blocks, ["t02"], "the newer attempt plays the beat");
    assert!(cut.derived.is_empty());
}
