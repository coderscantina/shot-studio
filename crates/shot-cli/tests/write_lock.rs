//! Two writers of one video folder: `shot` waits for the writer lock another process holds, so
//! both edits land.

use std::process::Command;
use std::time::Duration;

use shot_core::{CaptionFix, Cut};

#[test]
fn a_second_writer_waits_and_both_edits_land() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path();
    let cut_path = video.join("cut.kdl");
    std::fs::write(&cut_path, "").unwrap();

    // The first start of a freshly linked binary waits for macOS to check it.
    let warm = Command::new(env!("CARGO_BIN_EXE_shot"))
        .arg("--help")
        .output();
    assert!(warm.unwrap().status.success());

    let lock = shot_fs::WriteLock::acquire(video).unwrap();
    let mut cut = Cut::parse(&std::fs::read_to_string(&cut_path).unwrap()).unwrap();
    let shot = Command::new(env!("CARGO_BIN_EXE_shot"))
        .args(["caption-fix", "--video"])
        .arg(video)
        .args(["kubernetes", "Kubernetes"])
        .spawn()
        .unwrap();
    // Without the lock `shot` would read and write the file in this time, and the write below
    // would drop its fix.
    std::thread::sleep(Duration::from_millis(500));
    cut.caption_fixes.push(CaptionFix {
        heard: "shot studio".into(),
        said: "Shot Studio".into(),
    });
    shot_fs::write(&cut_path, cut.to_kdl()).unwrap();
    drop(lock);
    let out = shot.wait_with_output().unwrap();
    assert!(out.status.success());

    let cut = Cut::parse(&std::fs::read_to_string(&cut_path).unwrap()).unwrap();
    let heard: Vec<&str> = cut.caption_fixes.iter().map(|f| f.heard.as_str()).collect();
    assert_eq!(heard, ["shot studio", "kubernetes"]);
}
