//! The Edit screen's view of a project, as pure functions: which output frame shows which take
//! block, the transcript as paragraphs of words and cuts, selections, layout keys and the
//! whole-video overview. Nothing here holds edit state; it is all derived from the files.

use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};

use shot_core::{
    Cut, EditError, Id, Layout, MediaId, Micros, Overlay, Project, RangeEdit, RemovedBy, TakeAt,
    TimeRange, Timeline, Transcript, WordState, Zoom,
};
use shot_render::plan::{self, ExportPlan, frame_time};

/// Output frame rate of preview and export.
pub const FPS: u32 = 60;

/// Auto cuts shorter than this show as a seam without a label.
const SEAM_MAX: Micros = Micros(1_000_000);
/// A pause this long between two words starts a new paragraph.
const PARAGRAPH_GAP: Micros = Micros(1_500_000);
/// Paragraphs break after this many words even without a pause.
const PARAGRAPH_WORDS: usize = 60;

/// Whether timecodes and the preview follow the edit or the whole recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Edited,
    Untrimmed,
}

/// The cut a view plays: the edit itself, or the edit with every removal restored.
pub fn cut_for(cut: &Cut, view: View) -> Cut {
    match view {
        View::Edited => cut.clone(),
        View::Untrimmed => blocks(cut).fold(cut.clone(), |restored, (at, edit)| {
            let whole = edit
                .ranges
                .first()
                .zip(edit.ranges.last())
                .and_then(|(first, last)| TimeRange::new(first.range().from(), last.range().to()));
            match whole {
                Some(range) => restored.restore(at, range).unwrap_or(restored),
                None => restored,
            }
        }),
    }
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

/// Where every take block lands in one output.
#[derive(Debug, Clone, PartialEq)]
pub struct EditMap {
    pub spans: Vec<Span>,
    /// One per chapter of the cut, in order.
    pub chapters: Vec<ChapterSpan>,
    pub frames: u64,
}

impl EditMap {
    /// Plans `project` as `view` shows it.
    pub fn of(project: &Project, view: View) -> Self {
        let project = Project {
            cut: cut_for(&project.cut, view),
            ..project.clone()
        };
        let timeline = Timeline::evaluate(&project);
        // Only the time mapping is used here, so cache identity doesn't matter.
        let plan = ExportPlan::new(timeline.main(), FPS, |_| MediaId::default());
        Self::new(&project.cut, &plan)
    }

    /// Ties each planned segment back to its take block. Segments come in block order, so one
    /// walk over both assigns them.
    pub fn new(cut: &Cut, plan: &ExportPlan) -> Self {
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
                });
                segments.next();
            }
        }
        let chapters = cut
            .chapters
            .iter()
            .enumerate()
            .map(|(index, chapter)| {
                let mut of_chapter = spans.iter().filter(|s| s.at.chapter == index);
                let frames = match of_chapter.next() {
                    Some(first) => {
                        let end = of_chapter.next_back().unwrap_or(first).output_frames().end;
                        first.output_frame..end
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

    /// Every output frame showing take time `at` of `take`, once per span that keeps it, as
    /// export places the lock's moments.
    pub fn outputs_of<'a>(&'a self, take: &'a Id, at: Micros) -> impl Iterator<Item = u64> + 'a {
        let frame = at.0 * u64::from(FPS) / 1_000_000;
        self.spans
            .iter()
            .filter(move |s| s.take == *take && s.source_frames.contains(&frame))
            .map(move |s| s.output_frame + (frame - s.source_frames.start))
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

    /// The chapter shown at `frame`.
    pub fn chapter_at(&self, frame: u64) -> Option<usize> {
        self.chapters.iter().position(|c| c.frames.contains(&frame))
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

/// A transcript word in a take block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocWord {
    pub text: String,
    pub range: TimeRange,
    pub state: WordState,
    /// Output frame of the word's start, in the view's output.
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
}

/// The transcript of every block of `cut` as `view` shows it.
pub fn document(
    cut: &Cut,
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
                };
            };
            let words: Vec<DocWord> = cut
                .transcript_view(at, transcript)
                .unwrap_or_default()
                .into_iter()
                .map(|w| DocWord {
                    text: w.word.text.clone(),
                    range: w.word.range,
                    state: w.state,
                    output: map.output_frame(at, w.word.range.from()),
                })
                .collect();
            let layout_at = |time: Micros| {
                edit.layouts
                    .iter()
                    .find(|c| c.range.from() <= time && time < c.range.to())
                    .map_or(default_layout, |c| c.layout)
            };
            let items = match view {
                View::Edited => edited_items(&edit.ranges, &words),
                View::Untrimmed => (0..words.len()).map(Item::Word).collect(),
            };
            BlockDoc {
                at,
                take: edit.take.clone(),
                transcribed: true,
                paragraphs: paragraphs(items, &words, layout_at),
                words,
            }
        })
        .collect()
}

fn midpoint(range: TimeRange) -> Micros {
    Micros((range.from().0 + range.to().0) / 2)
}

/// Kept words, with each run of removals where it sits between them.
fn edited_items(ranges: &[RangeEdit], words: &[DocWord]) -> Vec<Item> {
    let mut runs: Vec<CutRun> = Vec::new();
    let mut previous_removed = false;
    for edit in ranges {
        match *edit {
            RangeEdit::Remove { range, by } => {
                match runs.last_mut().filter(|_| previous_removed) {
                    Some(run) => {
                        run.range = TimeRange::new(run.range.from(), range.to())
                            .expect("ranges are ordered");
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
                previous_removed = true;
            }
            RangeEdit::Keep(_) => previous_removed = false,
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
    let gap_frames = PARAGRAPH_GAP.0 * u64::from(FPS) / 1_000_000;
    let mut out: Vec<Paragraph> = Vec::new();
    let mut leading = Vec::new();
    let mut last_word: Option<usize> = None;
    let mut count = 0;
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
        let breaks = match out.last() {
            Some(paragraph) => paused || layout != paragraph.layout || count >= PARAGRAPH_WORDS,
            None => true,
        };
        if breaks {
            out.push(Paragraph {
                items: std::mem::take(&mut leading),
                start: None,
                layout,
            });
            count = 0;
        }
        let paragraph = out.last_mut().expect("pushed above");
        paragraph.start = paragraph.start.or(word.output);
        paragraph.items.push(item);
        count += 1;
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

    /// Media time from the start of the first selected word to the end of the last.
    pub fn range(&self, words: &[DocWord]) -> Option<TimeRange> {
        let words = &words.get(self.words())?;
        TimeRange::new(words.first()?.range.from(), words.last()?.range.to())
    }
}

/// The layouts on the 1 to 4 keys, in the order the mockup and the renderer offer them.
pub const LAYOUT_KEYS: [Layout; 4] = [
    Layout::Face,
    Layout::FaceContent,
    Layout::Split,
    Layout::Content,
];

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

pub fn changes(cut: &Cut) -> Changes {
    let mut out = Changes::default();
    for (_, edit) in blocks(cut) {
        for range in &edit.ranges {
            if let RangeEdit::Remove { range, by } = *range {
                match out.removed.iter_mut().find(|(who, ..)| *who == by) {
                    Some((_, count, length)) => {
                        *count += 1;
                        length.0 += range.duration().0;
                    }
                    None => out.removed.push((by, 1, range.duration())),
                }
            }
        }
        out.zooms += edit.zooms.len();
        out.layouts += edit.layouts.len();
        out.overlays += edit.overlays.len();
    }
    out.removed.sort_by_key(|(by, ..)| match by {
        RemovedBy::Manual => None,
        RemovedBy::Rule(rule) => Some(*rule),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Chapter, LayoutChange, RuleKind, TakeEdit, Word};

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
            reviews: Vec::new(),
            zooms: Vec::new(),
            overlays: Vec::new(),
        }
    }

    fn keep(from: u64, to: u64) -> RangeEdit {
        RangeEdit::Keep(range(from, to))
    }

    fn remove(from: u64, to: u64, by: RemovedBy) -> RangeEdit {
        RangeEdit::Remove {
            range: range(from, to),
            by,
        }
    }

    const SILENCE: RemovedBy = RemovedBy::Rule(RuleKind::Silence);

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
                            remove(1_000_000, 1_500_000, SILENCE),
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
        let map = EditMap::of(&project(two_chapters()), View::Edited);
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
    fn untrimmed_plays_every_removal() {
        let map = EditMap::of(&project(two_chapters()), View::Untrimmed);
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
    fn edited_document_hides_removed_words_behind_their_cut() {
        let cut = Cut {
            chapters: vec![Chapter {
                title: "A".into(),
                takes: vec![block(
                    "t01",
                    vec![
                        keep(0, 1_000_000),
                        remove(1_000_000, 1_300_000, SILENCE),
                        remove(1_300_000, 2_000_000, RemovedBy::Manual),
                        keep(2_000_000, 3_000_000),
                        remove(3_000_000, 3_400_000, SILENCE),
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
        let map = EditMap::of(&p, View::Edited);
        let doc = document(&p.cut, &transcripts, &map, View::Edited, Layout::Face);
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

        let untrimmed = EditMap::of(&p, View::Untrimmed);
        let all = document(
            &p.cut,
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
        edit.layouts.push(LayoutChange {
            layout: Layout::Split,
            range: range(10_000_000, 20_000_000),
        });
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
        let map = EditMap::of(&p, View::Edited);
        let doc = document(&p.cut, &transcripts, &map, View::Edited, Layout::Face);
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
