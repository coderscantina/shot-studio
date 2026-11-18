//! What the transcript marks as worth a look ([0040]): cuts the rules were not sure of, restarts
//! they cut, and beats whose proposal may not be what the person wants. N steps through them,
//! K and C answer one, and A switches the beat at the playhead to its next attempt.
//!
//! [0040]: ../../../../docs/decisions/0040-the-cut-proposes-itself.md

use std::ops::Range;

use gpui::{App, Context, Div, FontWeight, Stateful, Window, div, prelude::*, px};
use shot_core::{Decision, ElementKind, Lineup, Look, Row, RowAt, TakeAt, TimeRange};

use super::timeline::Pick;
use super::{
    AROUND, CompareAttempts, CompareBeat, CutLook, EditScreen, KeepLook, NextAttempt, NextLook,
};
use crate::model::{FPS, Item, Selection};
use crate::takes::take_name;
use crate::theme::*;
use crate::ui::{hint, label};

/// A beat with more than one attempt, where the transcript shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beat {
    pub row: RowAt,
    /// The first word of what plays the beat.
    pub at: TakeAt,
    pub word: usize,
    /// The output frames the beat plays in.
    pub frames: Range<u64>,
    /// The attempts that play it, as indexes into the row's attempts.
    pub playing: Vec<usize>,
}

/// The lineup as the transcript shows it, rebuilt with the document.
#[derive(Default)]
pub struct Looks {
    pub lineup: Lineup,
    pub beats: Vec<Beat>,
    /// What is worth a look, with the output frame it sits at, in output order.
    pub items: Vec<(Look, u64)>,
    /// The item N last went to.
    pub current: Option<Look>,
    /// After an answer was written: go on to the next item once the document is rebuilt.
    go_on: bool,
    /// After a beat was switched: play it once the document is rebuilt.
    play: Option<RowAt>,
    /// The beat N or A played last.
    played: Option<RowAt>,
}

impl Looks {
    pub fn beat_at(&self, at: TakeAt, word: usize) -> Option<&Beat> {
        self.beats.iter().find(|b| b.at == at && b.word == word)
    }

    /// The beat with other attempts that plays at `frame`. The beat N or A just played wins
    /// while the playhead rests at its end, where the next beat begins.
    fn beat_playing(&self, frame: u64) -> Option<&Beat> {
        let rested = |b: &&Beat| {
            Some(b.row) == self.played && (b.frames.start..=b.frames.end + 6).contains(&frame)
        };
        self.beats
            .iter()
            .find(rested)
            .or_else(|| self.beats.iter().find(|b| b.frames.contains(&frame)))
    }

    fn row(&self, at: RowAt) -> Option<&Row> {
        self.lineup.row(at)
    }
}

/// How a finding's kind reads in a sentence.
fn kind_name(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Retake => "restart",
        ElementKind::Misspoken => "misspoken passage",
        ElementKind::Filler => "filler",
        ElementKind::Pause => "pause mid-sentence",
        ElementKind::Silence => "silence",
        ElementKind::Idle => "still screen",
        _ => "finding",
    }
}

impl EditScreen {
    /// Lines the takes up again and finds the beats and the items to look at in the document
    /// just built.
    pub(super) fn rebuild_looks(&mut self, cx: &mut Context<Self>) {
        let folder = self.folder.read(cx);
        let lineup = folder.lineup();
        let cut = &folder.project.cut;
        let mut beats = Vec::new();
        for (row_at, row) in lineup.rows().filter(|(_, row)| row.attempts.len() > 1) {
            let pieces = cut.plays(&lineup, row, None);
            let mut first = None;
            let mut frames: Option<Range<u64>> = None;
            for piece in &pieces {
                for block in self.doc.iter().filter(|b| b.take == piece.take) {
                    let inside = |range: TimeRange| {
                        let middle = (range.from().0 + range.to().0) / 2;
                        piece.range.from().0 <= middle && middle < piece.range.to().0
                    };
                    let word = block
                        .words
                        .iter()
                        .position(|w| w.output.is_some() && inside(w.range));
                    if let Some(word) = word {
                        first = first.or(Some((block.at, word)));
                    }
                    if let Some(out) = self.map.output_range(block.at, piece.range) {
                        frames = Some(match frames {
                            Some(all) => all.start.min(out.start)..all.end.max(out.end),
                            None => out,
                        });
                    }
                }
            }
            let playing = (0..row.attempts.len())
                .filter(|&i| {
                    let reach = lineup.padded(&row.attempts[i]);
                    pieces
                        .iter()
                        .any(|p| p.take == reach.take && p.range.overlaps(reach.range))
                })
                .collect();
            if let (Some((at, word)), Some(frames)) = (first, frames) {
                beats.push(Beat {
                    row: row_at,
                    at,
                    word,
                    frames,
                    playing,
                });
            }
        }
        let mut items: Vec<(Look, u64)> = cut
            .looks(&lineup, folder.lock())
            .into_iter()
            .filter_map(|look| {
                let frame = match look {
                    Look::Unsure { at, review } => {
                        self.map.output_frame_from(at, review.range.from())
                    }
                    Look::Restart { at, range, .. } => self.map.seam(at, range),
                    Look::Beat(row) => beats.iter().find(|b| b.row == row).map(|b| b.frames.start),
                }?;
                Some((look, frame))
            })
            .collect();
        items.sort_by_key(|(_, frame)| *frame);
        let looks = &mut self.looks;
        looks.current = looks
            .current
            .filter(|current| items.iter().any(|(look, _)| look == current));
        looks.lineup = lineup;
        looks.beats = beats;
        looks.items = items;
        if std::mem::take(&mut self.looks.go_on) {
            self.go_to_look(self.playhead, cx);
        }
        if let Some(row) = self.looks.play.take() {
            self.play_beat(row);
        }
    }

    /// Plays the beat at `row` and stops at its end.
    fn play_beat(&mut self, row: RowAt) {
        let beat = self.looks.beats.iter().find(|b| b.row == row);
        if let Some(frames) = beat.map(|b| b.frames.clone()) {
            self.looks.played = Some(row);
            self.play_stretch(frames);
        }
    }

    /// Plays `frames` and stops.
    fn play_stretch(&mut self, frames: Range<u64>) {
        self.seek(frames.start);
        self.set_playing(true);
        self.play_until = Some(frames.end);
    }

    /// Goes to the first item at or after `frame`, or around to the first one, shows it and
    /// plays it.
    fn go_to_look(&mut self, frame: u64, cx: &mut Context<Self>) {
        let items = &self.looks.items;
        let Some(&(look, at)) = items
            .iter()
            .find(|(_, at)| *at >= frame)
            .or_else(|| items.first())
        else {
            self.looks.current = None;
            return;
        };
        self.looks.current = Some(look);
        self.lanes.picked = None;
        self.selection = None;
        self.text.picked = None;
        match look {
            Look::Unsure { at: block, review } => {
                // Its words are selected, so the person sees what would go.
                let words: Vec<usize> = self
                    .block(block)
                    .into_iter()
                    .flat_map(|b| b.words.iter().enumerate())
                    .filter(|(_, w)| w.range.overlaps(review.range))
                    .map(|(i, _)| i)
                    .collect();
                if let (Some(&anchor), Some(&head)) = (words.first(), words.last()) {
                    self.selection = Some(Selection {
                        at: block,
                        anchor,
                        head,
                    });
                }
                let second = u64::from(FPS);
                let length = review.range.duration().0 * second / 1_000_000;
                self.play_stretch(at.saturating_sub(second)..at + length + second);
            }
            Look::Restart {
                at: block, range, ..
            } => {
                // The chip that holds the cut is picked, so R and ⇧Space work on it.
                let run = self
                    .block(block)
                    .into_iter()
                    .flat_map(|b| &b.paragraphs)
                    .flat_map(|p| &p.items)
                    .find_map(|item| match item {
                        Item::Cut(run) if run.range.overlaps(range) => Some(run.range),
                        _ => None,
                    });
                self.lanes.picked = run.map(|run| Pick::Cut(block, run));
                self.play_stretch(at.saturating_sub(AROUND)..at + AROUND);
            }
            Look::Beat(row) => self.play_beat(row),
        }
        cx.notify();
    }

    pub(super) fn next_look(&mut self, _: &NextLook, _: &mut Window, cx: &mut Context<Self>) {
        // From the item N last went to, on to the one after it.
        let from = self
            .looks
            .current
            .and_then(|current| self.looks.items.iter().find(|(look, _)| *look == current))
            .map_or(self.playhead, |(_, frame)| frame + 1);
        self.go_to_look(from, cx);
    }

    /// The item K and C answer: the one N went to, else the one at the playhead.
    fn look_here(&self) -> Option<Look> {
        self.looks.current.or_else(|| {
            let second = u64::from(FPS);
            self.looks
                .items
                .iter()
                .find(|(look, frame)| match look {
                    Look::Beat(row) => self
                        .looks
                        .beats
                        .iter()
                        .any(|b| b.row == *row && b.frames.contains(&self.playhead)),
                    _ => frame.abs_diff(self.playhead) <= second,
                })
                .map(|(look, _)| *look)
        })
    }

    /// K keeps what plays: an unsure finding stays, a cut restart plays again, a beat keeps its
    /// proposal as the person's pick. C cuts: the finding goes, the restart stays cut.
    fn answer_look(&mut self, decision: Decision, cx: &mut Context<Self>) {
        let Some(look) = self.look_here() else {
            return;
        };
        // The lineup is lent to the edit and rebuilt after it.
        let lineup = &std::mem::take(&mut self.looks.lineup);
        let wrote = self.edit(cx, |cut| {
            Ok(match look {
                Look::Unsure { at, review } => {
                    let take = &cut.chapters[at.chapter].takes[at.take].take;
                    cut.answer(take, review.kind, review.t, decision)
                }
                Look::Restart { at, kind, t, .. } => {
                    let take = &cut.chapters[at.chapter].takes[at.take].take;
                    cut.answer(take, kind, t, decision)
                }
                Look::Beat(at) => match (lineup.row(at), decision) {
                    (Some(row), Decision::Keep) => cut.set_pick(row, &cut.plays(lineup, row, None)),
                    _ => cut.clone(),
                },
            })
        });
        if wrote {
            self.looks.current = None;
            self.lanes.picked = None;
            self.selection = None;
            self.looks.go_on = true;
        }
    }

    pub(super) fn keep_look(&mut self, _: &KeepLook, _: &mut Window, cx: &mut Context<Self>) {
        self.answer_look(Decision::Keep, cx);
    }

    pub(super) fn cut_look(&mut self, _: &CutLook, _: &mut Window, cx: &mut Context<Self>) {
        self.answer_look(Decision::Cut, cx);
    }

    /// A: the beat at the playhead plays its next attempt, as the person's pick, and is played.
    pub(super) fn next_attempt(&mut self, _: &NextAttempt, _: &mut Window, cx: &mut Context<Self>) {
        let Some(beat) = self.looks.beat_playing(self.playhead).cloned() else {
            return;
        };
        let lineup = &std::mem::take(&mut self.looks.lineup);
        let Some(row) = lineup.row(beat.row) else {
            return;
        };
        let next = beat
            .playing
            .last()
            .map_or(0, |last| (last + 1) % row.attempts.len());
        let attempt = row.attempts[next].clone();
        let wrote = self.edit(cx, |cut| {
            cut.set_pick(row, std::slice::from_ref(&attempt))
                .compose(lineup, None)
        });
        if wrote {
            self.selection = None;
            self.lanes.picked = None;
            self.looks.play = Some(beat.row);
        }
    }

    /// ⇧A: the takes side by side, on the beat at the playhead.
    pub(super) fn compare_attempts(
        &mut self,
        _: &CompareAttempts,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.looks.beat_playing(self.playhead).map(|b| b.row) {
            self.set_playing(false);
            cx.emit(CompareBeat(row));
        }
    }
}

/// "3 to look at" beside the chapter title. A click goes to the next one, as N does.
pub fn count(this: &EditScreen, cx: &mut Context<EditScreen>) -> Option<Stateful<Div>> {
    let count = this.looks.items.len();
    (count > 0).then(|| {
        div()
            .id("to-look-at")
            .flex()
            .items_center()
            .gap(px(7.))
            .h(px(26.))
            .px(px(10.))
            .rounded(RADIUS_SM)
            .bg(RAISED)
            .text_size(TEXT_SM)
            .font_weight(FontWeight::MEDIUM)
            .text_color(TEXT)
            .cursor_pointer()
            .hover(|s| s.text_color(STRONG))
            .child(div().size(px(6.)).rounded_full().bg(ACCENT))
            .child(format!("{count} to look at"))
            .child(crate::ui::kbd("N"))
            .on_click(cx.listener(|this, _, window, cx| this.next_look(&NextLook, window, cx)))
    })
}

/// The marker where a beat with other attempts starts: which attempt plays, of how many.
pub fn beat_chip(this: &EditScreen, beat: &Beat, cx: &mut Context<EditScreen>) -> Stateful<Div> {
    let of = this.looks.row(beat.row).map_or(0, |row| row.attempts.len());
    let playing: Vec<String> = beat.playing.iter().map(|i| (i + 1).to_string()).collect();
    let open = this
        .looks
        .items
        .iter()
        .any(|(look, _)| *look == Look::Beat(beat.row));
    let frames = beat.frames.clone();
    div()
        .id(("beat", beat.row.group * 10_000 + beat.row.row))
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .px(px(6.))
        .mr(px(6.))
        .rounded(px(5.))
        .bg(RAISED)
        .text_color(if open { TEXT } else { MUTED })
        .font_family(MONO)
        .text_size(TEXT_XS)
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(open, |chip| {
            chip.child(div().size(px(5.)).rounded_full().bg(ACCENT))
        })
        .child(format!("{} of {of}", playing.join("+")))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.selection = None;
            this.lanes.picked = None;
            this.seek(frames.start);
            cx.notify();
        }))
}

/// The inspector's section for the item N went to, or for the beat at the playhead: what it is
/// and the keys that answer it.
pub fn section(this: &EditScreen, cx: &App) -> Option<Div> {
    let beat = this.looks.beat_playing(this.playhead);
    let look = this.looks.current.or_else(|| {
        let row = beat?.row;
        this.looks
            .items
            .iter()
            .map(|(look, _)| *look)
            .find(|look| *look == Look::Beat(row))
    });
    if look.is_none() && beat.is_none() {
        return None;
    }
    let title = |text: &'static str| div().font_weight(FontWeight::SEMIBOLD).child(text);
    let text = |text: String| div().text_color(BODY).line_height(px(21.)).child(text);
    let mut section = div().flex().flex_col().gap(px(8.));
    match look {
        Some(Look::Unsure { review, .. }) => {
            section = section
                .child(title("Worth a look"))
                .child(text(format!(
                    "The rules found a {} here and were not sure enough to cut it. It plays.",
                    kind_name(review.kind)
                )))
                .child(hint("C", "Cut it"))
                .child(hint("K", "Keep it"));
        }
        Some(Look::Restart { kind, .. }) => {
            section = section
                .child(title("Worth a look"))
                .child(text(format!("The rules cut a {} here.", kind_name(kind))))
                .child(hint("⇧Space", "Hear the cut"))
                .child(hint("C", "The cut is right"))
                .child(hint("K", "Keep the words, they play again"));
        }
        Some(Look::Beat(_)) | None => {}
    }
    if let Some(beat) =
        beat.filter(|_| !matches!(look, Some(Look::Unsure { .. } | Look::Restart { .. })))
        && let Some(row) = this.looks.row(beat.row)
    {
        let cut = &this.folder.read(cx).project.cut;
        let picked = !cut.picked(row).is_empty();
        let open = look.is_some();
        section = section
            .child(title(if open { "Worth a look" } else { "This beat" }))
            .child(text(row.text.clone()))
            .children(row.attempts.iter().enumerate().map(|(i, attempt)| {
                let plays = beat.playing.contains(&i);
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(TEXT_SM)
                    .text_color(if plays { TEXT } else { MUTED })
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(if plays { ACCENT } else { FAINT }),
                    )
                    .child(format!("{}  {}", i + 1, take_name(&attempt.take)))
                    .child(div().flex_1())
                    .child(label(format!("{} of 100", row.scores[i])))
            }))
            .child(label(match (picked, beat.playing.len()) {
                (true, _) => "Your pick. It stays when a later take scores better.",
                (false, 1) if open => "An earlier or a close attempt plays. Hear the others.",
                (false, 1) => "The best scored attempt plays.",
                (false, _) => "Sentences of more than one attempt play. Hear the joins.",
            }))
            .child(hint("A", "Play the next attempt and use it"))
            .child(hint("⇧A", "Compare the attempts side by side"))
            .when(open, |section| section.child(hint("K", "Keep what plays")));
    }
    Some(section.when(!this.looks.items.is_empty(), |section| {
        section.child(hint("N", "The next one to look at"))
    }))
}
