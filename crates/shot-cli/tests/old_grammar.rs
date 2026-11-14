//! A video with a lock whose `cut.kdl` still holds a node from before style rules: `shot`
//! refuses it and says it has to be fixed by hand.

#![cfg(target_os = "macos")]

use std::process::Command;

#[test]
fn export_refuses_a_rule_removal_and_says_to_fix_it_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path();
    let write = |file: &str, text: &str| std::fs::write(video.join(file), text).unwrap();
    write("video.kdl", "title \"A\"\nstatus editing\nformat opinion\n");
    write("script.md", "# A\n");
    write("format.kdl", "");
    write("cut.lock", "lock 1\n");
    write(
        "cut.kdl",
        "chapter \"A\" {\n    take t01 {\n        keep from=0 to=4_000_000\n        remove from=4_000_000 to=5_000_000 by=silence\n    }\n}\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_shot"))
        .args(["export", "--video"])
        .arg(video)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cut.kdl:4:9: `remove by=silence` is from before style rules, and a video with a cut.lock is not migrated again: remove the node by hand"),
        "{stderr}"
    );
}
