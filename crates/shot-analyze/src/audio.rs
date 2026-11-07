//! Decoding a recorded audio track (AAC in MP4, or WAV) to mono f32 PCM.

use std::fs::File;
use std::path::Path;

use shot_core::Micros;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::Error;

/// Calls `sink(t, samples, rate)` for each decoded packet, mixed down to mono, where `t` is the
/// media time of the first sample counted from the first decoded sample. Returns the duration.
pub fn decode_mono(
    path: &Path,
    mut sink: impl FnMut(Micros, &[f32], u32),
) -> Result<Micros, Error> {
    let decode = |e: SymphoniaError| Error::Decode(format!("{}: {e}", path.display()));
    let source = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(decode)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error::Decode(format!("{} has no audio track", path.display())))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| Error::Decode(format!("{} has no audio parameters", path.display())))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(decode)?;

    let mut frames = 0u64;
    let mut rate = 0;
    let mut interleaved = Vec::new();
    let mut mono = Vec::new();
    while let Some(packet) = format.next_packet().map_err(decode)? {
        if packet.track_id != track_id {
            continue;
        }
        let buffer = match decoder.decode(&packet) {
            Ok(buffer) => buffer,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(decode(e)),
        };
        rate = buffer.spec().rate();
        let channels = buffer.spec().channels().count().max(1);
        buffer.copy_to_vec_interleaved(&mut interleaved);
        mono.clear();
        mono.extend(
            interleaved
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
        sink(micros(frames, rate), &mono, rate);
        frames += mono.len() as u64;
    }
    Ok(micros(frames, rate))
}

fn micros(frames: u64, rate: u32) -> Micros {
    Micros(frames * 1_000_000 / u64::from(rate.max(1)))
}
