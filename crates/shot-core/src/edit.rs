//! Edits of `cut.kdl` as pure functions: each returns the new `Cut` and touches only the one
//! take block it is aimed at, so the app, CLI and MCP make the same small diff.

use std::fmt;

use crate::{
    Cut, Id, Layout, LayoutChange, Micros, RangeEdit, RemovedBy, TakeEdit, TimeRange, Transcript,
    Word,
};

/// One take block of the main cut: the chapter and the take's position in that chapter. A take
/// may appear more than once in a chapter, so the id alone is not an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakeAt {
    pub chapter: usize,
    pub take: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// No take block at this address.
    UnknownTake(TakeAt),
    /// The transcript belongs to another take than the addressed one.
    WrongTranscript { expected: Id, got: Id },
    /// The selection covers no word.
    NoWords,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTake(at) => {
                write!(f, "no take {} in chapter {}", at.take + 1, at.chapter + 1)
            }
            Self::WrongTranscript { expected, got } => {
                write!(
                    f,
                    "transcript is of take `{got}`, the edit is of `{expected}`"
                )
            }
            Self::NoWords => f.write_str("the selection covers no word"),
        }
    }
}

impl std::error::Error for EditError {}

/// Whether the edit keeps a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordState {
    Kept,
    Removed(RemovedBy),
}

/// A word of the take with what the edit does to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordView<'a> {
    pub word: &'a Word,
    pub state: WordState,
}

impl Cut {
    /// Removes the media of the words that overlap `selection` as `remove ... by=manual`. The
    /// cut runs from the start of the first word to the end of the last, so no word is half
    /// cut. Material that is already removed keeps its original `by`.
    pub fn delete_words(
        &self,
        at: TakeAt,
        transcript: &Transcript,
        selection: TimeRange,
    ) -> Result<Cut, EditError> {
        let edit = self.take(at)?;
        if transcript.take() != &edit.take {
            return Err(EditError::WrongTranscript {
                expected: edit.take.clone(),
                got: transcript.take().clone(),
            });
        }
        let mut covered = transcript
            .words()
            .iter()
            .filter(|w| w.range.overlaps(selection));
        let first = covered.next().ok_or(EditError::NoWords)?;
        let last = covered.next_back().unwrap_or(first);
        let span = TimeRange::new(first.range.from(), last.range.to())
            .expect("words are ordered, so the span is not empty");
        Ok(self.with_take(at, |edit| {
            replace_in_ranges(edit, span, |piece| match piece {
                RangeEdit::Keep(range) => RangeEdit::Remove {
                    range,
                    by: RemovedBy::Manual,
                },
                removed => removed,
            });
        }))
    }

    /// Turns every removed part of `range` back into kept media, whoever removed it.
    pub fn restore(&self, at: TakeAt, range: TimeRange) -> Result<Cut, EditError> {
        self.take(at)?;
        Ok(self.with_take(at, |edit| {
            replace_in_ranges(edit, range, |piece| RangeEdit::Keep(piece.range()));
        }))
    }

    /// Sets the layout over `range`, replacing and splitting the changes it overlaps and
    /// merging it with equal neighbours.
    pub fn set_layout(
        &self,
        at: TakeAt,
        range: TimeRange,
        layout: Layout,
    ) -> Result<Cut, EditError> {
        self.take(at)?;
        Ok(self.with_take(at, |edit| {
            clear_layouts(&mut edit.layouts, range);
            let index = edit
                .layouts
                .partition_point(|c| c.range.to() <= range.from());
            edit.layouts.insert(index, LayoutChange { layout, range });
            merge_layouts(&mut edit.layouts);
        }))
    }

    /// Returns `range` to the format's default layout.
    pub fn clear_layout(&self, at: TakeAt, range: TimeRange) -> Result<Cut, EditError> {
        self.take(at)?;
        Ok(self.with_take(at, |edit| clear_layouts(&mut edit.layouts, range)))
    }

    /// The transcript's words that lie in the addressed take block, each with its state, for
    /// rendering struck-through words. A word is judged by its midpoint, so a word cut by a
    /// rule at its edge counts as removed or kept by where most of it is.
    pub fn transcript_view<'a>(
        &self,
        at: TakeAt,
        transcript: &'a Transcript,
    ) -> Result<Vec<WordView<'a>>, EditError> {
        let edit = self.take(at)?;
        if transcript.take() != &edit.take {
            return Err(EditError::WrongTranscript {
                expected: edit.take.clone(),
                got: transcript.take().clone(),
            });
        }
        Ok(transcript
            .words()
            .iter()
            .filter_map(|word| {
                let middle = Micros((word.range.from().0 + word.range.to().0) / 2);
                let covering = edit.ranges.iter().find(|e| {
                    let r = e.range();
                    r.from() <= middle && middle < r.to()
                })?;
                let state = match covering {
                    RangeEdit::Keep(_) => WordState::Kept,
                    RangeEdit::Remove { by, .. } => WordState::Removed(*by),
                };
                Some(WordView { word, state })
            })
            .collect())
    }

    fn take(&self, at: TakeAt) -> Result<&TakeEdit, EditError> {
        self.chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .ok_or(EditError::UnknownTake(at))
    }

    /// Applies `change` to a copy of the cut. The address is checked by the caller.
    fn with_take(&self, at: TakeAt, change: impl FnOnce(&mut TakeEdit)) -> Cut {
        let mut cut = self.clone();
        change(&mut cut.chapters[at.chapter].takes[at.take]);
        cut
    }
}

/// Splits the take's ranges at `span`, passes each piece inside it to `change`, and merges
/// neighbours that became equal.
fn replace_in_ranges(
    edit: &mut TakeEdit,
    span: TimeRange,
    change: impl Fn(RangeEdit) -> RangeEdit,
) {
    let mut out: Vec<RangeEdit> = Vec::with_capacity(edit.ranges.len() + 2);
    for old in edit.ranges.drain(..) {
        let range = old.range();
        let Some(inside) = range.intersect(span) else {
            out.push(old);
            continue;
        };
        let with_range = |range| match old {
            RangeEdit::Keep(_) => RangeEdit::Keep(range),
            RangeEdit::Remove { by, .. } => RangeEdit::Remove { range, by },
        };
        let before = TimeRange::new(range.from(), inside.from());
        let after = TimeRange::new(inside.to(), range.to());
        out.extend(before.map(with_range));
        out.push(change(with_range(inside)));
        out.extend(after.map(with_range));
    }
    edit.ranges = merge_adjacent(out, |a, b| match (a, b) {
        (RangeEdit::Keep(_), RangeEdit::Keep(_)) => true,
        (RangeEdit::Remove { by: a, .. }, RangeEdit::Remove { by: b, .. }) => a == b,
        _ => false,
    });
}

fn merge_adjacent(
    items: Vec<RangeEdit>,
    same: impl Fn(RangeEdit, RangeEdit) -> bool,
) -> Vec<RangeEdit> {
    let mut out: Vec<RangeEdit> = Vec::with_capacity(items.len());
    for item in items {
        match out.last_mut() {
            Some(last) if last.range().to() == item.range().from() && same(*last, item) => {
                let range = TimeRange::new(last.range().from(), item.range().to())
                    .expect("adjacent ranges join into a non-empty range");
                *last = match *last {
                    RangeEdit::Keep(_) => RangeEdit::Keep(range),
                    RangeEdit::Remove { by, .. } => RangeEdit::Remove { range, by },
                };
            }
            _ => out.push(item),
        }
    }
    out
}

/// Cuts `range` out of the layout changes, splitting those that stick out on either side.
fn clear_layouts(layouts: &mut Vec<LayoutChange>, range: TimeRange) {
    let mut out = Vec::with_capacity(layouts.len() + 1);
    for change in layouts.drain(..) {
        if !change.range.overlaps(range) {
            out.push(change);
            continue;
        }
        let before = TimeRange::new(change.range.from(), range.from());
        let after = TimeRange::new(range.to(), change.range.to());
        out.extend(before.into_iter().chain(after).map(|r| LayoutChange {
            layout: change.layout,
            range: r,
        }));
    }
    *layouts = out;
}

fn merge_layouts(layouts: &mut Vec<LayoutChange>) {
    let mut out: Vec<LayoutChange> = Vec::with_capacity(layouts.len());
    for change in layouts.drain(..) {
        match out.last_mut() {
            Some(last)
                if last.layout == change.layout && last.range.to() == change.range.from() =>
            {
                last.range = TimeRange::new(last.range.from(), change.range.to())
                    .expect("adjacent ranges join into a non-empty range");
            }
            _ => out.push(change),
        }
    }
    *layouts = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuleKind;

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    fn keep(from: u64, to: u64) -> RangeEdit {
        RangeEdit::Keep(range(from, to))
    }

    fn removed(from: u64, to: u64, by: RemovedBy) -> RangeEdit {
        RangeEdit::Remove {
            range: range(from, to),
            by,
        }
    }

    const AT: TakeAt = TakeAt {
        chapter: 0,
        take: 0,
    };

    fn id() -> Id {
        Id::new("t01").unwrap()
    }

    /// Ten words of 1 s each with a 100 ms gap after each, from 0.
    fn transcript() -> Transcript {
        let mut t = Transcript::new(id());
        for i in 0..10u64 {
            t.push(Word {
                text: format!("w{i}"),
                range: range(i * 1_100_000, i * 1_100_000 + 1_000_000),
                confidence: 90,
            })
            .unwrap();
        }
        t
    }

    fn cut(ranges: Vec<RangeEdit>) -> Cut {
        Cut {
            chapters: vec![crate::Chapter {
                title: "A".to_owned(),
                takes: vec![TakeEdit {
                    take: id(),
                    ranges,
                    layouts: Vec::new(),
                    zooms: Vec::new(),
                    overlays: Vec::new(),
                }],
            }],
            ..Cut::default()
        }
    }

    fn ranges(cut: &Cut) -> &[RangeEdit] {
        &cut.chapters[0].takes[0].ranges
    }

    const MANUAL: RemovedBy = RemovedBy::Manual;

    #[test]
    fn delete_in_the_middle_splits_the_keep() {
        let c = cut(vec![keep(0, 11_000_000)]);
        // Words 2 and 3 are 2.2 s to 3.2 s and 3.3 s to 4.3 s.
        let after = c
            .delete_words(AT, &transcript(), range(2_500_000, 3_500_000))
            .unwrap();
        assert_eq!(
            ranges(&after),
            [
                keep(0, 2_200_000),
                removed(2_200_000, 4_300_000, MANUAL),
                keep(4_300_000, 11_000_000)
            ]
        );
    }

    #[test]
    fn delete_at_the_edges_leaves_one_keep() {
        let c = cut(vec![keep(0, 11_000_000)]);
        let head = c.delete_words(AT, &transcript(), range(0, 100)).unwrap();
        assert_eq!(
            ranges(&head),
            [removed(0, 1_000_000, MANUAL), keep(1_000_000, 11_000_000)]
        );
        let tail = c
            .delete_words(AT, &transcript(), range(10_000_000, 10_500_000))
            .unwrap();
        assert_eq!(
            ranges(&tail),
            [
                keep(0, 9_900_000),
                removed(9_900_000, 10_900_000, MANUAL),
                keep(10_900_000, 11_000_000)
            ]
        );
    }

    #[test]
    fn delete_snaps_to_word_boundaries() {
        let c = cut(vec![keep(0, 11_000_000)]);
        // Starts inside word 1 (1.1 s to 2.1 s), ends in the gap after word 2.
        let after = c
            .delete_words(AT, &transcript(), range(1_500_000, 3_250_000))
            .unwrap();
        assert_eq!(
            ranges(&after),
            [
                keep(0, 1_100_000),
                removed(1_100_000, 3_200_000, MANUAL),
                keep(3_200_000, 11_000_000)
            ]
        );
    }

    #[test]
    fn adjacent_manual_removals_merge_but_rule_removals_stay() {
        let silence = RemovedBy::Rule(RuleKind::Silence);
        let c = cut(vec![
            removed(0, 1_100_000, MANUAL),
            keep(1_100_000, 2_100_000),
            removed(2_100_000, 2_200_000, silence),
            keep(2_200_000, 11_000_000),
        ]);
        let t = transcript();
        let merged = c.delete_words(AT, &t, range(1_100_000, 2_000_000)).unwrap();
        assert_eq!(
            ranges(&merged),
            [
                removed(0, 2_100_000, MANUAL),
                removed(2_100_000, 2_200_000, silence),
                keep(2_200_000, 11_000_000)
            ]
        );
        let kept_apart = c.delete_words(AT, &t, range(2_200_000, 2_300_000)).unwrap();
        assert_eq!(
            ranges(&kept_apart)[2],
            removed(2_100_000, 2_200_000, silence)
        );
        assert_eq!(
            ranges(&kept_apart)[3],
            removed(2_200_000, 3_200_000, MANUAL)
        );
    }

    #[test]
    fn restore_returns_manual_and_rule_removals_to_kept_media() {
        let filler = RemovedBy::Rule(RuleKind::Filler);
        let c = cut(vec![
            keep(0, 1_000_000),
            removed(1_000_000, 2_000_000, MANUAL),
            keep(2_000_000, 3_000_000),
            removed(3_000_000, 4_000_000, filler),
        ]);
        let after = c.restore(AT, range(1_500_000, 3_500_000)).unwrap();
        assert_eq!(
            ranges(&after),
            [
                keep(0, 1_000_000),
                removed(1_000_000, 1_500_000, MANUAL),
                keep(1_500_000, 3_500_000),
                removed(3_500_000, 4_000_000, filler)
            ]
        );
        let all = c.restore(AT, range(0, 4_000_000)).unwrap();
        assert_eq!(ranges(&all), [keep(0, 4_000_000)]);
    }

    fn layouts(cut: &Cut) -> Vec<(Layout, u64, u64)> {
        cut.chapters[0].takes[0]
            .layouts
            .iter()
            .map(|c| (c.layout, c.range.from().0, c.range.to().0))
            .collect()
    }

    #[test]
    fn layout_set_replaces_splits_and_merges() {
        let c = cut(vec![keep(0, 100)]);
        let a = c.set_layout(AT, range(10, 50), Layout::Split).unwrap();
        let b = a.set_layout(AT, range(20, 30), Layout::Face).unwrap();
        assert_eq!(
            layouts(&b),
            [
                (Layout::Split, 10, 20),
                (Layout::Face, 20, 30),
                (Layout::Split, 30, 50)
            ]
        );
        let c = b.set_layout(AT, range(20, 30), Layout::Split).unwrap();
        assert_eq!(layouts(&c), [(Layout::Split, 10, 50)]);
        let d = c.set_layout(AT, range(50, 70), Layout::Split).unwrap();
        assert_eq!(layouts(&d), [(Layout::Split, 10, 70)]);
        let e = d.set_layout(AT, range(0, 80), Layout::Content).unwrap();
        assert_eq!(layouts(&e), [(Layout::Content, 0, 80)]);
    }

    #[test]
    fn layout_clear_returns_to_the_default() {
        let c = cut(vec![keep(0, 100)])
            .set_layout(AT, range(10, 50), Layout::Split)
            .unwrap();
        let cleared = c.clear_layout(AT, range(20, 30)).unwrap();
        assert_eq!(
            layouts(&cleared),
            [(Layout::Split, 10, 20), (Layout::Split, 30, 50)]
        );
        assert!(
            c.clear_layout(AT, range(0, 100)).unwrap().chapters[0].takes[0]
                .layouts
                .is_empty()
        );
    }

    #[test]
    fn transcript_view_marks_removed_words_and_who_removed_them() {
        let silence = RemovedBy::Rule(RuleKind::Silence);
        let c = cut(vec![
            removed(0, 1_100_000, silence),
            keep(1_100_000, 3_300_000),
            removed(3_300_000, 4_400_000, MANUAL),
            keep(4_400_000, 5_500_000),
        ]);
        let t = transcript();
        let view = c.transcript_view(AT, &t).unwrap();
        let states: Vec<_> = view.iter().map(|v| v.state).collect();
        assert_eq!(
            states,
            [
                WordState::Removed(silence),
                WordState::Kept,
                WordState::Kept,
                WordState::Removed(MANUAL),
                WordState::Kept,
            ]
        );
        assert_eq!(view[3].word.text, "w3");
    }

    #[test]
    fn bad_addresses_and_selections_are_errors() {
        let c = cut(vec![keep(0, 11_000_000)]);
        let missing = TakeAt {
            chapter: 0,
            take: 3,
        };
        assert_eq!(
            c.restore(missing, range(0, 1)),
            Err(EditError::UnknownTake(missing))
        );
        assert_eq!(
            c.delete_words(AT, &transcript(), range(1_000_100, 1_000_200)),
            Err(EditError::NoWords)
        );
        let other = Transcript::new(Id::new("t02").unwrap());
        assert!(matches!(
            c.delete_words(AT, &other, range(0, 1)),
            Err(EditError::WrongTranscript { .. })
        ));
    }
}
