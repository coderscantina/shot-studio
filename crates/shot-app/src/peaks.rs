//! Peaks of a take's mic track for the Voice lane, read once on a background thread.

use std::path::Path;

use shot_core::Micros;
use shot_media::audio::{PcmReader, SAMPLE_RATE};

/// Peaks per second of audio.
pub const RATE: u64 = 100;
const CHUNK: u64 = SAMPLE_RATE as u64 / RATE;

/// The loudest absolute sample of each 10 ms of the track, from 0 to 1.
pub struct Peaks(Vec<f32>);

impl Peaks {
    /// Reads `mic.mp4` up to `length`.
    pub fn read(path: &Path, length: Micros) -> Result<Self, String> {
        let mut reader = PcmReader::open(path).map_err(|e| e.to_string())?;
        let samples = length.0 * u64::from(SAMPLE_RATE) / 1_000_000;
        let mut peaks = Vec::with_capacity((samples / CHUNK + 1) as usize);
        let mut buffer = Vec::new();
        let mut at = 0;
        while at < samples {
            let to = (at + CHUNK * RATE).min(samples);
            buffer.clear();
            reader
                .read(at, to, &mut buffer)
                .map_err(|e| e.to_string())?;
            peaks.extend(
                buffer
                    .chunks(CHUNK as usize)
                    .map(|chunk| chunk.iter().fold(0f32, |peak, s| peak.max(s.abs()))),
            );
            at = to;
        }
        Ok(Self(peaks))
    }

    /// The peak over `from..to` of media time.
    pub fn over(&self, from: Micros, to: Micros) -> f32 {
        let index = |t: Micros| (t.0 * RATE / 1_000_000) as usize;
        let (start, end) = (index(from), index(to).max(index(from) + 1));
        self.0
            .get(start.min(self.0.len())..end.min(self.0.len()))
            .map_or(0.0, |peaks| peaks.iter().fold(0f32, |a, b| a.max(*b)))
    }
}
