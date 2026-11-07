#![cfg(target_os = "macos")]

use std::f32::consts::TAU;

use shot_ship::media::loudness;

const RATE: u32 = 48_000;

/// Mono 16-bit PCM WAV of `samples`.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

fn tone(seconds: u32, amplitude: f32) -> impl Iterator<Item = f32> {
    (0..RATE * seconds).map(move |i| amplitude * (TAU * 440.0 * i as f32 / RATE as f32).sin())
}

#[test]
fn digital_silence_inside_the_track_does_not_end_the_measurement() {
    let samples: Vec<f32> = tone(1, 0.1)
        .chain(std::iter::repeat_n(0.0, (RATE * 3) as usize))
        .chain(tone(1, 0.5))
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gap.wav");
    std::fs::write(&path, wav(&samples)).unwrap();

    let (_, true_peak) = loudness(&path).unwrap();
    // The loud tone after the gap peaks near -6 dBTP; stopping at the gap would report -20.
    assert!(true_peak > -7.0, "true peak {true_peak} dBTP");
}
