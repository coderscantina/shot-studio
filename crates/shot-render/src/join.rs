//! How a migrated video's pieces join: the intro and outro placed around and between its
//! segments, and transitions at chapter edges and bookend joins ([0021], [0023]). Pure, no IO:
//! the export measures bumpers and hands in [`Joins`].
//!
//! - **Pieces** play in output order: each take segment, and a bookend before the video or
//!   after the hook (the first chapter), and one after the last segment. A bookend belongs to the
//!   chapter after it, the outro to the last chapter, so chapter markers start at the intro.
//! - **Transitions**: a `chapter-start` transition joins its chapter to the piece before it,
//!   which may be the intro. The `video-start` transition joins the bookends: into the intro, out
//!   of it where the next chapter has none of its own, and into the outro. A dissolve, dip, mask
//!   wipe or zoom-through is centered on its edge, half its length on each side, shortened to the
//!   pieces it joins. A stinger plays from its peak before the edge, so it covers the cut at its
//!   peak, for as long as its Lottie lasts. A cut draws nothing.
//! - **Handles**: a dissolve and a mask wipe show both sides at once. The outgoing piece runs on
//!   past its edge and the incoming one starts early, into their takes' material outside the
//!   kept range, holding the media's first or last frame where it has none.
//! - **Chunks** stay inside one piece. Every chunk a window overlaps adds the transition to its
//!   key: the kind and window relative to the chunk, and for two-sided kinds the other side's key
//!   over the frames it shows, so changing either side re-renders exactly the chunks of the
//!   window. A bookend's chunks are keyed by what it draws and their frames in it, never by the
//!   video around it, so a bumper renders the same in every video with the same brand.
//! - **The end card** draws over the last frames of the output, which the outro is at least as
//!   long as. Chunks under it add its length and their place in it.
//!
//! [0021]: ../../../docs/decisions/0021-sound-transitions-and-bookends.md
//! [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md

use std::ops::Range;

use shot_core::{
    Applied, ContentHash, Id, Lock, MediaId, Micros, Output, RENDERER_VERSION, Target, TimeRange,
    TransitionKind,
};

use crate::plan::{ExportPlan, PlannedChunk, chunk_grid, frame_time};
use crate::theme::Theme;

/// What the lock and the brand put around and between a main output's segments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Joins {
    pub intro: Option<BookendInput>,
    pub outro: Option<BookendInput>,
    /// Whether the intro plays after the hook, the first chapter, instead of before the video.
    pub intro_after_hook: bool,
    pub transitions: Vec<Join>,
    /// The brand stinger's timing, when it has one.
    pub stinger: Option<StingerTiming>,
    /// The end card's length, over the last frames.
    pub end_card: Option<Micros>,
}

/// An intro or outro to place, measured by the export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookendInput {
    /// The bumper's path in the channel repo.
    pub bumper: Option<String>,
    /// The generated title's text, `None` for a bookend without one.
    pub title: Option<String>,
    /// How long it plays; an outro plays at least as long as the end card.
    pub length: Micros,
    /// What its frames draw besides their place in it, for chunk keys: the bumper file's
    /// identity and the brand fields its title draws.
    pub identity: Vec<u8>,
}

/// One `transition` instance of the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    pub take: Id,
    pub at: Micros,
    /// `chapter-start` or `video-start`.
    pub target: Target,
    pub kind: TransitionKind,
    pub length: Micros,
}

/// How long the brand stinger plays and when it covers the whole picture, where the cut is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StingerTiming {
    pub duration: Micros,
    pub peak: Micros,
}

impl Joins {
    /// The lock's transitions, in take order. Bookends come from the export, which measures
    /// their files.
    pub fn transitions_of(lock: &Lock) -> Vec<Join> {
        lock.takes
            .iter()
            .flat_map(|take| {
                take.instances
                    .iter()
                    .filter_map(|instance| match instance.applied {
                        Applied::Transition { kind, at, length } => Some(Join {
                            take: take.take.clone(),
                            at,
                            target: instance.element.target,
                            kind,
                            length,
                        }),
                        _ => None,
                    })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BookendKind {
    Intro,
    Outro,
}

/// An intro or outro in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedBookend {
    pub kind: BookendKind,
    pub bumper: Option<String>,
    pub title: Option<String>,
    pub output_frame: u64,
    pub frames: u64,
    /// Its own frames from 0, on the chunk grid.
    pub chunks: Vec<PlannedChunk>,
    /// The key its chunks extend with their frames.
    pub key: ContentHash,
}

/// One piece of the output: a take segment or a bookend, by its index in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Piece {
    Segment(usize),
    Bookend(usize),
}

/// A transition placed in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTransition {
    pub kind: TransitionKind,
    /// Where `before` ends and `after` starts.
    pub edge: u64,
    /// The output frames it draws over.
    pub window: Range<u64>,
    pub before: Piece,
    pub after: Piece,
}

impl PlannedTransition {
    /// Whether a frame of it shows both sides at once.
    pub fn two_sided(&self) -> bool {
        matches!(
            self.kind,
            TransitionKind::Dissolve | TransitionKind::MaskWipe
        )
    }
}

/// A frame inside a transition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JoinFrame<'a> {
    pub transition: &'a PlannedTransition,
    /// How far through the window the frame's middle is, 0 to 1.
    pub progress: f32,
    /// How far through its half of the window, 0 at the window's edge, 1 at the cut.
    pub toward_edge: f32,
    /// Whether the frame is the outgoing piece's.
    pub before_edge: bool,
}

impl ExportPlan {
    /// Places `joins` into a plan of the main output: shifts the segments around the bookends,
    /// moves chapter markers to the intro that opens them, and resolves the transitions to
    /// output windows. Chunk keys of the segments stay as they are; see [`key_joins`].
    pub fn with_joins(mut self, joins: &Joins) -> Self {
        let fps = self.fps;
        let frames_of = |t: Micros| (t.0 * u64::from(fps) + 500_000) / 1_000_000;
        let chapter_starts: Vec<usize> = self
            .chapters
            .iter()
            .filter_map(|c| {
                self.segments
                    .iter()
                    .position(|s| s.output_frame == c.output_frame)
            })
            .collect();
        let mut order: Vec<Piece> = (0..self.segments.len()).map(Piece::Segment).collect();
        let bookend = |kind, input: &BookendInput, minimum: u64| {
            let frames = frames_of(input.length).max(minimum);
            let key = ContentHash::of(&framed(&[
                &RENDERER_VERSION.to_le_bytes(),
                b"bookend",
                &[kind as u8],
                &u64::from(fps).to_le_bytes(),
                &input.identity,
            ]));
            PlannedBookend {
                kind,
                bumper: input.bumper.clone(),
                title: input.title.clone(),
                output_frame: 0,
                frames,
                chunks: chunk_grid(0..frames, fps)
                    .map(|frames| PlannedChunk {
                        hash: key.extend(&framed(&[
                            &frames.start.to_le_bytes(),
                            &frames.end.to_le_bytes(),
                        ])),
                        frames,
                    })
                    .collect(),
                key,
            }
        };
        let end_card = joins.end_card.map_or(0, frames_of);
        if let Some(intro) = &joins.intro {
            let at = match chapter_starts.get(1) {
                Some(&second) if joins.intro_after_hook => second,
                _ => 0,
            };
            self.bookends.push(bookend(BookendKind::Intro, intro, 1));
            order.insert(at, Piece::Bookend(self.bookends.len() - 1));
        }
        if let Some(outro) = &joins.outro {
            self.bookends
                .push(bookend(BookendKind::Outro, outro, end_card.max(1)));
            order.push(Piece::Bookend(self.bookends.len() - 1));
        }

        let mut at = 0;
        for piece in &order {
            match *piece {
                Piece::Segment(i) => {
                    let segment = &mut self.segments[i];
                    segment.output_frame = at;
                    at += segment.frames();
                }
                Piece::Bookend(i) => {
                    let bookend = &mut self.bookends[i];
                    bookend.output_frame = at;
                    at += bookend.frames;
                }
            }
        }
        self.frames = at;
        for (chapter, &first) in self.chapters.iter_mut().zip(&chapter_starts) {
            let index = order
                .iter()
                .position(|p| *p == Piece::Segment(first))
                .expect("every segment is in the order");
            let opens = match index.checked_sub(1).map(|i| order[i]) {
                Some(Piece::Bookend(b)) if self.bookends[b].kind == BookendKind::Intro => {
                    self.bookends[b].output_frame
                }
                _ => self.segments[first].output_frame,
            };
            chapter.output_frame = opens;
        }
        self.end_card = (end_card > 0).then(|| self.frames.saturating_sub(end_card)..self.frames);
        self.pieces = order;
        self.place_transitions(joins, &chapter_starts);
        self
    }

    fn place_transitions(&mut self, joins: &Joins, chapter_starts: &[usize]) {
        let fps = self.fps;
        let frames_of = |t: Micros| (t.0 * u64::from(fps) + 500_000) / 1_000_000;
        // The join into each piece, by its index in the order.
        let mut into: Vec<Option<&Join>> = vec![None; self.pieces.len()];
        for join in joins
            .transitions
            .iter()
            .filter(|j| j.target == Target::ChapterStart)
        {
            let opened = chapter_starts.iter().copied().find(|&s| {
                let segment = &self.segments[s].segment;
                segment.take == join.take
                    && segment.source.from() <= join.at
                    && join.at < segment.source.to()
            });
            if let Some(first) = opened {
                let index = self.index_of(Piece::Segment(first));
                into[index] = Some(join);
            }
        }
        if let Some(video) = joins
            .transitions
            .iter()
            .find(|j| j.target == Target::VideoStart)
        {
            for (index, piece) in self.pieces.iter().enumerate() {
                let Piece::Bookend(b) = *piece else {
                    continue;
                };
                into[index] = Some(video);
                if self.bookends[b].kind == BookendKind::Intro
                    && let Some(next) = into.get_mut(index + 1)
                {
                    next.get_or_insert(video);
                }
            }
        }
        let mut transitions: Vec<PlannedTransition> = Vec::new();
        for (index, join) in into.iter().enumerate() {
            let (Some(join), Some(previous)) = (join, index.checked_sub(1)) else {
                continue;
            };
            let (before, after) = (self.pieces[previous], self.pieces[index]);
            let edge = self.range_of(after).start;
            let floor = transitions.last().map_or(0, |t| t.window.end);
            let window = match join.kind {
                TransitionKind::Cut => continue,
                TransitionKind::Stinger => {
                    let Some(stinger) = joins.stinger else {
                        continue;
                    };
                    let start = edge.saturating_sub(frames_of(stinger.peak)).max(floor);
                    start..(start + frames_of(stinger.duration)).min(self.frames)
                }
                _ => {
                    let half = (frames_of(join.length) / 2)
                        .min(self.range_of(before).end - self.range_of(before).start)
                        .min(self.range_of(after).end - edge)
                        .min(edge - floor);
                    edge - half..edge + half
                }
            };
            if window.start < edge && edge < window.end {
                transitions.push(PlannedTransition {
                    kind: join.kind,
                    edge,
                    window,
                    before,
                    after,
                });
            }
        }
        self.transitions = transitions;
    }

    fn index_of(&self, piece: Piece) -> usize {
        self.pieces
            .iter()
            .position(|p| *p == piece)
            .expect("the piece is in the plan")
    }

    /// The output frames `piece` shows.
    pub fn range_of(&self, piece: Piece) -> Range<u64> {
        match piece {
            Piece::Segment(i) => {
                let s = &self.segments[i];
                s.output_frame..s.output_frame + s.frames()
            }
            Piece::Bookend(i) => {
                let b = &self.bookends[i];
                b.output_frame..b.output_frame + b.frames
            }
        }
    }

    /// The piece that shows output frame `frame`.
    pub fn piece_at(&self, frame: u64) -> Option<Piece> {
        let index = self
            .pieces
            .partition_point(|p| self.range_of(*p).start <= frame)
            .checked_sub(1)?;
        let piece = self.pieces[index];
        self.range_of(piece).contains(&frame).then_some(piece)
    }

    /// Where `piece` is at output frame `frame`: a segment's frame of its take, a bookend's own
    /// frame. Past its edges a segment runs on into its take, never before the take's first
    /// frame, and a bookend holds its first or last frame.
    pub fn position(&self, piece: Piece, frame: u64) -> u64 {
        let range = self.range_of(piece);
        match piece {
            Piece::Segment(i) => {
                let start = self.segments[i].source_frames.start;
                if frame < range.start {
                    start.saturating_sub(range.start - frame)
                } else {
                    start + (frame - range.start)
                }
            }
            Piece::Bookend(_) => frame.clamp(range.start, range.end - 1) - range.start,
        }
    }

    /// The chunks of `piece`, with the output frame each starts at.
    pub fn chunks_of(&self, piece: Piece) -> impl Iterator<Item = (u64, &PlannedChunk)> {
        let (chunks, origin) = match piece {
            Piece::Segment(i) => {
                let s = &self.segments[i];
                (
                    &s.chunks,
                    s.output_frame as i64 - s.source_frames.start as i64,
                )
            }
            Piece::Bookend(i) => {
                let b = &self.bookends[i];
                (&b.chunks, b.output_frame as i64)
            }
        };
        chunks
            .iter()
            .map(move |c| ((origin + c.frames.start as i64) as u64, c))
    }

    /// Every chunk of the output in order, with the output frame each starts at.
    pub fn output_chunks(&self) -> impl Iterator<Item = (u64, &PlannedChunk)> {
        self.pieces.iter().flat_map(|p| self.chunks_of(*p))
    }

    /// The transition output frame `frame` lies in, if any.
    pub fn join_at(&self, frame: u64) -> Option<JoinFrame<'_>> {
        let index = self
            .transitions
            .partition_point(|t| t.window.start <= frame)
            .checked_sub(1)?;
        let transition = &self.transitions[index];
        let window = &transition.window;
        if !window.contains(&frame) {
            return None;
        }
        let middle = frame as f32 + 0.5;
        let before_edge = frame < transition.edge;
        let toward_edge = if before_edge {
            (middle - window.start as f32) / (transition.edge - window.start) as f32
        } else {
            (window.end as f32 - middle) / (window.end - transition.edge) as f32
        };
        Some(JoinFrame {
            transition,
            progress: (middle - window.start as f32) / (window.end - window.start) as f32,
            toward_edge,
            before_edge,
        })
    }
}

/// What the keys of a main output's chunks are made of, for [`key_joins`].
pub struct KeyInputs<'a> {
    pub output: &'a Output,
    pub media: &'a dyn Fn(&Id) -> MediaId,
    /// The identity of a file an overlay names, see [`crate::overlay::key_chunks`].
    pub identity: &'a dyn Fn(&str) -> Vec<u8>,
    pub theme: &'a Theme,
}

/// Extends the keys of the chunks transitions and the end card draw on, after
/// [`crate::overlay::key_chunks`] and [`ExportPlan::with_joins`]. A chunk under a transition adds
/// its kind, the window and edge relative to the chunk, the brand fields it draws in (the stinger
/// file, the background color of a dip or zoom-through, the shape and accent of a mask wipe), and
/// for a dissolve or mask wipe the key of the other side over the frames it shows. A chunk under
/// the end card adds its length, the chunk's place under it and the brand it draws in.
pub fn key_joins(plan: &mut ExportPlan, inputs: &KeyInputs) {
    let theme = inputs.theme;
    let mut extensions: Vec<(Piece, usize, Vec<u8>)> = Vec::new();
    for transition in &plan.transitions {
        let mut brand = Vec::new();
        match transition.kind {
            TransitionKind::Stinger => {
                if let Some(stinger) = &theme.stinger {
                    brand.extend_from_slice(&stinger.hash);
                }
            }
            TransitionKind::Dip | TransitionKind::ZoomThrough => {
                push_color(&mut brand, theme.palette.background.0);
            }
            TransitionKind::MaskWipe => {
                push_color(&mut brand, theme.palette.accent.0);
                brand.extend_from_slice(format!("{:?}", theme.wipe_shape()).as_bytes());
            }
            TransitionKind::Dissolve | TransitionKind::Cut => {}
        }
        for side in [transition.before, transition.after] {
            let other = if side == transition.before {
                transition.after
            } else {
                transition.before
            };
            let other_key = transition
                .two_sided()
                .then(|| side_key(plan, inputs, other, &transition.window));
            for (index, (start, chunk)) in plan.chunks_of(side).enumerate() {
                let end = start + chunk.frame_count();
                if end <= transition.window.start || transition.window.end <= start {
                    continue;
                }
                let relative = |frame: u64| (frame as i64 - start as i64).to_le_bytes();
                let input = framed(&[
                    b"transition",
                    transition.kind.as_str().as_bytes(),
                    &relative(transition.window.start),
                    &relative(transition.edge),
                    &relative(transition.window.end),
                    &brand,
                    other_key.as_ref().map_or(&[][..], |k| k.as_bytes()),
                ]);
                extensions.push((side, index, input));
            }
        }
    }
    if let Some(card) = plan.end_card.clone() {
        let pieces = plan.pieces.clone();
        for piece in pieces {
            for (index, (start, chunk)) in plan.chunks_of(piece).enumerate() {
                if start + chunk.frame_count() <= card.start {
                    continue;
                }
                let input = framed(&[
                    b"end-card",
                    &(card.end - card.start).to_le_bytes(),
                    &(start as i64 - card.start as i64).to_le_bytes(),
                    &theme.end_card_key(),
                ]);
                extensions.push((piece, index, input));
            }
        }
    }
    for (piece, index, input) in extensions {
        let chunk = match piece {
            Piece::Segment(i) => &mut plan.segments[i].chunks[index],
            Piece::Bookend(i) => &mut plan.bookends[i].chunks[index],
        };
        chunk.hash = chunk.hash.extend(&input);
    }
}

/// The key of what `piece` shows over the output frames `window`, its handles included: a
/// segment keyed like a chunk over those frames of its take, a bookend by its key and its frames.
fn side_key(
    plan: &ExportPlan,
    inputs: &KeyInputs,
    piece: Piece,
    window: &Range<u64>,
) -> ContentHash {
    let first = plan.position(piece, window.start);
    let last = plan.position(piece, window.end - 1);
    match piece {
        Piece::Segment(i) => {
            let segment = &plan.segments[i].segment;
            let frames = first..last + 1;
            let hash = inputs.output.chunk_hash(
                segment,
                (inputs.media)(&segment.take),
                plan.fps,
                frames.clone(),
            );
            match TimeRange::new(
                frame_time(frames.start, plan.fps),
                frame_time(frames.end, plan.fps),
            ) {
                Some(span) => {
                    let input =
                        crate::overlay::span_input(segment, span, inputs.identity, inputs.theme);
                    if input.is_empty() {
                        hash
                    } else {
                        hash.extend(&input)
                    }
                }
                None => hash,
            }
        }
        Piece::Bookend(i) => plan.bookends[i]
            .key
            .extend(&framed(&[&first.to_le_bytes(), &last.to_le_bytes()])),
    }
}

fn push_color(out: &mut Vec<u8>, rgba: [f32; 4]) {
    for channel in rgba {
        out.extend_from_slice(&channel.to_le_bytes());
    }
}

/// `parts`, each after its length, so distinct inputs never share bytes.
pub(crate) fn framed(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(&(part.len() as u64).to_le_bytes());
        out.extend_from_slice(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use shot_core::{Cut, FormatKind, Project, Script, Timeline, Video};

    const CUT: &str = r#"
chapter "Hook" {
    take t01 {
        keep from=0 to=4_000_000
    }
}
chapter "Two" {
    take t01 {
        keep from=5_000_000 to=9_000_000
    }
}
chapter "Three" {
    take t01 {
        keep from=9_000_000 to=12_000_000
    }
}
"#;

    fn output(cut: &str) -> Output {
        let project = Project {
            video: Video::parse("title \"t\"\nstatus editing\nformat opinion\n").unwrap(),
            script: Script::parse("# t\n").unwrap(),
            format: FormatKind::Opinion.preset(),
            cut: Cut::parse(cut).unwrap(),
        };
        Timeline::evaluate(&project).main().clone()
    }

    fn join(at: u64, target: Target, kind: TransitionKind) -> Join {
        Join {
            take: Id::new("t01").unwrap(),
            at: Micros(at),
            target,
            kind,
            length: Micros(500_000),
        }
    }

    fn bookend(length: u64, title: &str) -> BookendInput {
        BookendInput {
            bumper: Some("brand/media/intro.mp4".into()),
            title: Some(title.into()),
            length: Micros(length),
            identity: title.as_bytes().to_vec(),
        }
    }

    fn keyed(output: &Output, joins: &Joins) -> ExportPlan {
        let theme = Theme::default();
        let media = |_: &Id| MediaId::default();
        let mut plan = ExportPlan::new(output, 60, media);
        crate::overlay::key_chunks(&mut plan, &|_| Vec::new(), &theme);
        let mut plan = plan.with_joins(joins);
        key_joins(
            &mut plan,
            &KeyInputs {
                output,
                media: &media,
                identity: &|_| Vec::new(),
                theme: &theme,
            },
        );
        plan
    }

    fn hashes(plan: &ExportPlan) -> HashSet<ContentHash> {
        plan.output_chunks().map(|(_, c)| c.hash).collect()
    }

    #[test]
    fn the_intro_plays_after_the_hook_and_opens_the_second_chapter() {
        let joins = Joins {
            intro: Some(bookend(3_000_000, "Title")),
            outro: Some(bookend(2_000_000, "")),
            intro_after_hook: true,
            end_card: Some(Micros(5_000_000)),
            ..Joins::default()
        };
        let plan = ExportPlan::new(&output(CUT), 60, |_| MediaId::default()).with_joins(&joins);
        assert_eq!(
            plan.pieces,
            [
                Piece::Segment(0),
                Piece::Bookend(0),
                Piece::Segment(1),
                Piece::Segment(2),
                Piece::Bookend(1)
            ]
        );
        // 4 s hook, 3 s intro, 4 s and 3 s chapters, and the outro stretched to the 5 s card.
        assert_eq!(plan.range_of(Piece::Bookend(0)), 240..420);
        assert_eq!(plan.range_of(Piece::Segment(1)), 420..660);
        assert_eq!(plan.range_of(Piece::Bookend(1)), 840..1140);
        assert_eq!(plan.frames, 1140);
        let markers: Vec<u64> = plan.chapters.iter().map(|c| c.output_frame).collect();
        assert_eq!(markers, [0, 240, 660]);
        assert_eq!(plan.end_card, Some(840..1140));
        assert_eq!(plan.piece_at(419), Some(Piece::Bookend(0)));
        assert_eq!(plan.position(Piece::Bookend(1), 2000), 299);
        let starts: Vec<u64> = plan.output_chunks().map(|(at, _)| at).take(10).collect();
        assert_eq!(starts, [0, 30, 60, 90, 120, 150, 180, 210, 240, 270]);
    }

    #[test]
    fn transitions_center_on_their_edges_and_bookends_join_with_the_video_start_one() {
        let joins = Joins {
            intro: Some(bookend(3_000_000, "Title")),
            transitions: vec![
                join(0, Target::VideoStart, TransitionKind::Dip),
                join(0, Target::ChapterStart, TransitionKind::Dissolve),
                join(5_000_000, Target::ChapterStart, TransitionKind::Stinger),
                join(9_000_000, Target::ChapterStart, TransitionKind::Cut),
            ],
            stinger: Some(StingerTiming {
                duration: Micros(1_000_000),
                peak: Micros(400_000),
            }),
            ..Joins::default()
        };
        let plan = ExportPlan::new(&output(CUT), 60, |_| MediaId::default()).with_joins(&joins);
        let placed: Vec<_> = plan
            .transitions
            .iter()
            .map(|t| (t.kind, t.edge, t.window.clone(), t.before, t.after))
            .collect();
        assert_eq!(
            placed,
            [
                // Intro first: its join into the hook is the hook's chapter-start dissolve.
                (
                    TransitionKind::Dissolve,
                    180,
                    165..195,
                    Piece::Bookend(0),
                    Piece::Segment(0)
                ),
                // The stinger peaks on the cut, 24 frames after it starts.
                (
                    TransitionKind::Stinger,
                    420,
                    396..456,
                    Piece::Segment(0),
                    Piece::Segment(1)
                ),
            ]
        );
        let at = plan.join_at(170).unwrap();
        assert!(at.before_edge && (at.progress - 5.5 / 30.0).abs() < 1e-6);
        assert!((plan.join_at(194).unwrap().toward_edge - 0.5 / 15.0).abs() < 1e-6);
        assert!(plan.join_at(195).is_none());
        // The incoming side starts early into its take, the outgoing one holds its last frame.
        assert_eq!(plan.position(Piece::Segment(0), 170), 0);
        assert_eq!(plan.position(Piece::Bookend(0), 190), 179);
        assert_eq!(plan.position(Piece::Segment(0), 430), 250);
    }

    #[test]
    fn a_transition_re_keys_only_the_chunks_of_its_window() {
        let plain = keyed(&output(CUT), &Joins::default());
        let joins = Joins {
            transitions: vec![join(
                5_000_000,
                Target::ChapterStart,
                TransitionKind::Dissolve,
            )],
            ..Joins::default()
        };
        let dissolved = keyed(&output(CUT), &joins);
        let old = hashes(&plain);
        let changed: Vec<u64> = dissolved
            .output_chunks()
            .filter(|(_, c)| !old.contains(&c.hash))
            .map(|(at, _)| at)
            .collect();
        // The window is frames 225 to 255: the hook's last chunk and the next chapter's first.
        assert_eq!(changed, [210, 240]);

        // The other side's material is part of the key: moving the second chapter's start
        // re-keys the hook's last chunk, which shows the frames before it, and nothing outside
        // the window.
        let moved_joins = Joins {
            transitions: vec![join(
                5_500_000,
                Target::ChapterStart,
                TransitionKind::Dissolve,
            )],
            ..Joins::default()
        };
        let moved = keyed(
            &output(&CUT.replace("5_000_000", "5_500_000")),
            &moved_joins,
        );
        let dissolved_hashes = hashes(&dissolved);
        let changed: Vec<u64> = moved
            .output_chunks()
            .filter(|(_, c)| !dissolved_hashes.contains(&c.hash))
            .map(|(at, _)| at)
            .collect();
        assert_eq!(changed, [210, 240]);
    }

    #[test]
    fn bookend_chunks_depend_on_what_they_draw_not_on_the_video() {
        let joins = |title: &str| Joins {
            intro: Some(bookend(1_000_000, title)),
            ..Joins::default()
        };
        let one = ExportPlan::new(&output(CUT), 60, |_| MediaId::default()).with_joins(&joins("A"));
        let other_video =
            ExportPlan::new(&output(&CUT.replace("4_000_000", "3_000_000")), 60, |_| {
                MediaId::default()
            })
            .with_joins(&joins("A"));
        assert_eq!(one.bookends[0].chunks, other_video.bookends[0].chunks);
        let retitled =
            ExportPlan::new(&output(CUT), 60, |_| MediaId::default()).with_joins(&joins("B"));
        assert_ne!(
            one.bookends[0].chunks[0].hash,
            retitled.bookends[0].chunks[0].hash
        );
    }
}
