//! `shot ingest` on the three spoken takes of `fixtures/spec/takes-script.md`: the cut the
//! picks leave. Needs `$SHOT_FIXTURES` (`scripts/fixtures.sh takes`), the models
//! (`shot models fetch`) and ffmpeg, so it is ignored by default:
//! `cargo test -p shot-cli --test takes -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::process::Command;

use shot_core::{ElementKind, Id, RangeEdit};

/// `wav` over a test pattern, as a clip `shot ingest` takes.
fn clip(wav: &Path, to: &Path) {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-i"])
        .arg(wav)
        .args(["-shortest", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "aac"])
        .arg(to)
        .output()
        .expect("ffmpeg runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[ignore = "needs $SHOT_FIXTURES, the models and ffmpeg"]
fn three_takes_of_a_script_play_every_beat_once_with_the_clean_retakes() {
    let takes = PathBuf::from(std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
        .join("takes");
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("video");
    std::fs::create_dir_all(&video).unwrap();
    let file = |name: &str, text: &str| std::fs::write(video.join(name), text).unwrap();
    file(
        "video.kdl",
        "title \"A cli in rust\"\nstatus editing\nformat opinion\n",
    );
    file(
        "format.kdl",
        &shot_core::FormatKind::Opinion.preset().to_kdl(),
    );
    file("cut.kdl", "");
    std::fs::copy(takes.join("script.md"), video.join("script.md")).unwrap();
    // The whole script with two flubbed beats, the chapter again with a restart, then the two
    // beats said clean.
    for name in ["script-full", "script-chapter", "script-beats"] {
        let mp4 = dir.path().join(format!("{name}.mp4"));
        clip(&takes.join(format!("{name}.wav")), &mp4);
        let out = Command::new(env!("CARGO_BIN_EXE_shot"))
            .arg("ingest")
            .arg(&mp4)
            .arg("--video")
            .arg(&video)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let project = shot_mcp::style::open(&video).unwrap().project;
    let cut = &project.cut;
    println!("{}", cut.to_kdl());
    let blocks: Vec<(&str, Vec<&str>)> = cut
        .chapters
        .iter()
        .map(|c| {
            let takes = c.takes.iter().map(|edit| edit.take.as_str()).collect();
            (c.title.as_str(), takes)
        })
        .collect();
    assert_eq!(
        blocks,
        [
            ("Intro", vec!["t01"]),
            ("Arguments", vec!["t02", "t03"]),
            ("Colors", vec!["t01"]),
        ]
    );

    // Every beat plays one attempt, and the two flubbed beats the third take's.
    let lineup = shot_mcp::takes::lineup(&video, &project);
    let mut playing = Vec::new();
    for (_, row) in lineup.rows() {
        let kept: Vec<&str> = row
            .attempts
            .iter()
            .filter(|a| 2 * cut.kept_of(&a.take, a.range).0 > a.range.duration().0)
            .map(|a| a.take.as_str())
            .collect();
        assert_eq!(kept.len(), 1, "{}: {kept:?}", row.text);
        playing.push((row.text.split(' ').next().unwrap(), kept[0]));
    }
    assert_eq!(
        playing,
        [
            ("Welcome,", "t01"),
            ("By", "t01"),
            ("Create", "t02"),
            ("Add", "t03"),
            ("Clap", "t03"),
            ("Add", "t01"),
            ("Run", "t01"),
        ]
    );

    // No restart plays.
    for edit in cut.chapters.iter().flat_map(|c| &c.takes) {
        let elements = shot_mcp::takes::read_take(&video, &Id::new(edit.take.as_str()).unwrap())
            .and_then(|(_, elements)| elements)
            .expect("an ingested take has elements");
        let restarts: Vec<_> = elements
            .iter()
            .filter(|e| e.kind == ElementKind::Retake)
            .filter(|e| {
                edit.ranges
                    .iter()
                    .any(|r| matches!(r, RangeEdit::Keep(k) if k.from() <= e.t && e.t < k.to()))
            })
            .collect();
        assert!(restarts.is_empty(), "{}: {restarts:?}", edit.take);
    }
}
