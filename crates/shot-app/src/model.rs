//! The Edit screen's view of a project, as pure functions: which output frame shows which take
//! block, the transcript as paragraphs of words and cuts, selections, layout keys and the
//! whole-video overview. Nothing here holds edit state; it is all derived from the files.

use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};
use std::path::Path;
use std::sync::Arc;

use shot_core::{
    Applied, Cut, EditError, Format, Id, Instance, Layout, Lock, MediaId, Micros, Overlay, Project,
    RangeEdit, RemovedBy, SceneSwitch, TakeAt, TakeEdit, TimeRange, Timeline, Transcript,
    WordState, Zoom,
};
use shot_render::plan::{self, ExportPlan, frame_time};

/// Output frame rate of preview and export.
pub const FPS: u32 = 60;

/// Auto cuts shorter than this show as a seam without a label.
const SEAM_MAX: Micros = Micros(1_000_000);
/// A pause this long between two words starts a new paragraph.
const PARAGRAPH_GAP: Micros = Micros(1_500_000);
/// After this many recorded words, a paragraph breaks at the next sentence end.
const PARAGRAPH_WORDS: usize = 60;
/// A paragraph without a sentence end breaks after this many recorded words.
const PARAGRAPH_WORDS_MAX: usize = 120;

/// Whether timecodes and the preview follow the edit or the whole recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Edited,
    Untrimmed,
}

/// What a view plays: the edit and its lock, or every block's material whole with the lock's
/// zooms and overlays but none of its cuts.
pub fn view_of(cut: &Cut, lock: &Lock, view: View) -> (Cut, Lock) {
    match view {
        View::Edited => (cut.clone(), lock.clone()),
        View::Untrimmed => {
            let mut whole = cut.clone();
            for block in whole.chapters.iter_mut().flat_map(|c| &mut c.takes) {
                let mut ranges: Vec<RangeEdit> = Vec::with_capacity(block.ranges.len());
                for range in block.ranges.drain(..).map(|r| r.range()) {
                    match ranges.last_mut() {
                        Some(RangeEdit::Keep(last)) if last.to() == range.from() => {
                            *last = TimeRange::new(last.from(), range.to()).expect("joined");
                        }
                        _ => ranges.push(RangeEdit::Keep(range)),
                    }
                }
                block.ranges = ranges;
            }
            let mut uncut = lock.clone();
            for take in &mut uncut.takes {
                take.instances
                    .retain(|i| !matches!(i.applied, Applied::Cut(_)));
            }
            (whole, uncut)
        }
    }
}

/// What the block loses, in order: a person's removals and the lock's cuts in its kept
/// material, each with who removed it. Where the lock's cuts overlap, the earlier one names
/// what they remove.
pub fn removals(edit: &TakeEdit, lock: &Lock) -> Vec<(TimeRange, RemovedBy)> {
    let mut all: Vec<(TimeRange, RemovedBy)> = edit
        .ranges
        .iter()
        .filter_map(|r| match *r {
            RangeEdit::Remove(range) => Some((range, RemovedBy::Manual)),
            RangeEdit::Keep(_) => None,
        })
        .collect();
    for instance in lock.take(&edit.take).iter().flat_map(|t| &t.instances) {
        let Applied::Cut(cut) = instance.applied else {
            continue;
        };
        for range in &edit.ranges {
            if let RangeEdit::Keep(kept) = range
                && let Some(inside) = kept.intersect(cut)
            {
                all.push((inside, RemovedBy::Rule(instance.element.target)));
            }
        }
    }
    all.sort_by_key(|(range, _)| range.from());
    let mut out: Vec<(TimeRange, RemovedBy)> = Vec::with_capacity(all.len());
    for (range, by) in all {
        let from = out
            .last()
            .map_or(range.from(), |(last, _)| range.from().max(last.to()));
        if let Some(range) = TimeRange::new(from, range.to()) {
            out.push((range, by));
        }
    }
    out
}

/// What of the block plays: its kept material less the lock's cuts, in order.
pub fn played(edit: &TakeEdit, lock: &Lock) -> Vec<TimeRange> {
    let removed = removals(edit, lock);
    let mut out = Vec::new();
    for range in &edit.ranges {
        let RangeEdit::Keep(kept) = *range else {
            continue;
        };
        let mut from = kept.from();
        for (cut, _) in removed.iter().filter(|(cut, _)| cut.overlaps(kept)) {
            out.extend(TimeRange::new(from, cut.from()));
            from = from.max(cut.to());
        }
        out.extend(TimeRange::new(from, kept.to()));
    }
    out
}

/// The cut as it plays: each block's kept material less the lock's cuts, which show as removed
/// ranges, for views that ask how much of a take plays.
pub fn as_played(cut: &Cut, lock: &Lock) -> Cut {
    let mut out = cut.clone();
    for block in out.chapters.iter_mut().flat_map(|c| &mut c.takes) {
        let mut ranges: Vec<RangeEdit> = played(block, lock)
            .into_iter()
            .map(RangeEdit::Keep)
            .chain(
                removals(block, lock)
                    .into_iter()
                    .map(|(r, _)| RangeEdit::Remove(r)),
            )
            .collect();
        ranges.sort_by_key(|r| r.range().from());
        block.ranges = ranges;
    }
    out
}

/// The block's zooms as the edit plays them: a person's and the lock's that play in it, in
/// order of their start.
pub fn zooms(edit: &TakeEdit, lock: &Lock) -> Vec<Zoom> {
    let mut zooms = edit.zooms.clone();
    zooms.extend(styled(edit, lock).filter_map(|i| match i.applied {
        Applied::Zoom {
            scale,
            range,
            center,
            shape,
        } => Some(Zoom {
            scale,
            range,
            center,
            shape,
        }),
        _ => None,
    }));
    zooms.sort_by_key(|z| z.range.from());
    zooms
}

/// The lock's instance of `take` that plays `zoom`.
pub fn zoom_instance<'a>(lock: &'a Lock, take: &Id, zoom: &Zoom) -> Option<&'a Instance> {
    lock.take(take)?.instances.iter().find(|i| {
        matches!(i.applied, Applied::Zoom { scale, range, center, shape }
            if scale == zoom.scale
                && range == zoom.range
                && center == zoom.center
                && shape == zoom.shape)
    })
}

/// The block's overlays in drawing order: a person's, then the lock's that play in it.
pub fn overlays(edit: &TakeEdit, lock: &Lock) -> Vec<Overlay> {
    let mut overlays = edit.overlays.clone();
    overlays.extend(styled(edit, lock).filter_map(|i| match &i.applied {
        Applied::Overlay {
            template,
            range,
            params,
        } => Some(Overlay {
            template: template.clone(),
            range: *range,
            params: params.clone(),
        }),
        _ => None,
    }));
    overlays
}

/// The lock's zooms and overlays of the block's take that play in its kept material, with the
/// instance behind each.
pub fn styled<'a>(edit: &'a TakeEdit, lock: &'a Lock) -> impl Iterator<Item = &'a Instance> {
    let plays = |range: TimeRange| {
        edit.ranges
            .iter()
            .any(|r| matches!(r, RangeEdit::Keep(kept) if kept.overlaps(range)))
    };
    lock.take(&edit.take)
        .into_iter()
        .flat_map(|t| &t.instances)
        .filter(move |i| match &i.applied {
            Applied::Zoom { range, .. } | Applied::Overlay { range, .. } => plays(*range),
            _ => false,
        })
}

/// Every take block of the main cut with its address, in output order.
pub fn blocks(cut: &Cut) -> impl Iterator<Item = (TakeAt, &shot_core::TakeEdit)> {
    cut.chapters.iter().enumerate().flat_map(|(chapter, c)| {
        c.takes
            .iter()
            .enumerate()
            .map(move |(take, edit)| (TakeAt { chapter, take }, edit))
    })
}

/// One planned segment, tied back to the take block it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub at: TakeAt,
    pub take: Id,
    pub layout: Layout,
    /// Frames of the take at [`FPS`].
    pub source_frames: Range<u64>,
    /// The output frame the span starts at.
    pub output_frame: u64,
    pub zooms: Vec<Zoom>,
    pub overlays: Vec<Overlay>,
    /// The layout switch whose window overlaps the span.
    pub switch: Option<SceneSwitch>,
}

impl Span {
    pub fn output_frames(&self) -> Range<u64> {
        self.output_frame..self.output_frame + (self.source_frames.end - self.source_frames.start)
    }

    /// The take's frame shown at output frame `frame`, which must lie in the span.
    pub fn source_frame(&self, frame: u64) -> u64 {
        self.source_frames.start + (frame - self.output_frame)
    }

    /// The punch-in at a source frame, 1 for none.
    pub fn zoom_at(&self, source_frame: u64) -> f32 {
        plan::zoom_at(&self.zooms, source_frame, FPS)
    }
}

/// A chapter's place in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterSpan {
    pub title: String,
    /// Empty when everything in the chapter is removed.
    pub frames: Range<u64>,
}

/// Where every take block lands in one output, with the plan export draws it from.
#[derive(Debug, Clone, PartialEq)]
pub struct EditMap {
    pub spans: Vec<Span>,
    /// One per chapter of the cut, in order.
    pub chapters: Vec<ChapterSpan>,
    /// Output length in frames, bookends included.
    pub frames: u64,
    /// The plan the preview paints and plays: the main cut with its bookends and transitions
    /// in the edited view, the bare segments in the untrimmed one.
    pub plan: Arc<ExportPlan>,
}

impl Default for EditMap {
    fn default() -> Self {
        Self {
            spans: Vec::new(),
            chapters: Vec::new(),
            frames: 0,
            plan: Arc::new(ExportPlan {
                fps: FPS,
                segments: Vec::new(),
                chapters: Vec::new(),
                frames: 0,
                pieces: Vec::new(),
                bookends: Vec::new(),
                transitions: Vec::new(),
                end_card: None,
            }),
        }
    }
}

impl EditMap {
    /// Plans `project` styled by `lock` as `view` shows it, without bookends or transitions.
    pub fn of(project: &Project, lock: &Lock, view: View) -> Self {
        let (cut, lock) = view_of(&project.cut, lock, view);
        let project = Project {
            cut,
            ..project.clone()
        };
        let timeline = Timeline::evaluate(&project, &lock);
        // The preview caches nothing, so cache identity doesn't matter.
        let plan = ExportPlan::new(timeline.main(), FPS, |_| MediaId::default());
        Self::new(&project.cut, plan)
    }

    /// The map the Edit screen shows of the video in `dir`: in the edited view the timeline
    /// export renders (styled by the lock, with the brand's intro, outro and transitions),
    /// tied to the blocks of the cut; the whole recordings in the untrimmed view.
    pub fn preview(dir: &Path, project: &Project, lock: &Lock, view: View) -> Result<Self, String> {
        match view {
            View::Edited => {
                let plan =
                    shot_render::export::preview_plan(dir, project).map_err(|e| e.to_string())?;
                let map = Self::new(&project.cut, plan);
                if map.spans.len() != map.plan.segments.len() {
                    return Err(format!(
                        "the preview could place {} of the {} segments export renders",
                        map.spans.len(),
                        map.plan.segments.len()
                    ));
                }
                Ok(map)
            }
            View::Untrimmed => Ok(Self::of(project, lock, view)),
        }
    }

    /// Ties each planned segment back to its take block. Segments come in block order, so one
    /// walk over both assigns them. A chapter runs from its marker in the plan, at the intro
    /// when one opens it, to the next chapter's.
    pub fn new(cut: &Cut, plan: ExportPlan) -> Self {
        let mut spans = Vec::with_capacity(plan.segments.len());
        let mut segments = plan.segments.iter().peekable();
        for (at, edit) in blocks(cut) {
            while let Some(planned) = segments.peek() {
                let segment = &planned.segment;
                let inside = segment.take == edit.take
                    && edit.ranges.iter().any(|r| {
                        matches!(r, RangeEdit::Keep(keep)
                            if keep.from() <= segment.source.from() && segment.source.to() <= keep.to())
                    });
                if !inside {
                    break;
                }
                spans.push(Span {
                    at,
                    take: segment.take.clone(),
                    layout: segment.layout,
                    source_frames: planned.source_frames.clone(),
                    output_frame: planned.output_frame,
                    zooms: segment.zooms.clone(),
                    overlays: segment.overlays.clone(),
                    switch: segment.switch.clone(),
                });
                segments.next();
            }
        }
        // The plan marks each chapter that has frames, in order.
        let mut markers = plan.chapters.iter().map(|c| c.output_frame);
        let starts: Vec<Option<u64>> = (0..cut.chapters.len())
            .map(|index| {
                spans
                    .iter()
                    .any(|s| s.at.chapter == index)
                    .then(|| markers.next())
                    .flatten()
            })
            .collect();
        let chapters = cut
            .chapters
            .iter()
            .zip(&starts)
            .enumerate()
            .map(|(index, (chapter, start))| {
                let frames = match start {
                    Some(start) => {
                        let end = starts[index + 1..]
                            .iter()
                            .find_map(|s| *s)
                            .unwrap_or(plan.frames);
                        *start..end
                    }
                    None => 0..0,
                };
                ChapterSpan {
                    title: chapter.title.clone(),
                    frames,
                }
            })
            .collect();
        Self {
            spans,
            chapters,
            frames: plan.frames,
            plan: Arc::new(plan),
        }
    }

    /// The span that shows output frame `frame`.
    pub fn span_at(&self, frame: u64) -> Option<&Span> {
        let index = self.spans.partition_point(|s| s.output_frame <= frame);
        let span = self.spans.get(index.checked_sub(1)?)?;
        span.output_frames().contains(&frame).then_some(span)
    }

    /// The output frame showing `time` of the block at `at`, if the output keeps it.
    pub fn output_frame(&self, at: TakeAt, time: Micros) -> Option<u64> {
        let frame = time.0 * u64::from(FPS) / 1_000_000;
        self.spans
            .iter()
            .find(|s| s.at == at && s.source_frames.contains(&frame))
            .map(|s| s.output_frame + (frame - s.source_frames.start))
    }

    /// The output frame showing `time` of the block at `at`, or else the block's first kept
    /// media after it.
    pub fn output_frame_from(&self, at: TakeAt, time: Micros) -> Option<u64> {
        let frame = time.0 * u64::from(FPS) / 1_000_000;
        self.spans
            .iter()
            .filter(|s| s.at == at && s.source_frames.end > frame)
            .map(|s| s.output_frame + frame.saturating_sub(s.source_frames.start))
            .min()
    }

    /// Where the cut `range` of the block at `at` joins in the output: the block's next kept
    /// media, or the end of its last for a cut that ends the block.
    pub fn seam(&self, at: TakeAt, range: TimeRange) -> Option<u64> {
        self.output_frame_from(at, range.from()).or_else(|| {
            self.spans
                .iter()
                .filter(|s| s.at == at)
                .map(|s| s.output_frames().end)
                .max()
        })
    }

    /// Every output frame showing take time `at` of `take`, once per span that keeps it, as
    /// export places the lock's moments.
    pub fn outputs_of<'a>(&'a self, take: &'a Id, at: Micros) -> impl Iterator<Item = u64> + 'a {
        let frame = at.0 * u64::from(FPS) / 1_000_000;
        self.spans
            .iter()
            .filter(move |s| s.take == *take && s.source_frames.contains(&frame))
            .map(move |s| s.output_frame + (frame - s.source_frames.start))
    }

    /// Where a playhead on `time` of the block at `at` goes when an edit changes the media under
    /// it: the block's next kept media, or the last kept frame when the block ends in the cut.
    pub fn kept_frame(&self, at: TakeAt, time: Micros) -> Option<u64> {
        self.output_frame_from(at, time).or_else(|| {
            self.spans
                .iter()
                .filter(|s| s.at == at)
                .map(|s| s.output_frames().end.saturating_sub(1))
                .max()
        })
    }

    /// The block and media time output frame `frame` shows.
    pub fn source_at(&self, frame: u64) -> Option<(TakeAt, Micros)> {
        self.span_at(frame)
            .map(|s| (s.at, frame_time(s.source_frame(frame), FPS)))
    }

    /// The stretch of one take and layout under `frame`, as a media time range.
    pub fn stretch_at(&self, frame: u64) -> Option<(TakeAt, TimeRange)> {
        let span = self.span_at(frame)?;
        let range = TimeRange::new(
            frame_time(span.source_frames.start, FPS),
            frame_time(span.source_frames.end, FPS),
        )?;
        Some((span.at, range))
    }

    /// The output frames that show media `range` of the block at `at`, from the first kept
    /// frame to the last, as export places a zoom or an overlay.
    pub fn output_range(&self, at: TakeAt, range: TimeRange) -> Option<Range<u64>> {
        let snapped = shot_render::plan::snap(range, FPS);
        self.spans
            .iter()
            .filter(|s| s.at == at)
            .filter_map(|s| {
                let from = snapped.start.max(s.source_frames.start);
                let to = snapped.end.min(s.source_frames.end);
                (from < to).then(|| {
                    s.output_frame + (from - s.source_frames.start)
                        ..s.output_frame + (to - s.source_frames.start)
                })
            })
            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
    }

    /// The media range of the block at `at` that output `frames` show, clipped to the block.
    /// The inverse of [`Self::output_range`].
    pub fn source_range(&self, at: TakeAt, frames: Range<u64>) -> Option<TimeRange> {
        let spans = || self.spans.iter().filter(|s| s.at == at);
        let first = spans().find(|s| s.output_frames().end > frames.start)?;
        let last = spans().rfind(|s| s.output_frame < frames.end)?;
        let from = first.source_frame(frames.start.max(first.output_frame));
        let to = last.source_frame(frames.end.min(last.output_frames().end) - 1) + 1;
        TimeRange::new(frame_time(from, FPS), frame_time(to, FPS))
    }

    /// The chapter shown at `frame`.
    pub fn chapter_at(&self, frame: u64) -> Option<usize> {
        self.chapters.iter().position(|c| c.frames.contains(&frame))
    }

    /// Each stretch of one layout in one take block: spans of the block with the same layout
    /// that follow each other in the output, joined across the cuts between them.
    pub fn layout_runs(&self) -> Vec<LayoutRun> {
        // Each run's block, layout, output frames and source frames.
        let mut runs: Vec<(TakeAt, Layout, Range<u64>, Range<u64>)> = Vec::new();
        for span in &self.spans {
            let frames = span.output_frames();
            match runs.last_mut() {
                Some((at, layout, output, source))
                    if *at == span.at && *layout == span.layout && output.end == frames.start =>
                {
                    output.end = frames.end;
                    source.end = span.source_frames.end;
                }
                _ => runs.push((span.at, span.layout, frames, span.source_frames.clone())),
            }
        }
        runs.into_iter()
            .filter_map(|(at, layout, frames, source)| {
                Some(LayoutRun {
                    at,
                    layout,
                    frames,
                    range: TimeRange::new(
                        frame_time(source.start, FPS),
                        frame_time(source.end, FPS),
                    )?,
                })
            })
            .collect()
    }

    /// Output frames where the picture jumps: each span that does not continue the one before.
    pub fn cut_frames(&self) -> Vec<u64> {
        self.spans
            .windows(2)
            .filter(|pair| {
                pair[0].take != pair[1].take
                    || pair[0].source_frames.end != pair[1].source_frames.start
            })
            .map(|pair| pair[1].output_frame)
            .collect()
    }
}

/// A stretch of one layout in one take block, as the Timeline's Layout lane shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutRun {
    pub at: TakeAt,
    pub layout: Layout,
    pub frames: Range<u64>,
    /// Media from the first frame shown to the last, removed media between included.
    pub range: TimeRange,
}

/// A transcript word in a take block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocWord {
    pub text: String,
    pub range: TimeRange,
    pub state: WordState,
    /// Output frame of the word's start, or of its first kept media, in the view's output.
    pub output: Option<u64>,
}

/// A run of removed media between two kept words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutRun {
    /// From the start of the first removal to the end of the last.
    pub range: TimeRange,
    /// Removed media in the run.
    pub removed: Micros,
    /// Whether a person removed any of it.
    pub manual: bool,
    /// The removed words inside the run, as indices into the block's words.
    pub words: Range<usize>,
}

impl CutRun {
    /// Short auto cuts show as a faint seam, everything else as a red length chip.
    pub fn is_seam(&self) -> bool {
        !self.manual && self.removed < SEAM_MAX
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Index into the block's words.
    Word(usize),
    Cut(CutRun),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    pub items: Vec<Item>,
    /// Output frame of the first shown word.
    pub start: Option<u64>,
    /// Layout at the first word.
    pub layout: Layout,
}

/// One take block of the transcript document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDoc {
    pub at: TakeAt,
    pub take: Id,
    /// False when the take has no cached transcript.
    pub transcribed: bool,
    pub words: Vec<DocWord>,
    pub paragraphs: Vec<Paragraph>,
    /// What the block loses, in order, see [`removals`].
    pub removals: Vec<(TimeRange, RemovedBy)>,
}

/// The transcript of every block of `cut` styled by `lock` as `view` shows it.
pub fn document(
    cut: &Cut,
    lock: &Lock,
    transcripts: &HashMap<Id, Transcript>,
    map: &EditMap,
    view: View,
    default_layout: Layout,
) -> Vec<BlockDoc> {
    blocks(cut)
        .map(|(at, edit)| {
            let Some(transcript) = transcripts.get(&edit.take) else {
                return BlockDoc {
                    at,
                    take: edit.take.clone(),
                    transcribed: false,
                    words: Vec::new(),
                    paragraphs: Vec::new(),
                    removals: Vec::new(),
                };
            };
            let words: Vec<DocWord> = cut
                .transcript_view(at, transcript, lock)
                .unwrap_or_default()
                .into_iter()
                .map(|w| DocWord {
                    text: w.word.text.clone(),
                    range: w.word.range,
                    state: w.state,
                    // A cut next to a kept word may take its first milliseconds; the word then
                    // starts where its media does.
                    output: match w.state {
                        WordState::Kept => map.output_frame_from(at, w.word.range.from()),
                        _ => map.output_frame(at, w.word.range.from()),
                    },
                })
                .collect();
            let layout_at = |time: Micros| {
                edit.layouts
                    .iter()
                    .find(|c| c.range.from() <= time && time < c.range.to())
                    .map_or(default_layout, |c| c.layout)
            };
            let removed = removals(edit, lock);
            let items = match view {
                View::Edited => edited_items(&removed, &words),
                View::Untrimmed => (0..words.len()).map(Item::Word).collect(),
            };
            BlockDoc {
                at,
                take: edit.take.clone(),
                transcribed: true,
                paragraphs: paragraphs(items, &words, layout_at),
                words,
                removals: removed,
            }
        })
        .collect()
}

fn midpoint(range: TimeRange) -> Micros {
    Micros((range.from().0 + range.to().0) / 2)
}

/// Kept words, with each run of `removals`, the block's in order, where it sits between them.
/// Removals that meet make one run.
fn edited_items(removals: &[(TimeRange, RemovedBy)], words: &[DocWord]) -> Vec<Item> {
    let mut runs: Vec<CutRun> = Vec::new();
    for &(range, by) in removals {
        match runs.last_mut().filter(|run| run.range.to() == range.from()) {
            Some(run) => {
                run.range = TimeRange::new(run.range.from(), range.to()).expect("in order");
                run.removed = Micros(run.removed.0 + range.duration().0);
                run.manual |= by == RemovedBy::Manual;
            }
            None => runs.push(CutRun {
                range,
                removed: range.duration(),
                manual: by == RemovedBy::Manual,
                words: 0..0,
            }),
        }
    }
    for run in &mut runs {
        let inside = |w: &DocWord| midpoint(w.range) < run.range.from();
        let start = words.partition_point(inside);
        let end = words.partition_point(|w| midpoint(w.range) < run.range.to());
        run.words = start..end;
    }

    let mut items = Vec::with_capacity(words.len() + runs.len());
    let mut runs = runs.into_iter().peekable();
    for (index, word) in words.iter().enumerate() {
        while let Some(run) = runs.next_if(|run| run.words.start <= index) {
            items.push(Item::Cut(run));
        }
        if word.state == WordState::Kept {
            items.push(Item::Word(index));
        }
    }
    items.extend(runs.map(Item::Cut));
    items
}

/// Breaks items into paragraphs at long pauses, layout changes and after many words. A pause
/// counts in the view's output, so a cut does not break a paragraph. Cuts before the first word
/// open the first paragraph.
fn paragraphs(
    items: Vec<Item>,
    words: &[DocWord],
    layout_at: impl Fn(Micros) -> Layout,
) -> Vec<Paragraph> {
    let length_breaks = length_breaks(words, &layout_at);
    let gap_frames = PARAGRAPH_GAP.0 * u64::from(FPS) / 1_000_000;
    let mut out: Vec<Paragraph> = Vec::new();
    let mut leading = Vec::new();
    let mut last_word: Option<usize> = None;
    for item in items {
        let Item::Word(index) = item else {
            match out.last_mut() {
                Some(paragraph) => paragraph.items.push(item),
                None => leading.push(item),
            }
            continue;
        };
        let word = &words[index];
        let layout = layout_at(word.range.from());
        let paused = last_word.is_some_and(|last| {
            let last = &words[last];
            match (last.output, word.output) {
                (Some(from), Some(to)) => {
                    let spoken = last.range.duration().0 * u64::from(FPS) / 1_000_000;
                    to.saturating_sub(from + spoken) > gap_frames
                }
                _ => word.range.from().0.saturating_sub(last.range.to().0) > PARAGRAPH_GAP.0,
            }
        });
        let long = last_word.is_some_and(|last| length_breaks[last + 1..=index].contains(&true));
        let breaks = match out.last() {
            Some(paragraph) => paused || layout != paragraph.layout || long,
            None => true,
        };
        if breaks {
            out.push(Paragraph {
                items: std::mem::take(&mut leading),
                start: None,
                layout,
            });
        }
        let paragraph = out.last_mut().expect("pushed above");
        paragraph.start = paragraph.start.or(word.output);
        paragraph.items.push(item);
        last_word = Some(index);
    }
    if !leading.is_empty() {
        out.push(Paragraph {
            items: leading,
            start: None,
            layout: layout_at(Micros(0)),
        });
    }
    out
}

/// Marks the words that start a paragraph because the one before it ran long. Counts every
/// recorded word, cut or kept, so cutting words never moves a break, and breaks after the first
/// sentence end past `PARAGRAPH_WORDS`, or at `PARAGRAPH_WORDS_MAX` without one. A recorded pause
/// or layout change restarts the count.
fn length_breaks(words: &[DocWord], layout_at: impl Fn(Micros) -> Layout) -> Vec<bool> {
    let mut breaks = vec![false; words.len()];
    let mut count = 0;
    for (index, pair) in words.windows(2).enumerate() {
        let [last, word] = pair else { continue };
        count += 1;
        let restarts = word.range.from().0.saturating_sub(last.range.to().0) > PARAGRAPH_GAP.0
            || layout_at(word.range.from()) != layout_at(last.range.from());
        let sentence_end = last.text.trim_end().ends_with(['.', '?', '!']);
        let long = count >= PARAGRAPH_WORDS_MAX || (count >= PARAGRAPH_WORDS && sentence_end);
        breaks[index + 1] = long && !restarts;
        if restarts || long {
            count = 0;
        }
    }
    breaks
}

/// Every word the transcript shows, in output order: the block's place in `doc`, the block and
/// the word's index.
fn shown_words(doc: &[BlockDoc]) -> impl Iterator<Item = (usize, TakeAt, usize)> + '_ {
    doc.iter().enumerate().flat_map(|(n, block)| {
        block
            .paragraphs
            .iter()
            .flat_map(|p| &p.items)
            .filter_map(move |item| match item {
                Item::Word(index) => Some((n, block.at, *index)),
                Item::Cut(_) => None,
            })
    })
}

/// The shown word after, or before, word `index` of block `at`, across blocks and chapters.
/// `index` may be a word the view hides, such as a removed word of an opened cut.
pub fn shown_word_next(
    doc: &[BlockDoc],
    at: TakeAt,
    index: usize,
    forward: bool,
) -> Option<(TakeAt, usize)> {
    let here = (doc.iter().position(|b| b.at == at)?, index);
    let found = if forward {
        shown_words(doc).find(|&(n, _, i)| (n, i) > here)
    } else {
        shown_words(doc).filter(|&(n, _, i)| (n, i) < here).last()
    };
    found.map(|(_, at, index)| (at, index))
}

/// Whether `range` holds the middle of `word`, which is what makes a word count as cut.
fn holds(range: TimeRange, word: &DocWord) -> bool {
    let middle = midpoint(word.range);
    range.from() <= middle && middle < range.to()
}

/// The first word of the person's removal right before word `index` of `block`, if that removal
/// holds the word before it. A cut of `index` that starts there joins it, so cutting word after
/// word leaves one cut. A rule's cut in between stops the join, so Cut and a restyle can still
/// change it.
pub fn cut_words_before(block: &BlockDoc, index: usize) -> Option<usize> {
    let start = block.words.get(index)?.range.from();
    let (cut, by) = block.removals.iter().rfind(|(r, _)| r.from() < start)?;
    let before = block.words.get(index.checked_sub(1)?)?;
    (*by == RemovedBy::Manual && holds(*cut, before)).then(|| {
        block
            .words
            .partition_point(|w| midpoint(w.range) < cut.from())
    })
}

/// The last word of the person's removal right after word `index` of `block`, the mirror of
/// [`cut_words_before`].
pub fn cut_words_after(block: &BlockDoc, index: usize) -> Option<usize> {
    let end = block.words.get(index)?.range.to();
    let (cut, by) = block.removals.iter().find(|(r, _)| r.to() > end)?;
    let after = block.words.get(index + 1)?;
    (*by == RemovedBy::Manual && holds(*cut, after)).then(|| {
        block
            .words
            .partition_point(|w| midpoint(w.range) < cut.to())
            - 1
    })
}

/// The media to cut for `selection`: its words and the person's removals right next to them.
pub fn cut_range(block: &BlockDoc, selection: &Selection) -> Option<TimeRange> {
    let (first, last) = (*selection.words().start(), *selection.words().end());
    let joined = Selection {
        at: selection.at,
        anchor: cut_words_before(block, first).unwrap_or(first),
        head: cut_words_after(block, last).unwrap_or(last),
    };
    joined.range(&block.words)
}

/// The media a restore of `selection` brings back: its words and every removal that holds one of
/// them, whole, so no pause or edge of the cut stays behind. Other words of a person's removal
/// stay cut, each with the edge a cut of its own would have.
pub fn restore_range(block: &BlockDoc, selection: &Selection) -> Option<TimeRange> {
    let words = &block.words;
    let selected = selection.words();
    let mut from = words.get(*selected.start())?.range.from();
    let mut to = words.get(*selected.end())?.range.to();
    for (cut, _) in &block.removals {
        let mut inside = selected.clone().filter(|&i| holds(*cut, &words[i]));
        let Some(first) = inside.next() else {
            continue;
        };
        let last = inside.next_back().unwrap_or(first);
        let before = first.checked_sub(1).map(|i| &words[i]);
        from = from.min(match before.filter(|w| holds(*cut, w)) {
            Some(w) => shot_core::cut_after(w.range, Some(words[first].range)),
            None => cut.from(),
        });
        to = to.max(match words.get(last + 1).filter(|w| holds(*cut, w)) {
            Some(w) => shot_core::cut_before(Some(words[last].range), w.range),
            None => cut.to(),
        });
    }
    TimeRange::new(from, to)
}

/// Where the caret goes after a cut of `words` of block `at`: the next shown word going `forward`,
/// else the one on the other side. It stays in the chapter, so a cut never changes the page.
pub fn caret_after_cut(
    doc: &[BlockDoc],
    at: TakeAt,
    words: RangeInclusive<usize>,
    forward: bool,
) -> Option<(TakeAt, usize)> {
    let in_chapter = |word: &(TakeAt, usize)| word.0.chapter == at.chapter;
    let next = shown_word_next(doc, at, *words.end(), true).filter(in_chapter);
    let previous = shown_word_next(doc, at, *words.start(), false).filter(in_chapter);
    if forward {
        next.or(previous)
    } else {
        previous.or(next)
    }
}

/// The last shown word that starts at or before output frame `frame`, else the first one.
pub fn shown_word_at(doc: &[BlockDoc], frame: u64) -> Option<(TakeAt, usize)> {
    let starts = |&(n, _, index): &(usize, TakeAt, usize)| {
        doc[n].words[index]
            .output
            .is_some_and(|start| start <= frame)
    };
    shown_words(doc)
        .filter(starts)
        .last()
        .or_else(|| shown_words(doc).next())
        .map(|(_, at, index)| (at, index))
}

/// Whether the block at `at` holds the same take in both documents.
pub fn same_take(before: &[BlockDoc], doc: &[BlockDoc], at: TakeAt) -> bool {
    let take = |doc: &'_ [BlockDoc]| doc.iter().find(|b| b.at == at).map(|b| b.take.clone());
    take(before).is_some_and(|t| take(doc) == Some(t))
}

/// Whether the block at `at` still shows a cut run over exactly `range`.
pub fn has_run(doc: &[BlockDoc], at: TakeAt, range: TimeRange) -> bool {
    doc.iter()
        .filter(|b| b.at == at)
        .flat_map(|b| b.paragraphs.iter().flat_map(|p| &p.items))
        .any(|item| matches!(item, Item::Cut(run) if run.range == range))
}

/// Selected words of one take block, from the word the selection started at to the word it
/// extends to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub at: TakeAt,
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn word(at: TakeAt, index: usize) -> Self {
        Self {
            at,
            anchor: index,
            head: index,
        }
    }

    pub fn words(&self) -> RangeInclusive<usize> {
        self.anchor.min(self.head)..=self.anchor.max(self.head)
    }

    /// Whether the selection's head still shows in `block`: as a word, or as a removed word of
    /// a cut run that is `opened`.
    pub fn shows_in(&self, block: &BlockDoc, opened: impl Fn(&CutRun) -> bool) -> bool {
        *self.words().end() < block.words.len()
            && block
                .paragraphs
                .iter()
                .flat_map(|p| &p.items)
                .any(|item| match item {
                    Item::Word(index) => *index == self.head,
                    Item::Cut(run) => run.words.contains(&self.head) && opened(run),
                })
    }

    /// Media time from the start of the first selected word to the end of the last.
    pub fn range(&self, words: &[DocWord]) -> Option<TimeRange> {
        let words = &words.get(self.words())?;
        TimeRange::new(words.first()?.range.from(), words.last()?.range.to())
    }
}

/// The built-in layouts on the 1 to 4 keys, in the order the mockup and the renderer offer them,
/// where the format lists fewer than four.
pub const LAYOUT_KEYS: [Layout; 4] = [
    Layout::Face,
    Layout::FaceContent,
    Layout::Split,
    Layout::Content,
];

/// The layouts on the 1 to 4 keys: the built-ins of [`LAYOUT_KEYS`], each other layout the
/// format lists, a channel layout, taking the place of the first built-in it does not list.
pub fn layout_keys(format: &Format) -> Vec<Layout> {
    let listed: Vec<Layout> = std::iter::once(format.default_layout)
        .chain(format.other_layouts.iter().copied())
        .collect();
    let mut keys = LAYOUT_KEYS.to_vec();
    for extra in listed.iter().filter(|l| !LAYOUT_KEYS.contains(l)) {
        if let Some(free) = keys
            .iter_mut()
            .find(|k| LAYOUT_KEYS.contains(k) && !listed.contains(k))
        {
            *free = *extra;
        }
    }
    keys
}

/// A switch on the Layout lane: the edge where the layout changes inside a take block, in its
/// media time, and the output frame it shows at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchMark {
    pub at: TakeAt,
    pub frame: u64,
    pub switch: SceneSwitch,
}

impl EditMap {
    /// Every layout switch the output shows, once, at the output frame of its edge or, where the
    /// edge falls in removed media, of the first frame after it.
    pub fn switch_marks(&self) -> Vec<SwitchMark> {
        let mut marks: Vec<SwitchMark> = Vec::new();
        for span in &self.spans {
            let Some(switch) = &span.switch else { continue };
            if marks
                .iter()
                .any(|m| m.at == span.at && m.switch.at == switch.at)
            {
                continue;
            }
            let start = frame_time(span.source_frames.start, FPS);
            let into = switch.at.0.saturating_sub(start.0) * u64::from(FPS) / 1_000_000;
            marks.push(SwitchMark {
                at: span.at,
                frame: span.output_frame
                    + into.min(span.source_frames.end - span.source_frames.start),
                switch: switch.clone(),
            });
        }
        marks
    }
}

/// Sets `layout` over `range`. The format's default clears instead, so `cut.kdl` only spells
/// out the changes.
pub fn apply_layout(
    cut: &Cut,
    at: TakeAt,
    range: TimeRange,
    layout: Layout,
    default: Layout,
) -> Result<Cut, EditError> {
    if layout == default {
        cut.clear_layout(at, range)
    } else {
        cut.set_layout(at, range, layout)
    }
}

/// Position of `frame` along a bar for `total` frames, from 0 to 1.
pub fn fraction(frame: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (frame.min(total) as f64 / total as f64) as f32
}

/// The frame under a point at `fraction` along a bar for `total` frames.
pub fn frame_at(fraction: f32, total: u64) -> u64 {
    let frame = (f64::from(fraction.clamp(0.0, 1.0)) * total as f64) as u64;
    frame.min(total.saturating_sub(1))
}

/// `MM:SS` of an output frame, or `H:MM:SS` from an hour.
pub fn timecode(frame: u64) -> String {
    let seconds = frame / u64::from(FPS);
    match seconds / 3600 {
        0 => format!("{:02}:{:02}", seconds / 60, seconds % 60),
        hours => format!("{hours}:{:02}:{:02}", seconds / 60 % 60, seconds % 60),
    }
}

/// `MM:SS.hh` of an output frame, for the transport.
pub fn timecode_fine(frame: u64) -> String {
    let hundredths = frame % u64::from(FPS) * 100 / u64::from(FPS);
    format!("{}.{hundredths:02}", timecode(frame))
}

/// Seconds with one decimal, for lengths like "2.1 s".
pub fn seconds(time: Micros) -> String {
    format!("{:.1} s", time.0 as f64 / 1e6)
}

/// What the cut changes against the raw takes, for the Timeline workspace's change list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Removals by who made them, with their count and length, in a stable order.
    pub removed: Vec<(RemovedBy, usize, Micros)>,
    pub zooms: usize,
    pub layouts: usize,
    pub overlays: usize,
}

pub fn changes(cut: &Cut, lock: &Lock) -> Changes {
    let mut out = Changes::default();
    for (_, edit) in blocks(cut) {
        for (range, by) in removals(edit, lock) {
            match out.removed.iter_mut().find(|(who, ..)| *who == by) {
                Some((_, count, length)) => {
                    *count += 1;
                    length.0 += range.duration().0;
                }
                None => out.removed.push((by, 1, range.duration())),
            }
        }
        let (zooms, overlays) = styled(edit, lock).fold((0, 0), |(z, o), i| match i.applied {
            Applied::Zoom { .. } => (z + 1, o),
            _ => (z, o + 1),
        });
        out.zooms += edit.zooms.len() + zooms;
        out.layouts += edit.layouts.len();
        out.overlays += edit.overlays.len() + overlays;
    }
    out.removed.sort_by_key(|(by, ..)| match by {
        RemovedBy::Manual => None,
        RemovedBy::Rule(target) => Some(target.as_str()),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Chapter, LayoutChange, Word};

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    fn id(text: &str) -> Id {
        Id::new(text).unwrap()
    }

    fn block(take: &str, ranges: Vec<RangeEdit>) -> TakeEdit {
        TakeEdit {
            take: id(take),
            ranges,
            layouts: Vec::new(),
            zooms: Vec::new(),
            overlays: Vec::new(),
        }
    }

    fn keep(from: u64, to: u64) -> RangeEdit {
        RangeEdit::Keep(range(from, to))
    }

    fn remove(from: u64, to: u64) -> RangeEdit {
        RangeEdit::Remove(range(from, to))
    }

    /// A lock of `t01` that cuts `ranges` on silences.
    fn silences(ranges: &[(u64, u64)]) -> Lock {
        let cuts: String = ranges
            .iter()
            .map(|(from, to)| {
                format!("    cut from={from} to={to} on=silence t={from} layer=format rule=\"silence\"\n")
            })
            .collect();
        Lock::parse(&format!("lock 1\n\ntake t01 {{\n{cuts}}}\n")).unwrap()
    }

    fn project(cut: Cut) -> Project {
        Project {
            video: shot_core::Video::parse("title \"A\"\nstatus editing\nformat opinion\n")
                .unwrap(),
            script: shot_core::Script::parse("# A\n").unwrap(),
            format: shot_core::FormatKind::Opinion.preset(),
            cut,
        }
    }

    /// Two chapters: t01 0 to 2 s with 0.5 s removed in the middle, then t01 again from 10 s.
    fn two_chapters() -> Cut {
        Cut {
            chapters: vec![
                Chapter {
                    title: "A".into(),
                    takes: vec![block(
                        "t01",
                        vec![
                            keep(0, 1_000_000),
                            remove(1_000_000, 1_500_000),
                            keep(1_500_000, 2_000_000),
                        ],
                    )],
                },
                Chapter {
                    title: "B".into(),
                    takes: vec![block("t01", vec![keep(10_000_000, 11_000_000)])],
                },
            ],
            ..Cut::default()
        }
    }

    #[test]
    fn edit_map_ties_output_frames_to_blocks_and_media_time() {
        let map = EditMap::of(&project(two_chapters()), &Lock::default(), View::Edited);
        assert_eq!(map.frames, 150);
        assert_eq!(map.chapters[0].frames, 0..90);
        assert_eq!(map.chapters[1].frames, 90..150);
        let b = TakeAt {
            chapter: 1,
            take: 0,
        };
        // 1.5 s of t01 follows the cut at output frame 60.
        assert_eq!(
            map.output_frame(
                TakeAt {
                    chapter: 0,
                    take: 0
                },
                Micros(1_500_000)
            ),
            Some(60)
        );
        assert_eq!(
            map.output_frame(
                TakeAt {
                    chapter: 0,
                    take: 0
                },
                Micros(1_200_000)
            ),
            None
        );
        assert_eq!(map.source_at(90), Some((b, Micros(10_000_000))));
        assert_eq!(map.cut_frames(), [60, 90]);
    }

    #[test]
    fn media_ranges_and_output_frames_map_both_ways_across_a_cut() {
        let map = EditMap::of(&project(two_chapters()), &Lock::default(), View::Edited);
        let a = TakeAt {
            chapter: 0,
            take: 0,
        };
        let range = TimeRange::new(Micros(500_000), Micros(1_750_000)).unwrap();
        assert_eq!(map.output_range(a, range), Some(30..75));
        let back = map.source_range(a, 30..75).unwrap();
        assert_eq!(
            (back.from(), back.to()),
            (Micros(500_000), Micros(1_750_000))
        );
        // Frames past the block are clipped to it.
        let clipped = map.source_range(a, 80..120).unwrap();
        assert_eq!(clipped.to(), Micros(2_000_000));
    }

    #[test]
    fn an_intro_after_the_hook_moves_what_follows_and_opens_its_chapter() {
        use shot_render::join::{BookendInput, Joins};
        let project = project(two_chapters());
        let timeline = Timeline::evaluate(&project, &shot_core::Lock::default());
        let bookend = |seconds: u64| BookendInput {
            bumper: None,
            title: Some("A".into()),
            length: Micros(seconds * 1_000_000),
            identity: Vec::new(),
        };
        let plan =
            ExportPlan::new(timeline.main(), FPS, |_| MediaId::default()).with_joins(&Joins {
                intro: Some(bookend(1)),
                outro: Some(bookend(2)),
                intro_after_hook: true,
                ..Joins::default()
            });
        let map = EditMap::new(&project.cut, plan);
        // The hook's 90 frames, the intro's 60, the body's 60 and the outro's 120.
        assert_eq!(map.frames, 330);
        assert_eq!(map.chapters[0].frames, 0..90);
        assert_eq!(map.chapters[1].frames, 90..330);
        let body = TakeAt {
            chapter: 1,
            take: 0,
        };
        assert_eq!(map.output_frame(body, Micros(10_000_000)), Some(150));
        assert_eq!(map.source_at(120), None);
        assert_eq!(map.source_at(150), Some((body, Micros(10_000_000))));
    }

    #[test]
    fn layout_runs_join_a_blocks_spans_across_its_cuts() {
        let map = EditMap::of(&project(two_chapters()), &Lock::default(), View::Edited);
        let runs = map.layout_runs();
        let shape: Vec<_> = runs
            .iter()
            .map(|r| {
                (
                    r.at.chapter,
                    r.frames.clone(),
                    r.range.from().0,
                    r.range.to().0,
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (0, 0..90, 0, 2_000_000),
                (1, 90..150, 10_000_000, 11_000_000)
            ]
        );
    }

    #[test]
    fn untrimmed_plays_every_removal() {
        let map = EditMap::of(&project(two_chapters()), &Lock::default(), View::Untrimmed);
        assert_eq!(map.frames, 180);
        assert_eq!(map.cut_frames(), [120]);
    }

    fn word(text: &str, from: u64, to: u64) -> Word {
        Word {
            text: text.into(),
            range: range(from, to),
            confidence: 90,
        }
    }

    #[test]
    fn the_word_cursor_skips_cut_words_and_crosses_takes() {
        let cut = Cut {
            chapters: vec![
                Chapter {
                    title: "A".into(),
                    takes: vec![block(
                        "t01",
                        vec![
                            keep(0, 1_300_000),
                            remove(1_300_000, 2_000_000),
                            keep(2_000_000, 3_000_000),
                        ],
                    )],
                },
                Chapter {
                    title: "B".into(),
                    takes: vec![block("t02", vec![keep(0, 1_000_000)])],
                },
            ],
            ..Cut::default()
        };
        let mut one = Transcript::new(id("t01"));
        for w in [
            word("ship", 100_000, 600_000),
            word("uh", 1_400_000, 1_800_000),
            // Starts inside the cut, as when a cut's edge takes a word's first milliseconds.
            word("it", 1_950_000, 2_500_000),
        ] {
            one.push(w).unwrap();
        }
        let mut two = Transcript::new(id("t02"));
        two.push(word("now", 100_000, 500_000)).unwrap();
        let transcripts = HashMap::from([(id("t01"), one), (id("t02"), two)]);
        let p = project(cut);
        let lock = Lock::default();
        let map = EditMap::of(&p, &lock, View::Edited);
        let doc = document(
            &p.cut,
            &lock,
            &transcripts,
            &map,
            View::Edited,
            Layout::Face,
        );
        let a = TakeAt {
            chapter: 0,
            take: 0,
        };
        let b = TakeAt {
            chapter: 1,
            take: 0,
        };

        // "uh" is cut, so the cursor steps over it, and on into the next chapter's take.
        assert_eq!(shown_word_next(&doc, a, 0, true), Some((a, 2)));
        assert_eq!(shown_word_next(&doc, a, 2, true), Some((b, 0)));
        assert_eq!(shown_word_next(&doc, b, 0, true), None);
        assert_eq!(shown_word_next(&doc, b, 0, false), Some((a, 2)));
        // From a hidden word, as in an opened cut, it goes on to the next shown one.
        assert_eq!(shown_word_next(&doc, a, 1, true), Some((a, 2)));

        // "it" starts where its kept media does, right after "ship"'s 1.3 s.
        let it = doc[0].words[2].output.unwrap();
        assert_eq!(it, 78);
        assert_eq!(shown_word_at(&doc, it), Some((a, 2)));
        // A cut of "it" joins the cut of "uh" right before it.
        assert_eq!(cut_words_before(&doc[0], 2), Some(1));
        assert_eq!(cut_words_before(&doc[0], 0), None);
        // A playhead on cut media goes on to the next kept media, as after a cut under it.
        assert_eq!(map.output_frame_from(a, Micros(1_400_000)), Some(78));
        assert_eq!(map.output_frame_from(a, Micros(3_000_000)), None);
        // A cut that ends the block joins where its last kept media ends.
        assert_eq!(
            map.seam(a, range(3_000_000, 3_200_000)),
            Some(map.chapters[0].frames.end)
        );

        // The same "uh" cut by a rule is not joined.
        let mut ruled = p.cut.clone();
        ruled.chapters[0].takes[0].ranges = vec![keep(0, 3_000_000)];
        let lock = silences(&[(1_300_000, 2_000_000)]);
        let map = EditMap::of(&project(ruled.clone()), &lock, View::Edited);
        let doc = document(
            &ruled,
            &lock,
            &transcripts,
            &map,
            View::Edited,
            Layout::Face,
        );
        assert_eq!(cut_words_before(&doc[0], 2), None);
        assert_eq!(shown_word_at(&doc, it - 1), Some((a, 0)));
    }

    const AT: TakeAt = TakeAt {
        chapter: 0,
        take: 0,
    };

    /// The document of one block of `t01` with `words`, each `(text, from, to)`.
    fn one_block(
        ranges: Vec<RangeEdit>,
        lock: &Lock,
        words: &[(&str, u64, u64)],
    ) -> (EditMap, Vec<BlockDoc>) {
        let cut = Cut {
            chapters: vec![Chapter {
                title: "A".into(),
                takes: vec![block("t01", ranges)],
            }],
            ..Cut::default()
        };
        let mut transcript = Transcript::new(id("t01"));
        for &(text, from, to) in words {
            transcript.push(word(text, from, to)).unwrap();
        }
        let transcripts = HashMap::from([(id("t01"), transcript)]);
        let p = project(cut);
        let map = EditMap::of(&p, lock, View::Edited);
        let doc = document(&p.cut, lock, &transcripts, &map, View::Edited, Layout::Face);
        (map, doc)
    }

    const SPOKEN: [(&str, u64, u64); 5] = [
        ("ship", 100_000, 600_000),
        ("uh", 1_100_000, 1_400_000),
        ("it", 2_100_000, 2_400_000),
        ("right", 2_500_000, 2_900_000),
        ("now", 3_000_000, 3_400_000),
    ];

    #[test]
    fn a_cut_joins_a_persons_removal_but_never_a_rules_cut() {
        // The person cut "uh", a rule cut the silence right after it: one mixed chip.
        let ranges = vec![
            keep(0, 1_000_000),
            remove(1_000_000, 1_500_000),
            keep(1_500_000, 4_000_000),
        ];
        let (_, doc) = one_block(
            ranges.clone(),
            &silences(&[(1_500_000, 2_000_000)]),
            &SPOKEN,
        );
        assert_eq!(cut_words_before(&doc[0], 2), None);
        // The silence on the far side of the person's removal does not stop the join.
        let (_, doc) = one_block(ranges, &silences(&[(600_000, 1_000_000)]), &SPOKEN);
        assert_eq!(cut_words_before(&doc[0], 2), Some(1));
    }

    #[test]
    fn cutting_next_to_a_persons_cut_joins_it_on_either_side() {
        let ranges = vec![
            keep(0, 1_000_000),
            remove(1_000_000, 2_000_000),
            keep(2_000_000, 4_000_000),
        ];
        let (_, doc) = one_block(ranges, &Lock::default(), &SPOKEN);
        // "ship" takes the cut of "uh" after it, "it" the one before it.
        assert_eq!(cut_words_after(&doc[0], 0), Some(1));
        assert_eq!(
            cut_range(&doc[0], &Selection::word(AT, 0)),
            Some(range(100_000, 1_400_000))
        );
        assert_eq!(
            cut_range(&doc[0], &Selection::word(AT, 2)),
            Some(range(1_100_000, 2_400_000))
        );
        // Without a cut next to it, a word cuts alone.
        assert_eq!(
            cut_range(&doc[0], &Selection::word(AT, 3)),
            Some(range(2_500_000, 2_900_000))
        );
        // Delete goes on to the next word, Backspace back to the one before.
        assert_eq!(caret_after_cut(&doc, AT, 2..=2, true), Some((AT, 3)));
        assert_eq!(caret_after_cut(&doc, AT, 2..=2, false), Some((AT, 0)));
    }

    #[test]
    fn restoring_words_brings_back_what_their_cut_took() {
        // The cut of "uh" took its pause as well.
        let (_, doc) = one_block(
            vec![
                keep(0, 1_000_000),
                remove(1_000_000, 2_000_000),
                keep(2_000_000, 4_000_000),
            ],
            &Lock::default(),
            &SPOKEN,
        );
        assert_eq!(
            restore_range(&doc[0], &Selection::word(AT, 1)),
            Some(range(1_000_000, 2_000_000))
        );

        // The middle of a longer cut splits it at the gaps to the words that stay cut.
        let (_, doc) = one_block(
            vec![keep(0, 1_000_000), remove(1_000_000, 3_500_000)],
            &Lock::default(),
            &SPOKEN,
        );
        assert_eq!(
            restore_range(&doc[0], &Selection::word(AT, 2)),
            Some(range(1_480_000, 2_450_000))
        );
        // The last word of the cut takes the end of the cut with it.
        assert_eq!(
            restore_range(&doc[0], &Selection::word(AT, 4)),
            Some(range(2_950_000, 3_500_000))
        );

        // A rule's cut comes back whole, the person's removal next to it stays.
        let (_, doc) = one_block(
            vec![
                keep(0, 1_000_000),
                remove(1_000_000, 1_500_000),
                keep(1_500_000, 4_000_000),
            ],
            &silences(&[(2_000_000, 2_450_000)]),
            &SPOKEN,
        );
        assert_eq!(
            restore_range(&doc[0], &Selection::word(AT, 2)),
            Some(range(2_000_000, 2_450_000))
        );
    }

    #[test]
    fn a_selection_stands_only_on_words_that_still_show() {
        let ranges = vec![
            keep(0, 1_000_000),
            remove(1_000_000, 2_000_000),
            keep(2_000_000, 4_000_000),
        ];
        let (_, doc) = one_block(ranges, &Lock::default(), &SPOKEN);
        assert!(Selection::word(AT, 0).shows_in(&doc[0], |_| false));
        // "uh" is cut: it shows only while its chip is open.
        assert!(!Selection::word(AT, 1).shows_in(&doc[0], |_| false));
        assert!(Selection::word(AT, 1).shows_in(&doc[0], |_| true));
        assert!(!Selection::word(AT, 9).shows_in(&doc[0], |_| true));

        assert!(has_run(&doc, AT, range(1_000_000, 2_000_000)));
        assert!(!has_run(&doc, AT, range(1_000_000, 1_900_000)));

        // Another take at the same place is not the block the person picked from.
        let mut swapped = doc.clone();
        swapped[0].take = id("t02");
        assert!(same_take(&doc, &doc, AT));
        assert!(!same_take(&doc, &swapped, AT));
    }

    #[test]
    fn the_caret_stays_in_the_chapter_after_a_cut() {
        let cut = Cut {
            chapters: vec![
                Chapter {
                    title: "A".into(),
                    takes: vec![block("t01", vec![keep(0, 2_000_000)])],
                },
                Chapter {
                    title: "B".into(),
                    takes: vec![block("t02", vec![keep(0, 1_000_000)])],
                },
            ],
            ..Cut::default()
        };
        let mut one = Transcript::new(id("t01"));
        one.push(word("ship", 100_000, 600_000)).unwrap();
        one.push(word("it", 1_000_000, 1_500_000)).unwrap();
        let mut two = Transcript::new(id("t02"));
        two.push(word("now", 100_000, 500_000)).unwrap();
        let transcripts = HashMap::from([(id("t01"), one), (id("t02"), two)]);
        let p = project(cut);
        let lock = Lock::default();
        let map = EditMap::of(&p, &lock, View::Edited);
        let doc = document(
            &p.cut,
            &lock,
            &transcripts,
            &map,
            View::Edited,
            Layout::Face,
        );
        let b = TakeAt {
            chapter: 1,
            take: 0,
        };
        // The chapter's last word falls back to the word before, in either direction.
        assert_eq!(caret_after_cut(&doc, AT, 1..=1, true), Some((AT, 0)));
        assert_eq!(caret_after_cut(&doc, AT, 0..=0, false), Some((AT, 1)));
        // A chapter's only word leaves no caret, rather than one in another chapter.
        assert_eq!(caret_after_cut(&doc, b, 0..=0, true), None);
        assert_eq!(caret_after_cut(&doc, b, 0..=0, false), None);
    }

    #[test]
    fn a_playhead_on_media_cut_at_the_end_of_a_block_stays_in_the_block() {
        let map = EditMap::of(&project(two_chapters()), &Lock::default(), View::Edited);
        // Inside the block it goes on to the next kept media.
        assert_eq!(map.kept_frame(AT, Micros(1_200_000)), Some(60));
        // Past the block's last kept media it stays on that media's last frame, in chapter A.
        let last = map.kept_frame(AT, Micros(2_500_000)).unwrap();
        assert_eq!(last, map.chapters[0].frames.end - 1);
        assert_eq!(map.chapter_at(last), Some(0));
    }

    #[test]
    fn edited_document_hides_removed_words_behind_their_cut() {
        let cut = Cut {
            chapters: vec![Chapter {
                title: "A".into(),
                takes: vec![block(
                    "t01",
                    vec![
                        keep(0, 1_300_000),
                        remove(1_300_000, 2_000_000),
                        keep(2_000_000, 3_400_000),
                    ],
                )],
            }],
            ..Cut::default()
        };
        let mut transcript = Transcript::new(id("t01"));
        for w in [
            word("ship", 100_000, 600_000),
            word("uh", 1_400_000, 1_800_000),
            word("it", 2_100_000, 2_500_000),
        ] {
            transcript.push(w).unwrap();
        }
        let transcripts = HashMap::from([(id("t01"), transcript)]);
        let p = project(cut);
        let lock = silences(&[(1_000_000, 1_300_000), (3_000_000, 3_400_000)]);
        let map = EditMap::of(&p, &lock, View::Edited);
        let doc = document(
            &p.cut,
            &lock,
            &transcripts,
            &map,
            View::Edited,
            Layout::Face,
        );
        let items = &doc[0].paragraphs[0].items;
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], Item::Word(0));
        let Item::Cut(run) = &items[1] else {
            panic!("expected a cut, got {:?}", items[1]);
        };
        assert_eq!(run.range, range(1_000_000, 2_000_000));
        assert_eq!(run.removed, Micros(1_000_000));
        assert!(run.manual && !run.is_seam());
        assert_eq!(run.words, 1..2);
        assert_eq!(items[2], Item::Word(2));
        let Item::Cut(tail) = &items[3] else {
            panic!("expected the trailing cut");
        };
        assert!(tail.is_seam());
        // "it" at 2.1 s plays after 1 s of kept media.
        assert_eq!(doc[0].words[2].output, Some(66));

        let untrimmed = EditMap::of(&p, &lock, View::Untrimmed);
        let all = document(
            &p.cut,
            &lock,
            &transcripts,
            &untrimmed,
            View::Untrimmed,
            Layout::Face,
        );
        assert_eq!(
            all[0].paragraphs[0].items,
            [Item::Word(0), Item::Word(1), Item::Word(2)]
        );
    }

    #[test]
    fn paragraphs_break_at_pauses_and_layout_changes() {
        let mut edit = block("t01", vec![keep(0, 20_000_000)]);
        edit.layouts.push(LayoutChange::new(
            Layout::Split,
            range(10_000_000, 20_000_000),
        ));
        let cut = Cut {
            chapters: vec![Chapter {
                title: "A".into(),
                takes: vec![edit],
            }],
            ..Cut::default()
        };
        let mut transcript = Transcript::new(id("t01"));
        for w in [
            word("a", 0, 500_000),
            word("b", 600_000, 1_000_000),
            word("c", 3_000_000, 3_500_000),
            word("d", 10_000_000, 10_500_000),
        ] {
            transcript.push(w).unwrap();
        }
        let transcripts = HashMap::from([(id("t01"), transcript)]);
        let p = project(cut);
        let lock = Lock::default();
        let map = EditMap::of(&p, &lock, View::Edited);
        let doc = document(
            &p.cut,
            &lock,
            &transcripts,
            &map,
            View::Edited,
            Layout::Face,
        );
        let paragraphs = &doc[0].paragraphs;
        let shape: Vec<_> = paragraphs
            .iter()
            .map(|p| (p.items.len(), p.layout, p.start))
            .collect();
        assert_eq!(
            shape,
            [
                (2, Layout::Face, Some(0)),
                (1, Layout::Face, Some(180)),
                (1, Layout::Split, Some(600))
            ]
        );
    }

    #[test]
    fn paragraphs_break_at_a_sentence_end_and_ignore_cuts() {
        let words: Vec<DocWord> = (0..200u64)
            .map(|i| DocWord {
                text: if i == 64 { "end.".into() } else { "w".into() },
                range: range(i * 300_000, i * 300_000 + 250_000),
                state: WordState::Kept,
                output: None,
            })
            .collect();
        let starts = |kept: &dyn Fn(usize) -> bool| -> Vec<usize> {
            let items = (0..words.len())
                .filter(|&i| kept(i))
                .map(Item::Word)
                .collect();
            paragraphs(items, &words, |_| Layout::Face)
                .iter()
                .map(|p| match p.items[0] {
                    Item::Word(index) => index,
                    Item::Cut(_) => unreachable!(),
                })
                .collect()
        };
        // Breaks after the sentence end, then after the hard limit without one.
        assert_eq!(starts(&|_| true), [0, 65, 185]);
        // Cutting words inside the first paragraph moves no break.
        assert_eq!(starts(&|i| !(10..14).contains(&i)), [0, 65, 185]);
    }

    #[test]
    fn selection_spans_its_words_in_either_direction() {
        let words: Vec<DocWord> = [(0, 400_000), (500_000, 900_000), (1_000_000, 1_400_000)]
            .into_iter()
            .map(|(from, to)| DocWord {
                text: "w".into(),
                range: range(from, to),
                state: WordState::Kept,
                output: None,
            })
            .collect();
        let at = TakeAt {
            chapter: 0,
            take: 0,
        };
        let backwards = Selection {
            at,
            anchor: 2,
            head: 1,
        };
        assert_eq!(backwards.range(&words), Some(range(500_000, 1_400_000)));
        assert_eq!(
            Selection::word(at, 0).range(&words),
            Some(range(0, 400_000))
        );
        assert_eq!(Selection::word(at, 7).range(&words), None);
    }

    #[test]
    fn layout_keys_map_to_the_four_layouts_and_the_default_clears() {
        assert_eq!(LAYOUT_KEYS[0], Layout::Face);
        assert_eq!(LAYOUT_KEYS[3], Layout::Content);
        let mut format = shot_core::FormatKind::Opinion.preset();
        assert_eq!(layout_keys(&format), LAYOUT_KEYS);
        let custom = |n: &str| Layout::Custom(shot_core::LayoutName::new(n).unwrap());
        format.other_layouts = vec![custom("side-by-side"), Layout::Split, custom("talk-left")];
        assert_eq!(
            layout_keys(&format),
            [
                Layout::Face,
                custom("side-by-side"),
                Layout::Split,
                custom("talk-left")
            ]
        );

        let cut = two_chapters();
        let at = TakeAt {
            chapter: 0,
            take: 0,
        };
        let split = apply_layout(&cut, at, range(0, 500_000), Layout::Split, Layout::Face).unwrap();
        assert_eq!(split.chapters[0].takes[0].layouts.len(), 1);
        let back = apply_layout(&split, at, range(0, 500_000), Layout::Face, Layout::Face).unwrap();
        assert!(back.chapters[0].takes[0].layouts.is_empty());
    }

    #[test]
    fn overview_maps_between_frames_and_bar_positions() {
        assert_eq!(fraction(30, 120), 0.25);
        assert_eq!(fraction(500, 120), 1.0);
        assert_eq!(fraction(5, 0), 0.0);
        assert_eq!(frame_at(0.25, 120), 30);
        assert_eq!(frame_at(1.0, 120), 119);
        assert_eq!(frame_at(-1.0, 120), 0);
        assert_eq!(timecode(3 * 60 * 60 + 41 * 60 + 12), "03:41");
        assert_eq!(timecode_fine(221 * 60 + 12), "03:41.20");
    }
}
