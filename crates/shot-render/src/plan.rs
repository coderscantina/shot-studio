//! An evaluated output snapped to the frame grid: which source frames each segment renders, where
//! it lands in the output, and where chapters start. Pure, no IO.
//!
//! Each segment is cut into chunks, the unit of render caching: half-second windows on its take's
//! own frame grid, which is the recordings' keyframe grid, cut short at the segment's edges.
//! Snapping and chunking depend only on a segment's source range, never on its output position,
//! so deleting a word only re-keys the two chunks at the new edges, and every other chunk stays
//! valid even though it moved. Audio follows the same grid: at 48 kHz and 60 fps a frame is
//! exactly 800 samples, so audio and video can never drift apart.

use std::ops::Range;

use shot_core::{ContentHash, Id, MediaId, Micros, Output, Segment, TimeRange, Zoom};

/// Chunks per second of a take: one per keyframe interval of the recordings.
const CHUNKS_PER_SECOND: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPlan {
    pub fps: u32,
    pub segments: Vec<PlannedSegment>,
    pub chapters: Vec<PlannedChapter>,
    /// Output length in frames.
    pub frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedSegment {
    pub segment: Segment,
    /// Frames of the take, at the output frame rate, that this segment shows.
    pub source_frames: Range<u64>,
    /// The output frame the segment starts at.
    pub output_frame: u64,
    /// Whether the previous segment ends where this one starts in the same take, so the audio
    /// runs on without a cut.
    pub continues_previous: bool,
    /// The segment's frames on its take's chunk grid, in order.
    pub chunks: Vec<PlannedChunk>,
}

/// A cached piece of a segment: at most half a second of one take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChunk {
    /// Frames of the take, at the output frame rate.
    pub frames: Range<u64>,
    pub hash: ContentHash,
}

impl PlannedChunk {
    pub fn frame_count(&self) -> u64 {
        self.frames.end - self.frames.start
    }
}

impl PlannedSegment {
    pub fn frames(&self) -> u64 {
        self.source_frames.end - self.source_frames.start
    }

    /// The output frame where `chunk`, one of this segment's chunks, starts.
    pub fn output_frame_of(&self, chunk: &PlannedChunk) -> u64 {
        self.output_frame + chunk.frames.start - self.source_frames.start
    }

    /// The punch-in at a source frame: the zoom covering that frame's start, or 1.
    pub fn zoom_at(&self, source_frame: u64, fps: u32) -> f32 {
        zoom_at(&self.segment.zooms, source_frame, fps)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChapter {
    pub title: String,
    pub output_frame: u64,
}

impl ExportPlan {
    /// Plans `output` at `fps`, with `media` naming the recorded files of each take. Segments
    /// shorter than half a frame snap to nothing and are left out, and so are chapters left
    /// without frames.
    pub fn new(output: &Output, fps: u32, media: impl Fn(&Id) -> MediaId) -> Self {
        let mut segments: Vec<PlannedSegment> = Vec::new();
        let mut chapters = Vec::new();
        let mut frames = 0;
        for chapter in &output.chapters {
            let start = frames;
            for segment in &chapter.segments {
                let source_frames = snap(segment.source, fps);
                if source_frames.is_empty() {
                    continue;
                }
                let continues_previous = segments.last().is_some_and(|previous| {
                    previous.segment.take == segment.take
                        && previous.source_frames.end == source_frames.start
                });
                let output_frame = frames;
                frames += source_frames.end - source_frames.start;
                let media = media(&segment.take);
                let chunks = chunk_grid(source_frames.clone(), fps)
                    .map(|frames| PlannedChunk {
                        hash: output.chunk_hash(segment, media, fps, frames.clone()),
                        frames,
                    })
                    .collect();
                segments.push(PlannedSegment {
                    segment: segment.clone(),
                    source_frames,
                    output_frame,
                    continues_previous,
                    chunks,
                });
            }
            if frames > start {
                chapters.push(PlannedChapter {
                    title: chapter.title.clone(),
                    output_frame: start,
                });
            }
        }
        Self {
            fps,
            segments,
            chapters,
            frames,
        }
    }
}

/// `frames` cut at every multiple of the chunk length.
fn chunk_grid(frames: Range<u64>, fps: u32) -> impl Iterator<Item = Range<u64>> {
    let length = u64::from((fps / CHUNKS_PER_SECOND).max(1));
    let mut start = frames.start;
    std::iter::from_fn(move || {
        (start < frames.end).then(|| {
            let end = ((start / length + 1) * length).min(frames.end);
            let chunk = start..end;
            start = end;
            chunk
        })
    })
}

/// The frames whose start falls in `range`, rounding each edge to the nearest frame.
pub fn snap(range: TimeRange, fps: u32) -> Range<u64> {
    let frame = |time: Micros| (time.0 * u64::from(fps) + 500_000) / 1_000_000;
    frame(range.from())..frame(range.to())
}

/// The punch-in of `zooms` at a source frame: the zoom covering that frame's start, or 1.
pub fn zoom_at(zooms: &[Zoom], source_frame: u64, fps: u32) -> f32 {
    let time = frame_time(source_frame, fps);
    zooms
        .iter()
        .find(|zoom| zoom.range.from() <= time && time < zoom.range.to())
        .map_or(1.0, |zoom| f32::from(zoom.scale) / 100.0)
}

/// The media time where frame `frame` starts.
pub fn frame_time(frame: u64, fps: u32) -> Micros {
    Micros(frame * 1_000_000 / u64::from(fps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use shot_core::{Cut, FormatKind, Project, Script, Timeline, Video};

    fn plan(cut: &str) -> ExportPlan {
        let project = Project {
            video: Video::parse("title \"t\"\nstatus editing\nformat opinion\n").unwrap(),
            script: Script::parse("# t\n").unwrap(),
            format: FormatKind::Opinion.preset(),
            cut: Cut::parse(cut).unwrap(),
        };
        ExportPlan::new(Timeline::evaluate(&project).main(), 60, |_| {
            MediaId::default()
        })
    }

    #[test]
    fn segments_snap_to_the_nearest_frame_and_follow_each_other() {
        let plan = plan(
            r#"
chapter "One" {
    take t01 {
        keep from=0 to=1_000_000
        remove from=1_000_000 to=2_008_000 by=silence
        keep from=2_008_000 to=2_510_000
        keep from=2_510_000 to=2_515_000
    }
}
chapter "Two" {
    take t01 {
        keep from=2_515_000 to=3_000_000
        layout split from=2_800_000 to=3_000_000
    }
}
"#,
        );
        let ranges: Vec<_> = plan
            .segments
            .iter()
            .map(|s| {
                (
                    s.source_frames.clone(),
                    s.output_frame,
                    s.continues_previous,
                )
            })
            .collect();
        assert_eq!(
            ranges,
            [
                (0..60, 0, false),
                // 2.008 s is frame 120.48, 2.51 s is frame 150.6.
                (120..151, 60, false),
                // 2.51 s to 2.515 s is less than half a frame: left out.
                (151..168, 91, true),
                (168..180, 108, true),
            ]
        );
        assert_eq!(plan.frames, 120);
        let chapters: Vec<_> = plan
            .chapters
            .iter()
            .map(|c| (c.title.as_str(), c.output_frame))
            .collect();
        assert_eq!(chapters, [("One", 0), ("Two", 91)]);
    }

    #[test]
    fn deleting_a_word_re_keys_only_the_chunks_at_its_edges() {
        let keys = |plan: &ExportPlan| -> Vec<(Range<u64>, ContentHash)> {
            plan.segments
                .iter()
                .flat_map(|s| s.chunks.iter().map(|c| (c.frames.clone(), c.hash)))
                .collect()
        };
        let before = plan(
            r#"
chapter "One" {
    take t01 {
        keep from=0 to=10_000_000
    }
}
"#,
        );
        let after = plan(
            r#"
chapter "One" {
    take t01 {
        keep from=0 to=4_200_000
        remove from=4_200_000 to=4_600_000 by=manual
        keep from=4_600_000 to=10_000_000
    }
}
"#,
        );
        let before_keys = keys(&before);
        assert_eq!(before_keys.len(), 20);
        assert!(
            before_keys
                .iter()
                .all(|(frames, _)| frames.end - frames.start == 30)
        );
        let old: HashSet<ContentHash> = before_keys.iter().map(|(_, hash)| *hash).collect();
        let new: Vec<Range<u64>> = keys(&after)
            .into_iter()
            .filter(|(_, hash)| !old.contains(hash))
            .map(|(frames, _)| frames)
            .collect();
        // 4.2 s is frame 252, 4.6 s is frame 276.
        assert_eq!(new, [240..252, 276..300]);
        let moved = &after.segments[1];
        assert_eq!(moved.output_frame_of(&moved.chunks[1]), 252 + 24);
    }

    #[test]
    fn zoom_applies_to_the_frames_it_covers() {
        let plan = plan(
            r#"
chapter "One" {
    take t01 {
        keep from=0 to=2_000_000
        zoom 115 from=500_000 to=1_000_000
    }
}
"#,
        );
        let segment = &plan.segments[0];
        assert_eq!(segment.zoom_at(29, 60), 1.0);
        assert_eq!(segment.zoom_at(30, 60), 1.15);
        assert_eq!(segment.zoom_at(59, 60), 1.15);
        assert_eq!(segment.zoom_at(60, 60), 1.0);
    }
}
