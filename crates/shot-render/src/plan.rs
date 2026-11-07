//! An evaluated output snapped to the frame grid: which source frames each segment renders, where
//! it lands in the output, and where chapters start. Pure, no IO.
//!
//! Snapping depends only on a segment's source range, never on its output position, so a cached
//! segment stays valid when the edit before it changes. Audio follows the same grid: at 48 kHz and
//! 60 fps a frame is exactly 800 samples, so audio and video can never drift apart.

use std::ops::Range;

use shot_core::{Micros, Output, Segment, TimeRange};

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
}

impl PlannedSegment {
    pub fn frames(&self) -> u64 {
        self.source_frames.end - self.source_frames.start
    }

    /// The punch-in at a source frame: the zoom covering that frame's start, or 1.
    pub fn zoom_at(&self, source_frame: u64, fps: u32) -> f32 {
        let time = frame_time(source_frame, fps);
        self.segment
            .zooms
            .iter()
            .find(|zoom| zoom.range.from() <= time && time < zoom.range.to())
            .map_or(1.0, |zoom| f32::from(zoom.scale) / 100.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChapter {
    pub title: String,
    pub output_frame: u64,
}

impl ExportPlan {
    /// Plans `output` at `fps`. Segments shorter than half a frame snap to nothing and are left
    /// out, and so are chapters left without frames.
    pub fn new(output: &Output, fps: u32) -> Self {
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
                segments.push(PlannedSegment {
                    segment: segment.clone(),
                    source_frames,
                    output_frame,
                    continues_previous,
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

/// The frames whose start falls in `range`, rounding each edge to the nearest frame.
pub fn snap(range: TimeRange, fps: u32) -> Range<u64> {
    let frame = |time: Micros| (time.0 * u64::from(fps) + 500_000) / 1_000_000;
    frame(range.from())..frame(range.to())
}

/// The media time where frame `frame` starts.
pub fn frame_time(frame: u64, fps: u32) -> Micros {
    Micros(frame * 1_000_000 / u64::from(fps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Cut, FormatKind, Project, Script, Timeline, Video};

    fn plan(cut: &str) -> ExportPlan {
        let project = Project {
            video: Video::parse("title \"t\"\nstatus editing\nformat opinion\n").unwrap(),
            script: Script::parse("# t\n").unwrap(),
            format: FormatKind::Opinion.preset(),
            cut: Cut::parse(cut).unwrap(),
        };
        ExportPlan::new(Timeline::evaluate(&project).main(), 60)
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
