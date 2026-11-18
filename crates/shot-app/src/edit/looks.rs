//! What the transcript marks as worth a look ([0040]): cuts the rules were not sure of, restarts
//! they cut, and beats whose proposal may not be what the person wants. N steps through them,
//! K and C answer one, and A switches the beat at the playhead to its next attempt.
//!
//! [0040]: ../../../../docs/decisions/0040-the-cut-proposes-itself.md

use std::ops::Range;

use gpui::{App, Context, Div, FontWeight, Stateful, Window, div, prelude::*, px};
use shot_core::{
    Attempt, Decision, EditError, ElementKind, Lineup, Look, Row, RowAt, TakeAt, TimeRange,
};

use super::timeline::Pick;
use super::{
    AROUND, CompareAttempts, CompareBeat, CutLook, EditScreen, KeepLook, NextAttempt, NextLook,
    Workspace,
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

/// How many frames past the end of what N or A played the playhead still rests on it.
const REST: u64 = 6;

/// The attempts of `row` that `pieces` play, as indexes into the row's attempts.
fn playing(lineup: &Lineup, row: &Row, pieces: &[Attempt]) -> Vec<usize> {
    (0..row.attempts.len())
        .filter(|&i| {
            let reach = lineup.padded(&row.attempts[i]);
            pieces
                .iter()
                .any(|p| p.take == reach.take && p.range.overlaps(reach.range))
        })
        .collect()
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
            Some(b.row) == self.played && (b.frames.start..=b.frames.end + REST).contains(&frame)
        };
        self.beats
            .iter()
            .find(rested)
            .or_else(|| self.beats.iter().find(|b| b.frames.contains(&frame)))
    }

    fn row(&self, at: RowAt) -> Option<&Row> {
        self.lineup.row(at)
    }

    /// The one item the inspector shows and K and C answer with the playhead at `frame`:
    /// `current`, the item N went to, else the beat that plays there while it is one to look
    /// at, else the first finding within a second.
    fn target(&self, current: Option<Look>, frame: u64) -> Option<Look> {
        let open = |look: &Look| self.items.iter().any(|(item, _)| item == look);
        let beat = || {
            let beat = Look::Beat(self.beat_playing(frame)?.row);
            open(&beat).then_some(beat)
        };
        let finding = || {
            let near = self.items.iter().find(|(look, at)| {
                !matches!(look, Look::Beat(_)) && at.abs_diff(frame) <= u64::from(FPS)
            });
            near.map(|(look, _)| *look)
        };
        current.or_else(beat).or_else(finding)
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
            let playing = playing(&lineup, row, &pieces);
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

    /// Whether the review keys act: only in Text, where the items show. Elsewhere the key
    /// goes on to whatever else is bound to it.
    fn reviews(&self, cx: &mut Context<Self>) -> bool {
        let text = self.workspace == Workspace::Text;
        if !text {
            cx.propagate();
        }
        text
    }

    pub(super) fn next_look(&mut self, _: &NextLook, _: &mut Window, cx: &mut Context<Self>) {
        if !self.reviews(cx) {
            return;
        }
        // From the item N last went to, on to the one after it.
        let from = self
            .current_look()
            .and_then(|current| self.looks.items.iter().find(|(look, _)| *look == current))
            .map_or(self.playhead, |(_, frame)| frame + 1);
        self.go_to_look(from, cx);
    }

    /// The item N went to, while the playhead is still on what N played of it. After a seek,
    /// a scrub or playing on, it is no longer the one the person sees.
    pub(super) fn current_look(&self) -> Option<Look> {
        let current = self.looks.current?;
        let (_, frame) = self.looks.items.iter().find(|(look, _)| *look == current)?;
        let second = u64::from(FPS);
        let played = match current {
            Look::Unsure { review, .. } => {
                let length = review.range.duration().0 * second / 1_000_000;
                frame.saturating_sub(second)..=frame + length + second + REST
            }
            Look::Restart { .. } => frame.saturating_sub(AROUND)..=frame + AROUND + REST,
            Look::Beat(row) => {
                let beat = self.looks.beats.iter().find(|b| b.row == row)?;
                beat.frames.start..=beat.frames.end + REST
            }
        };
        played.contains(&self.playhead).then_some(current)
    }

    /// The item K and C answer and the inspector shows, see [`Looks::target`].
    fn look_here(&self) -> Option<Look> {
        self.looks.target(self.current_look(), self.playhead)
    }

    /// K keeps what plays: an unsure finding stays, a cut restart plays again. C cuts: the
    /// finding goes, the restart stays cut. Either answers a beat as looked at, which is no
    /// pick: a later take that scores better still plays it.
    fn answer_look(&mut self, decision: Decision, cx: &mut Context<Self>) {
        if !self.reviews(cx) {
            return;
        }
        let Some(look) = self.look_here() else {
            return;
        };
        // The lineup is lent to the edit and comes back, for the rebuild to replace.
        let lineup = std::mem::take(&mut self.looks.lineup);
        let wrote = self.edit(cx, |cut| {
            let lineup = &lineup;
            Ok(match look {
                Look::Unsure { at, review } => {
                    let take = &cut.chapters[at.chapter].takes[at.take].take;
                    cut.answer(take, review.kind, review.t, decision)
                }
                Look::Restart { at, kind, t, .. } => {
                    let take = &cut.chapters[at.chapter].takes[at.take].take;
                    cut.answer(take, kind, t, decision)
                }
                Look::Beat(at) => {
                    let row = lineup.row(at).ok_or(EditError::UnknownRow(at))?;
                    cut.set_looked(row, &row.proposal)
                }
            })
        });
        self.looks.lineup = lineup;
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
    /// From sentences of several attempts it goes to the first attempt whole, and after the
    /// last attempt back to those sentences, the proposal, so the spliced sentence is not lost.
    /// Going back to the proposal holds also after a word was cut in the attempt that played:
    /// the cut stays with that attempt ([`shot_core::Cut::propose_row`]).
    pub(super) fn next_attempt(&mut self, _: &NextAttempt, _: &mut Window, cx: &mut Context<Self>) {
        if !self.reviews(cx) {
            return;
        }
        let Some(beat) = self.looks.beat_playing(self.playhead).cloned() else {
            return;
        };
        let Some(row) = self.looks.row(beat.row) else {
            return;
        };
        let spliced = playing(&self.looks.lineup, row, &row.proposal).len() > 1;
        let chosen: Vec<Attempt> = next_attempt(&beat.playing, row.attempts.len(), spliced)
            .map(|i| row.attempts[i].clone())
            .into_iter()
            .collect();
        // The lineup is lent to the edit and comes back: a refused switch, as when a Short
        // uses the take that would leave, changes nothing.
        let lineup = std::mem::take(&mut self.looks.lineup);
        let wrote = self.edit(cx, |cut| {
            let row = lineup
                .row(beat.row)
                .ok_or(EditError::UnknownRow(beat.row))?;
            if chosen.is_empty() {
                cut.propose_row(&lineup, beat.row)
            } else {
                cut.set_pick(row, &chosen).compose(&lineup, None)
            }
        });
        self.looks.lineup = lineup;
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

/// What A plays after the attempts at `playing`, of a row with `attempts`: the next attempt
/// whole, from sentences of several attempts the first one, and `None`, the proposal again,
/// after the last attempt of a row whose proposal is `spliced` from several.
fn next_attempt(playing: &[usize], attempts: usize, spliced: bool) -> Option<usize> {
    match *playing {
        [last] if last + 1 < attempts => Some(last + 1),
        [_] if spliced => None,
        _ => Some(0),
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

/// Whether [`section`] lists the attempts of the beat at the playhead: it has more than one,
/// and N did not go to a finding.
pub fn shows_attempts(this: &EditScreen) -> bool {
    let finding = matches!(
        this.current_look(),
        Some(Look::Unsure { .. } | Look::Restart { .. })
    );
    !finding && this.looks.beat_playing(this.playhead).is_some()
}

/// The inspector's section for the item K and C answer ([`Looks::target`]) and for the beat at
/// the playhead: what it is and the keys that answer it. A finding N went to shows alone, one
/// near the playhead above the beat it is in.
pub fn section(this: &EditScreen, cx: &App) -> Option<Div> {
    let beat = this.looks.beat_playing(this.playhead);
    let look = this.look_here();
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
    if let Some(beat) = beat.filter(|_| shows_attempts(this))
        && let Some(row) = this.looks.row(beat.row)
    {
        let cut = &this.folder.read(cx).project.cut;
        let picked = !cut.picked(row).is_empty();
        // K answers the beat only while it is the target, not a finding near the playhead.
        let open = matches!(look, Some(Look::Beat(_)));
        // The keys come first: a long list of attempts must not push them out of sight.
        section = section
            .child(title(if open { "Worth a look" } else { "This beat" }))
            .child(text(row.text.clone()))
            .when(open, |section| {
                section.child(hint("K", "It is fine as it plays"))
            })
            .child(hint("A", "Play the next attempt and use it"))
            .child(hint("⇧A", "Compare the attempts side by side"))
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
            }));
    }
    Some(section.when(!this.looks.items.is_empty(), |section| {
        section.child(hint("N", "The next one to look at"))
    }))
}

#[cfg(test)]
mod tests {
    use shot_core::{ElementKind, Look, Micros, RowAt, TakeAt, TimeRange};

    use super::{Beat, Looks, next_attempt};

    #[test]
    fn the_inspector_and_k_answer_the_beat_before_a_restart_near_the_playhead() {
        let at = TakeAt {
            chapter: 0,
            take: 0,
        };
        let row = RowAt { group: 0, row: 1 };
        let restart = Look::Restart {
            at,
            kind: ElementKind::Retake,
            t: Micros(9_000_000),
            range: TimeRange::new(Micros(9_000_000), Micros(9_800_000)).unwrap(),
        };
        // A restart was cut 20 frames before a contested beat starts.
        let looks = Looks {
            beats: vec![Beat {
                row,
                at,
                word: 12,
                frames: 600..900,
                playing: vec![0],
            }],
            items: vec![(restart, 580), (Look::Beat(row), 600)],
            ..Looks::default()
        };
        // On a word near the start of the beat, the beat is the target, not the restart.
        assert_eq!(looks.target(None, 610), Some(Look::Beat(row)));
        // What N went to wins, and outside the beat the restart near the playhead does.
        assert_eq!(looks.target(Some(restart), 610), Some(restart));
        assert_eq!(looks.target(None, 590), Some(restart));
        assert_eq!(looks.target(None, 400), None);
    }

    #[test]
    fn a_steps_through_the_whole_attempts_and_back_to_the_spliced_proposal() {
        // Sentences of both attempts play: the first attempt whole, the second, the splice.
        assert_eq!(next_attempt(&[0, 1], 2, true), Some(0));
        assert_eq!(next_attempt(&[0], 2, true), Some(1));
        assert_eq!(next_attempt(&[1], 2, true), None);
        // Without a splice the last attempt goes on to the first.
        assert_eq!(next_attempt(&[2], 3, false), Some(0));
    }
}
