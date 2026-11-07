//! The mic audio of an output, cut like its video: each segment's samples of its take, faded
//! over 5 ms where the source jumps. Export encodes it, the preview plays it, so both cut the
//! same samples and fade them the same way.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use shot_core::Id;
use shot_media::audio::{AudioError, PcmReader, SAMPLE_RATE};

/// Fade length where the audio jumps, 5 ms.
pub const FADE_SAMPLES: u64 = SAMPLE_RATE as u64 / 200;

/// One segment of the output: frames of a take at the output frame rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSegment {
    pub take: Id,
    pub source_frames: Range<u64>,
}

#[derive(Debug, thiserror::Error)]
#[error("{path}: {source}")]
pub struct TimelineAudioError {
    pub path: PathBuf,
    pub source: AudioError,
}

/// A segment placed in the output, in samples.
#[derive(Debug, Clone)]
struct Placed {
    take: Id,
    source_start: u64,
    output: Range<u64>,
    fade_in: bool,
    fade_out: bool,
}

/// Reads output samples of a sequence of segments, from each take's mic track.
pub struct TimelineAudio {
    segments: Vec<Placed>,
    track: Box<dyn Fn(&Id) -> PathBuf>,
    readers: HashMap<Id, PcmReader>,
    scratch: Vec<f32>,
}

impl TimelineAudio {
    /// `segments` in output order at `fps`, which must divide [`SAMPLE_RATE`]. `track` names the
    /// mic file of a take. A segment that starts where the one before ends in the same take
    /// runs on without a fade.
    pub fn new(
        segments: impl IntoIterator<Item = AudioSegment>,
        fps: u32,
        track: impl Fn(&Id) -> PathBuf + 'static,
    ) -> Self {
        let per_frame = u64::from(SAMPLE_RATE / fps);
        let mut placed: Vec<Placed> = Vec::new();
        let mut previous: Option<AudioSegment> = None;
        let mut at = 0;
        for segment in segments {
            let length = (segment.source_frames.end - segment.source_frames.start) * per_frame;
            let continues = previous.as_ref().is_some_and(|p| {
                p.take == segment.take && p.source_frames.end == segment.source_frames.start
            });
            if let Some(last) = placed.last_mut() {
                last.fade_out = !continues;
            }
            placed.push(Placed {
                take: segment.take.clone(),
                source_start: segment.source_frames.start * per_frame,
                output: at..at + length,
                fade_in: !continues,
                fade_out: true,
            });
            at += length;
            previous = Some(segment);
        }
        Self {
            segments: placed,
            track: Box::new(track),
            readers: HashMap::new(),
            scratch: Vec::new(),
        }
    }

    /// Output length in samples.
    pub fn len(&self) -> u64 {
        self.segments.last().map_or(0, |s| s.output.end)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends output samples `range` to `out`, silence past the end. Reading forward continues
    /// decoding; reading backward restarts a take's reader.
    pub fn read(
        &mut self,
        range: Range<u64>,
        out: &mut Vec<f32>,
    ) -> Result<(), TimelineAudioError> {
        let first = self
            .segments
            .partition_point(|s| s.output.end <= range.start);
        let mut at = range.start;
        for segment in &self.segments[first..] {
            if at >= range.end {
                break;
            }
            let end = range.end.min(segment.output.end);
            let offset = at - segment.output.start;
            let source = segment.source_start + offset;
            let path = (self.track)(&segment.take);
            let reader = match self.readers.entry(segment.take.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(PcmReader::open(&path).map_err(|source| TimelineAudioError {
                        path: path.clone(),
                        source,
                    })?)
                }
            };
            self.scratch.clear();
            reader
                .read(source, source + (end - at), &mut self.scratch)
                .map_err(|source| TimelineAudioError { path, source })?;
            let length = segment.output.end - segment.output.start;
            let start = out.len();
            out.extend_from_slice(&self.scratch);
            fade(
                &mut out[start..],
                offset,
                length,
                segment.fade_in,
                segment.fade_out,
            );
            at = end;
        }
        out.resize(out.len() + (range.end - at) as usize, 0.0);
        Ok(())
    }
}

/// Fades `samples`, which start `offset` samples into a segment of `length`, over
/// [`FADE_SAMPLES`] at the edges asked for, or over half the segment if it is shorter.
fn fade(samples: &mut [f32], offset: u64, length: u64, fade_in: bool, fade_out: bool) {
    let ramp = FADE_SAMPLES.min(length / 2);
    let gain = |i: u64| (i as f32 + 0.5) / ramp as f32;
    for (i, sample) in samples.iter_mut().enumerate() {
        let position = offset + i as u64;
        if fade_in && position < ramp {
            *sample *= gain(position);
        }
        let from_end = length - 1 - position;
        if fade_out && from_end < ramp {
            *sample *= gain(from_end);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_ramp_only_the_cut_edges_they_are_asked_for() {
        let ramp = FADE_SAMPLES as usize;
        let mut samples = vec![1.0; 1000];
        fade(&mut samples, 0, 1000, true, false);
        assert!(samples[0] < 0.01);
        assert!(samples[ramp / 2] > 0.4 && samples[ramp / 2] < 0.6);
        assert_eq!(samples[ramp], 1.0);
        assert_eq!(samples[999], 1.0);

        let mut short = vec![1.0; 10];
        fade(&mut short, 0, 10, true, true);
        assert!(short[0] < 0.2 && short[9] < 0.2);
        assert!(short.iter().all(|&s| s <= 1.0));
    }

    #[test]
    fn only_jumps_in_the_source_fade() {
        let segment = |take: &str, frames: Range<u64>| AudioSegment {
            take: Id::new(take).unwrap(),
            source_frames: frames,
        };
        let audio = TimelineAudio::new(
            [
                segment("t01", 0..60),
                segment("t01", 60..90),
                segment("t01", 120..150),
                segment("t02", 150..180),
            ],
            60,
            |_| PathBuf::new(),
        );
        let edges: Vec<_> = audio
            .segments
            .iter()
            .map(|s| (s.output.clone(), s.fade_in, s.fade_out))
            .collect();
        assert_eq!(
            edges,
            [
                (0..48_000, true, false),
                (48_000..72_000, false, true),
                (72_000..96_000, true, true),
                (96_000..120_000, true, true),
            ]
        );
        assert_eq!(audio.len(), 120_000);
    }

    #[test]
    fn a_fade_read_in_pieces_equals_the_fade_read_at_once() {
        let mut whole = vec![1.0; 600];
        fade(&mut whole, 0, 600, true, true);
        let mut pieces = Vec::new();
        for range in [0..7, 7..300, 300..597, 597..600] {
            let mut piece = vec![1.0; range.len()];
            fade(&mut piece, range.start as u64, 600, true, true);
            pieces.extend(piece);
        }
        assert_eq!(pieces, whole);
    }
}
