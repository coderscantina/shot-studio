//! Frame sampling of the generated 30 s screen fixture, 0.5 s GOP. Needs `SHOT_FIXTURES`:
//! `cargo test -p shot-ship --test frames -- --ignored`.
#![cfg(target_os = "macos")]

use std::path::PathBuf;

use shot_core::Micros;
use shot_ship::media::{MediaError, sample_frames};

fn screen() -> PathBuf {
    PathBuf::from(std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
        .join("screen-30s.mp4")
}

#[test]
#[ignore = "needs the generated screen fixture"]
fn frames_are_sampled_on_the_keyframe_of_every_half_second() {
    let samples = sample_frames(&screen(), Micros(30_000_000), &|| false).unwrap();
    let at: Vec<u64> = samples.iter().map(|s| s.at.0).collect();
    let half_seconds: Vec<u64> = (0..60).map(|k| k * 500_000).collect();
    assert_eq!(at, half_seconds);
}

#[test]
#[ignore = "needs the generated screen fixture"]
fn sampling_stops_when_cancelled() {
    let result = sample_frames(&screen(), Micros(30_000_000), &|| true);
    assert!(matches!(result, Err(MediaError::Cancelled)));
}
