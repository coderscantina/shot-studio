//! The finished voice of the generated speech clips, alone and with a ducked bundled bed and a
//! pop every 1.5 s, encoded to AAC like an export and decoded again, stays under the true-peak
//! ceiling at the target loudness. Needs `$SHOT_FIXTURES`:
//! `cargo test --release -p shot-render --test true_peak -- --ignored --nocapture`. Set
//! `SHOT_TRUE_PEAK_OUT` to a folder to keep the encoded clips for other meters.
#![cfg(target_os = "macos")]

use std::io::Write;
use std::path::{Path, PathBuf};

use shot_audio::{CEILING_DBTP, Meter, TARGET_LUFS};
use shot_core::Id;
use shot_media::audio::{AacEncoder, AacStream, PcmReader, SAMPLE_RATE};
use shot_render::audio::{AudioSegment, TimelineAudio, TimelineAudioError};
use shot_render::voice::{self, Score};

const FPS: u32 = 60;
const BITRATE: u32 = 128_000;

fn speech() -> PathBuf {
    PathBuf::from(std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is set")).join("speech")
}

/// Samples of a file until a second of digital silence.
fn read_all(path: &Path) -> Vec<f32> {
    let mut reader = PcmReader::open(path).unwrap();
    let block = u64::from(SAMPLE_RATE);
    let mut out = Vec::new();
    loop {
        let at = out.len();
        reader.read(at as u64, at as u64 + block, &mut out).unwrap();
        if at > 0 && out[at..].iter().all(|s| *s == 0.0) {
            out.truncate(at);
            return out;
        }
    }
}

/// Raw AAC with ADTS headers, which AVFoundation and ffmpeg both read.
fn write_adts(path: &Path, stream: &AacStream) {
    let mut file = std::fs::File::create(path).unwrap();
    let mut offset = 0;
    for &size in &stream.sizes {
        let length = size as usize + 7;
        // AAC-LC, 48 kHz (index 3), mono, no CRC.
        let header = [
            0xff,
            0xf1,
            (1 << 6) | (3 << 2),
            (1 << 6) | (length >> 11) as u8,
            (length >> 3) as u8,
            ((length & 7) << 5) as u8 | 0x1f,
            0xfc,
        ];
        file.write_all(&header).unwrap();
        file.write_all(&stream.data[offset..offset + size as usize])
            .unwrap();
        offset += size as usize;
    }
}

/// A bundled bed under the whole clip and a pop at 0 dB every 1.5 s, as the lock places them.
fn scored(samples: u64) -> Score {
    use shot_audio::{Anchor, CROSSFADE, Music, MusicCue, Play, SfxCue, Track, beats, sound};
    let bed = sound::bundled("bed-calm").unwrap().decode();
    let beats = beats::detect(bed.samples());
    let pop = sound::bundled("pop").unwrap().decode();
    let step = u64::from(SAMPLE_RATE) * 3 / 2;
    Score {
        music: Music::new(
            vec![MusicCue {
                at: 0,
                play: Some(Play {
                    track: Track::new(bed, beats),
                    gain_db: -18.0,
                    duck_db: -12.0,
                }),
            }],
            CROSSFADE,
        ),
        sfx: (0..samples / step)
            .map(|i| SfxCue::new(pop.clone(), Anchor::Start(i * step), 0.0))
            .collect(),
        bumpers: Vec::new(),
    }
}

#[test]
#[ignore]
fn encoded_speech_stays_under_the_ceiling_at_the_target() {
    let keep = std::env::var_os("SHOT_TRUE_PEAK_OUT").map(PathBuf::from);
    let dir = tempfile::tempdir().unwrap();
    let mut clips: Vec<PathBuf> = std::fs::read_dir(speech())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    clips.sort();
    let mut failures = Vec::new();
    for (clip, with_score) in clips.iter().flat_map(|c| [(c, false), (c, true)]) {
        let stem = clip.file_stem().unwrap().to_string_lossy().into_owned();
        let name = if with_score {
            format!("{stem}-scored")
        } else {
            stem
        };
        let frames = read_all(clip).len() as u64 / u64::from(SAMPLE_RATE / FPS);
        let score = if with_score {
            scored(frames * u64::from(SAMPLE_RATE / FPS))
        } else {
            Score::default()
        };
        let take = Id::new("t01").unwrap();
        let mut audio = TimelineAudio::new(
            [AudioSegment {
                take: take.clone(),
                source_frames: 0..frames,
            }],
            FPS,
            {
                let clip = clip.clone();
                move |_| clip.clone()
            },
        );
        let mut encoder = AacEncoder::new(SAMPLE_RATE, BITRATE).unwrap();
        let finished = voice::finish(&mut audio, &score, |samples| {
            encoder.push(samples).unwrap();
            Ok::<_, TimelineAudioError>(())
        })
        .unwrap();
        let out = keep
            .as_deref()
            .unwrap_or(dir.path())
            .join(format!("{name}.aac"));
        write_adts(&out, &encoder.finish().unwrap());

        let mut meter = Meter::new();
        meter.add(&read_all(&out));
        let loudness = meter.loudness().unwrap();
        let peak = meter.true_peak();
        println!(
            "{name}: before encode {:.2} LUFS, encoded {loudness:.2} LUFS {peak:.2} dBTP",
            finished.after.unwrap(),
        );
        if peak > CEILING_DBTP || (loudness - TARGET_LUFS).abs() > 0.5 {
            failures.push(name);
        }
    }
    assert!(
        failures.is_empty(),
        "over the ceiling or off target: {failures:?}"
    );
}
