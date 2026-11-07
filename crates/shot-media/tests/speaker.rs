#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use shot_media::audio::SAMPLE_RATE;
use shot_media::speaker::Speaker;

#[test]
#[ignore = "needs an audio output device; plays half a second of silence"]
fn the_clock_follows_the_samples_the_device_plays() {
    let mut speaker = Speaker::open().expect("a default output device");
    let clock = speaker.clock();
    println!("output latency {:?}", speaker.latency());
    let half_second = vec![0.0; SAMPLE_RATE as usize / 2];
    assert_eq!(speaker.push(&half_second), half_second.len());
    assert_eq!(clock.audible(Duration::ZERO), None);
    speaker.play().expect("starting the device");
    let started = Instant::now();
    let first = loop {
        if let Some(sample) = clock.audible(Duration::ZERO) {
            break sample;
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the device never played"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    std::thread::sleep(Duration::from_millis(200));
    let later = clock.audible(Duration::ZERO).expect("still playing");
    let heard = (later - first) as f64 / f64::from(SAMPLE_RATE);
    assert!((0.17..0.23).contains(&heard), "200 ms played as {heard} s");

    // Past the queued samples the clock holds at the last one.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        clock.audible(Duration::ZERO),
        Some(half_second.len() as u64)
    );
    speaker.clear();
    assert_eq!(clock.audible(Duration::ZERO), None);
}
