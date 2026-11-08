//! The mic audio of an output, cut like its video: each segment's samples of its take, faded
//! over 5 ms where the source jumps. Export encodes it, the preview plays it, so both cut the
//! same samples and fade them the same way. Bleeps silence the voice over their range; the
//! export puts the tone in after the voice chain.
//!
//! A plan with bookends and transitions ([`TimelineAudio::of_plan`]) is silent under the
//! bookends, and its voice follows the picture at each transition: a dip fades out to the cut
//! and in after it, every other transition crossfades on equal-power curves over its middle,
//! with the outgoing take running on past its edge and the incoming one starting early, as the
//! picture's handles do. A stinger crosses over a quarter second around its cut.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::ops::Range;
use std::path::PathBuf;

use shot_core::{Id, TransitionKind};
use shot_media::audio::{AudioError, PcmReader, SAMPLE_RATE};

use crate::join::Piece;
use crate::plan::ExportPlan;

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

/// How the voice joins two pieces at a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blend {
    /// The outgoing side fades out to the cut, the incoming one in after it.
    Dip,
    /// Both sides play across the middle, one fading out as the other fades in.
    Cross,
}

/// A transition's voice, in output samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Join {
    edge: u64,
    /// From the edge to each end of the blend.
    half: u64,
    blend: Blend,
    /// The segments on each side, `None` for a bookend.
    before: Option<usize>,
    after: Option<usize>,
}

impl Join {
    /// The outgoing and incoming gains at output sample `n`.
    fn gains(&self, n: u64) -> (f32, f32) {
        let start = self.edge - self.half;
        match self.blend {
            Blend::Cross => {
                let x = ((n - start) as f32 + 0.5) / (2 * self.half) as f32 * FRAC_PI_2;
                (x.cos(), x.sin())
            }
            Blend::Dip if n < self.edge => {
                let x = ((n - start) as f32 + 0.5) / self.half as f32 * FRAC_PI_2;
                (x.cos(), 0.0)
            }
            Blend::Dip => {
                let x = ((n - self.edge) as f32 + 0.5) / self.half as f32 * FRAC_PI_2;
                (0.0, x.sin())
            }
        }
    }
}

/// Reads output samples of a sequence of segments, from each take's mic track.
pub struct TimelineAudio {
    segments: Vec<Placed>,
    track: Box<dyn Fn(&Id) -> PathBuf>,
    readers: HashMap<Id, PcmReader>,
    /// Read the material past the segments' edges, so the segments' readers read on.
    handles: HashMap<Id, PcmReader>,
    scratch: Vec<f32>,
    /// Output samples the voice is silenced over, in order.
    bleeps: Vec<Range<u64>>,
    joins: Vec<Join>,
    /// Output length in samples.
    length: u64,
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
            length: placed.last().map_or(0, |s| s.output.end),
            segments: placed,
            track: Box::new(track),
            readers: HashMap::new(),
            handles: HashMap::new(),
            scratch: Vec::new(),
            bleeps: Vec::new(),
            joins: Vec::new(),
        }
    }

    /// The voice of `plan` at its frame rate, which must divide [`SAMPLE_RATE`]: each segment
    /// where the plan places it, silence under the bookends, and the transitions' blends.
    pub fn of_plan(plan: &ExportPlan, track: impl Fn(&Id) -> PathBuf + 'static) -> Self {
        let per_frame = u64::from(SAMPLE_RATE / plan.fps);
        let mut placed: Vec<Placed> = Vec::with_capacity(plan.segments.len());
        let mut previous: Option<Piece> = None;
        for piece in &plan.pieces {
            if let Piece::Segment(i) = *piece {
                let segment = &plan.segments[i];
                let continues = previous.is_some_and(|p| {
                    p == Piece::Segment(i.wrapping_sub(1)) && segment.continues_previous
                });
                if let Some(last) = placed.last_mut().filter(|_| continues) {
                    last.fade_out = false;
                }
                let start = segment.output_frame * per_frame;
                placed.push(Placed {
                    take: segment.segment.take.clone(),
                    source_start: segment.source_frames.start * per_frame,
                    output: start..start + segment.frames() * per_frame,
                    fade_in: !continues,
                    fade_out: true,
                });
            }
            previous = Some(*piece);
        }
        let index = |piece| match piece {
            Piece::Segment(i) => Some(i),
            Piece::Bookend(_) => None,
        };
        let mut joins = Vec::new();
        for t in &plan.transitions {
            let (before, after) = (index(t.before), index(t.after));
            // A join inside one take's continuous material has nothing to blend.
            if let (Some(b), Some(a)) = (before, after)
                && a == b + 1
                && plan.segments[a].continues_previous
            {
                continue;
            }
            let (blend, half) = match t.kind {
                TransitionKind::Cut => continue,
                TransitionKind::Dip => (Blend::Dip, t.edge - t.window.start),
                TransitionKind::Stinger => (
                    Blend::Cross,
                    (t.edge - t.window.start)
                        .min(t.window.end - t.edge)
                        .min(u64::from(plan.fps / 4)),
                ),
                _ => (Blend::Cross, t.edge - t.window.start),
            };
            if half == 0 {
                continue;
            }
            if blend == Blend::Cross {
                if let Some(b) = before {
                    placed[b].fade_out = false;
                }
                if let Some(a) = after {
                    placed[a].fade_in = false;
                }
            }
            joins.push(Join {
                edge: t.edge * per_frame,
                half: half * per_frame,
                blend,
                before,
                after,
            });
        }
        Self {
            segments: placed,
            track: Box::new(track),
            readers: HashMap::new(),
            handles: HashMap::new(),
            scratch: Vec::new(),
            bleeps: Vec::new(),
            joins,
            length: plan.frames * per_frame,
        }
    }

    /// Silences the voice over `bleeps`, ranges of the take's media time per take. Each range is
    /// placed wherever its take's segments show it.
    pub fn with_bleeps(mut self, bleeps: &[(Id, shot_core::TimeRange)]) -> Self {
        let per_second = u64::from(SAMPLE_RATE);
        let sample = |t: shot_core::Micros| t.0 * per_second / 1_000_000;
        let mut out: Vec<Range<u64>> = Vec::new();
        for segment in &self.segments {
            let source_end = segment.source_start + (segment.output.end - segment.output.start);
            for (_, range) in bleeps.iter().filter(|(take, _)| *take == segment.take) {
                let from = sample(range.from()).max(segment.source_start);
                let to = sample(range.to()).min(source_end);
                if from < to {
                    let shift = |s: u64| segment.output.start + s - segment.source_start;
                    out.push(shift(from)..shift(to));
                }
            }
        }
        out.sort_by_key(|r| r.start);
        self.bleeps = out;
        self
    }

    /// Output samples the voice is silenced over, in order.
    pub fn bleeps(&self) -> &[Range<u64>] {
        &self.bleeps
    }

    /// Output length in samples.
    pub fn len(&self) -> u64 {
        self.length
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
            if at < segment.output.start {
                let gap = segment.output.start.min(range.end);
                out.resize(out.len() + (gap - at) as usize, 0.0);
                at = gap;
            }
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
        let first = out.len() - (range.end - range.start) as usize;
        self.blend(range.clone(), &mut out[first..])?;
        for bleep in &self.bleeps {
            let from = bleep.start.max(range.start);
            let to = bleep.end.min(range.end);
            if from < to {
                let at = first + (from - range.start) as usize;
                out[at..at + (to - from) as usize].fill(0.0);
            }
        }
        Ok(())
    }
}

impl TimelineAudio {
    /// Applies the joins' gains to `out`, output samples `range`, and adds the handles of
    /// crossfades: the outgoing take after its edge, the incoming one before its edge.
    fn blend(&mut self, range: Range<u64>, out: &mut [f32]) -> Result<(), TimelineAudioError> {
        for join in self.joins.clone() {
            let from = range.start.max(join.edge - join.half);
            let to = range.end.min(join.edge + join.half);
            if from >= to {
                continue;
            }
            for n in from..to {
                let (before, after) = join.gains(n);
                out[(n - range.start) as usize] *= if n < join.edge { before } else { after };
            }
            if join.blend == Blend::Dip {
                continue;
            }
            if let Some(b) = join.before
                && to > join.edge
            {
                let segment = self.segments[b].clone();
                let source_end = segment.source_start + (segment.output.end - segment.output.start);
                let start = from.max(join.edge);
                let samples =
                    self.handle(&segment.take, source_end + (start - join.edge), to - start)?;
                for (i, s) in samples.iter().enumerate() {
                    let n = start + i as u64;
                    out[(n - range.start) as usize] += s * join.gains(n).0;
                }
            }
            if let Some(a) = join.after
                && from < join.edge
            {
                let segment = self.segments[a].clone();
                let end = to.min(join.edge);
                // The take has nothing before its start: the handle is silence there.
                let early = join.edge - from;
                let skip = early.saturating_sub(segment.source_start);
                let samples = self.handle(
                    &segment.take,
                    segment.source_start.saturating_sub(early),
                    (end - from).saturating_sub(skip),
                )?;
                for (i, s) in samples.iter().enumerate() {
                    let n = from + skip + i as u64;
                    out[(n - range.start) as usize] += s * join.gains(n).1;
                }
            }
        }
        Ok(())
    }

    /// `count` samples of `take`'s mic from source sample `from`, through the handle reader.
    fn handle(&mut self, take: &Id, from: u64, count: u64) -> Result<Vec<f32>, TimelineAudioError> {
        let path = (self.track)(take);
        let reader = match self.handles.entry(take.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(PcmReader::open(&path).map_err(|source| TimelineAudioError {
                    path: path.clone(),
                    source,
                })?)
            }
        };
        let mut samples = Vec::with_capacity(count as usize);
        reader
            .read(from, from + count, &mut samples)
            .map_err(|source| TimelineAudioError { path, source })?;
        samples.resize(count as usize, 0.0);
        Ok(samples)
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

    /// A mono 16-bit WAV of `seconds` at a constant `level`.
    fn constant_wav(path: &std::path::Path, seconds: u32, level: f32) {
        let samples = SAMPLE_RATE * seconds;
        let mut bytes = Vec::new();
        let mut put = |b: &[u8]| bytes.extend_from_slice(b);
        put(b"RIFF");
        put(&(36 + samples * 2).to_le_bytes());
        put(b"WAVEfmt ");
        put(&16u32.to_le_bytes());
        put(&1u16.to_le_bytes());
        put(&1u16.to_le_bytes());
        put(&SAMPLE_RATE.to_le_bytes());
        put(&(SAMPLE_RATE * 2).to_le_bytes());
        put(&2u16.to_le_bytes());
        put(&16u16.to_le_bytes());
        put(b"data");
        put(&(samples * 2).to_le_bytes());
        let value = ((level * 32767.0) as i16).to_le_bytes();
        for _ in 0..samples {
            put(&value);
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn the_voice_rests_under_the_intro_and_crosses_over_a_dissolve() {
        use crate::join::{BookendInput, Join, Joins};
        use shot_core::{Cut, FormatKind, Micros, Project, Script, Target, Timeline, Video};

        let dir = tempfile::tempdir().unwrap();
        let mic = dir.path().join("mic.wav");
        constant_wav(&mic, 4, 0.5);
        let project = Project {
            video: Video::parse("title \"t\"\nstatus editing\nformat tutorial\n").unwrap(),
            script: Script::parse("# t\n").unwrap(),
            format: FormatKind::Tutorial.preset(),
            cut: Cut::parse(
                "chapter \"One\" {\n    take t01 {\n        keep from=0 to=1_000_000\n    }\n}\n\
                 chapter \"Two\" {\n    take t01 {\n        keep from=2_000_000 to=3_000_000\n    }\n}\n",
            )
            .unwrap(),
        };
        let joins = Joins {
            intro: Some(BookendInput {
                bumper: None,
                title: Some("t".into()),
                length: Micros(500_000),
                identity: Vec::new(),
            }),
            transitions: vec![Join {
                take: Id::new("t01").unwrap(),
                at: Micros(2_000_000),
                target: Target::ChapterStart,
                kind: TransitionKind::Dissolve,
                length: Micros(500_000),
            }],
            ..Joins::default()
        };
        let plan = crate::plan::ExportPlan::new(Timeline::evaluate(&project).main(), 60, |_| {
            shot_core::MediaId::default()
        })
        .with_joins(&joins);
        let mut audio = TimelineAudio::of_plan(&plan, move |_| mic.clone());
        assert_eq!(audio.len(), 2_500 * 48);
        let mut out = Vec::new();
        audio.read(0..audio.len(), &mut out).unwrap();
        let at = |ms: u64| out[(ms * 48) as usize];
        // Silent under the intro, then the voice faded in over 5 ms where it starts.
        assert_eq!(at(400), 0.0);
        assert!((at(800) - 0.5).abs() < 1e-3);
        // The edge is at 1.5 s: on equal-power curves both takes play across the middle, the
        // first one past its kept range and the second one before its own.
        assert!((at(1_260) - 0.5).abs() < 0.02);
        assert!((out[1_500 * 48] - 0.5 * std::f32::consts::SQRT_2).abs() < 1e-2);
        assert!((at(1_740) - 0.5).abs() < 0.02);
        assert!((at(2_000) - 0.5).abs() < 1e-3);
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
