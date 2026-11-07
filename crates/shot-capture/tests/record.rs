//! Real recordings. They need a display, Screen Recording and Input Monitoring permission, and
//! the mic test also Microphone permission, so they are ignored by default:
//! `cargo test -p shot-capture -- --ignored`.

#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use shot_capture::{Session, SessionConfig, Source};

/// Top-level box types of an MP4 file.
fn boxes(path: &std::path::Path) -> Vec<String> {
    let data = std::fs::read(path).unwrap();
    let mut at = 0;
    let mut found = Vec::new();
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        found.push(String::from_utf8_lossy(&data[at + 4..at + 8]).into_owned());
        at += size.max(8);
    }
    found
}

#[test]
#[ignore = "records the main display"]
fn a_take_is_fragmented_mp4_with_a_fresh_id() {
    let video = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(video.path().join("media/t04")).unwrap();
    let mut config = SessionConfig::new(video.path());
    config.camera = Source::Off;
    config.mic = Source::Off;
    let session = Session::start(config).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let take = session.stop().unwrap();

    assert_eq!(take.id.as_str(), "t05");
    let screen = boxes(&take.dir.join("screen.mp4"));
    assert_eq!(screen[..3], ["ftyp", "moov", "moof"]);
    assert!(screen.iter().filter(|b| *b == "moof").count() >= 3);
    assert_eq!(boxes(&take.dir.join("system.mp4"))[..2], ["ftyp", "moov"]);
    assert!(take.dir.join("events.jsonl").is_file());
}

#[test]
#[ignore = "records the default mic"]
fn the_mic_tap_hands_over_mono_samples_on_the_media_clock() {
    let video = tempfile::tempdir().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut config = SessionConfig::new(video.path());
    config.camera = Source::Off;
    config.system_audio = false;
    config.mic_tap = Some(Box::new({
        let received = received.clone();
        move |at, samples, rate| received.lock().unwrap().push((at, samples.len(), rate))
    }));
    let session = Session::start(config).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    session.stop().unwrap();

    let received = received.lock().unwrap();
    let total: usize = received.iter().map(|(_, n, _)| n).sum();
    assert!(received.iter().all(|(_, _, rate)| *rate == 48_000));
    assert!(received.windows(2).all(|w| w[0].0 < w[1].0));
    assert!(
        (80_000..=110_000).contains(&total),
        "{total} samples in 2 s"
    );
}
