//! `shot analyze` on a small video folder, with a fake `claude` that answers the annotation
//! prompt.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use shot_analyze::{Speech, cache};
use shot_core::{Id, Micros, TimeRange, Transcript, Word};

const FAKE: &str = r#"#!/bin/sh
cat > stdin.txt
printf '%s\n' '{"type":"system","subtype":"init","model":"claude-test","tools":["mcp__shot__list_videos"]}'
printf '%s\n' '{"type":"result","subtype":"success","result":"{\"kind\":\"claim\",\"from\":0,\"confidence\":80}\n{\"kind\":\"topic-shift\",\"from\":1}"}'
"#;

fn analyze(video: &Path, claude: &Path, ask: bool) -> Vec<u8> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_shot"));
    command
        .args(["analyze", "--video"])
        .arg(video)
        .args(["--take", "t01"])
        .env("SHOT_CLAUDE", claude);
    if ask {
        command.arg("--claude");
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read(video.join("elements/t01.jsonl")).unwrap()
}

#[test]
fn rewrites_the_same_bytes_and_keeps_what_claude_found() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("video");
    let media = video.join("media/t01");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(
        media.join("events.jsonl"),
        "{\"t\":500000,\"kind\":\"down\",\"x\":10,\"y\":20,\"button\":\"left\"}\n",
    )
    .unwrap();
    let take = Id::new("t01").unwrap();
    let mut transcript = Transcript::new(take.clone());
    for (i, text) in ["We", "ship.", "On", "Fridays."].iter().enumerate() {
        let from = 1_000_000 + i as u64 * 400_000;
        transcript
            .push(Word {
                text: (*text).to_owned(),
                range: TimeRange::new(Micros(from), Micros(from + 300_000)).unwrap(),
                confidence: 90,
            })
            .unwrap();
    }
    cache::write_cached(&video, &transcript).unwrap();
    cache::write_speech(&video, &take, &Speech::default()).unwrap();
    let claude = dir.path().join("claude");
    std::fs::write(&claude, FAKE).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();

    let asked = analyze(&video, &claude, true);
    let text = String::from_utf8(asked.clone()).unwrap();
    assert!(
        text.contains(r#"{"kind":"claim","t":1000000,"to":1700000,"confidence":80,"by":"claude"}"#),
        "{text}"
    );
    assert!(text.contains(r#"{"kind":"topic-shift","t":1800000,"by":"claude"}"#));
    let sent = std::fs::read_dir(video.join(".shot/agent"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path().join("stdin.txt")).unwrap())
        .next()
        .unwrap();
    assert!(sent.ends_with("0 We ship.\n1 On Fridays.\n"), "{sent}");

    let missing = dir.path().join("no-claude");
    assert_eq!(analyze(&video, &missing, false), asked);
    assert_eq!(analyze(&video, &missing, false), asked);
}
