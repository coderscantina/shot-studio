//! Edits of `cut.kdl` as pure functions: each returns the new `Cut` and touches only the one
//! take block it is aimed at, so the app, CLI and MCP make the same small diff.

use std::fmt;

use crate::takes::{holds, middle};
use crate::{
    Applied, Attempt, Chapter, Cut, EffectKey, ElementKind, Id, Instance, Layer, Layout,
    LayoutChange, Lineup, Lock, Micros, Music, Overlay, Override, OverrideEffect, Pick, RangeEdit,
    Row, RowAt, TakeEdit, Target, TimeRange, Transcript, Word, Zoom, ZoomShape, word_cut,
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
    /// No open review item with exactly this range in the addressed take block.
    NotInReview(TimeRange),
    /// No zoom or overlay at this index in the addressed take block.
    UnknownPlaced(TakeAt, Placed),
    /// No row at this place in the lineup.
    UnknownRow(RowAt),
    /// The edit would take a take out of the main cut that a derived cut still uses.
    TakeInDerived { derived: Id, take: Id },
    /// No `layout` range starts or ends at this media time in the addressed take block.
    NoLayoutEdge(Micros),
    /// The main cut keeps nothing of this take at this media time.
    NotKept { take: Id, at: Micros },
}

/// A zoom or an overlay of a take block, by its index in the block's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placed {
    Zoom(usize),
    Overlay(usize),
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
            Self::NotInReview(range) => write!(
                f,
                "no open review item from {} to {} microseconds",
                range.from().0,
                range.to().0
            ),
            Self::NoLayoutEdge(t) => {
                write!(f, "no layout change starts or ends at {} microseconds", t.0)
            }
            Self::UnknownPlaced(at, placed) => {
                let (what, index) = match placed {
                    Placed::Zoom(index) => ("zoom", index),
                    Placed::Overlay(index) => ("overlay", index),
                };
                write!(
                    f,
                    "no {what} {} in take {} of chapter {}",
                    index + 1,
                    at.take + 1,
                    at.chapter + 1
                )
            }
            Self::UnknownRow(at) => {
                write!(f, "no beat {} in chapter {}", at.row + 1, at.group + 1)
            }
            Self::TakeInDerived { derived, take } => write!(
                f,
                "`{derived}` uses take {take}, which would leave the cut: remove its clips first"
            ),
            Self::NotKept { take, at } => write!(
                f,
                "the cut keeps nothing of take {take} at {:.2} s",
                at.0 as f64 / 1e6
            ),
        }
    }
}

impl std::error::Error for EditError {}

/// A person's answer to a review item: K or C in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Keep,
    Cut,
}

/// Who removed a word: a person in `cut.kdl`, or a cut the style rules made on an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovedBy {
    Manual,
    Rule(Target),
}

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

/// An open item of the review queue: a lock's `review` instance that plays in a take block,
/// on an element of `kind` starting at `t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Review {
    pub range: TimeRange,
    pub kind: ElementKind,
    pub t: Micros,
}

impl Cut {
    /// Appends a chapter named `title` with one block of `take` that keeps all of `range`, the
    /// way a finished recording enters the cut.
    pub fn append_take(&self, take: Id, title: impl Into<String>, range: TimeRange) -> Cut {
        let mut cut = self.clone();
        cut.chapters.push(Chapter {
            title: title.into(),
            takes: vec![TakeEdit {
                take,
                ranges: vec![RangeEdit::Keep(range)],
                layouts: Vec::new(),
                zooms: Vec::new(),
                overlays: Vec::new(),
            }],
        });
        cut
    }

    /// The open review items of the main cut in cut order: the lock's `review` instances of
    /// each block's take that lie in its kept material.
    pub fn review_queue<'a>(
        &'a self,
        lock: &'a Lock,
    ) -> impl Iterator<Item = (TakeAt, Review)> + 'a {
        self.chapters
            .iter()
            .enumerate()
            .flat_map(move |(chapter, c)| {
                c.takes.iter().enumerate().flat_map(move |(take, edit)| {
                    let instances = lock.take(&edit.take).into_iter().flat_map(|t| &t.instances);
                    instances.filter_map(move |i| match (&i.applied, i.element.target) {
                        (&Applied::Review(range), Target::Kind(kind)) if edit.keeps(range) => {
                            Some((
                                TakeAt { chapter, take },
                                Review {
                                    range,
                                    kind,
                                    t: i.element.t,
                                },
                            ))
                        }
                        _ => None,
                    })
                })
            })
    }

    /// K or C on the review item at `range` of the block at `at`: `override { keep }` or
    /// `override { cut }` on its element.
    pub fn decide(
        &self,
        at: TakeAt,
        range: TimeRange,
        decision: Decision,
        lock: &Lock,
    ) -> Result<Cut, EditError> {
        let (_, review) = self
            .review_queue(lock)
            .find(|(a, r)| *a == at && r.range == range)
            .ok_or(EditError::NotInReview(range))?;
        let take = self.take(at)?.take.clone();
        Ok(self.with_override(&take, review.kind, review.t, |o| {
            o.decision = Some(decision);
        }))
    }

    /// Removes the media of the words that overlap `selection` as `remove ... by=manual`, with
    /// one of the pauses around them, see [`word_cut`]. No word is half cut. A cut of the
    /// `lock` that lies in that pause stays the rules' own: the removal stops at it.
    pub fn delete_words(
        &self,
        at: TakeAt,
        transcript: &Transcript,
        selection: TimeRange,
        lock: &Lock,
    ) -> Result<Cut, EditError> {
        let edit = self.take(at)?;
        if transcript.take() != &edit.take {
            return Err(EditError::WrongTranscript {
                expected: edit.take.clone(),
                got: transcript.take().clone(),
            });
        }
        let words = transcript.words();
        let first = words
            .iter()
            .position(|w| w.range.overlaps(selection))
            .ok_or(EditError::NoWords)?;
        let last = words
            .iter()
            .rposition(|w| w.range.overlaps(selection))
            .unwrap_or(first);
        let span = word_cut(
            first.checked_sub(1).map(|i| words[i].range),
            words[first].range,
            words[last].range,
            words.get(last + 1).map(|w| w.range),
        );
        let (start, end) = (words[first].range.from(), words[last].range.to());
        let (mut from, mut to) = (span.from(), span.to());
        for cut in lock.take(&edit.take).into_iter().flat_map(|t| t.cuts()) {
            if cut.from() < start && cut.to() > from {
                from = cut.to().min(start);
            }
            if cut.to() > end && cut.from() < to {
                to = cut.from().max(end);
            }
        }
        let span = TimeRange::new(from, to).expect("a cut holds its words");
        Ok(self.with_take(at, |edit| {
            replace_in_ranges(edit, span, |piece| RangeEdit::Remove(piece.range()));
        }))
    }

    /// Turns every removed part of `range` of the block at `at` back into kept media: a
    /// person's removals there become kept, and each of the lock's cuts there gets
    /// `override { keep }` on its element, so the rules leave it.
    pub fn restore(&self, at: TakeAt, range: TimeRange, lock: &Lock) -> Result<Cut, EditError> {
        let take = self.take(at)?.take.clone();
        let mut restored = self.with_take(at, |edit| {
            replace_in_ranges(edit, range, |p| RangeEdit::Keep(p.range()))
        });
        for instance in lock.take(&take).iter().flat_map(|t| &t.instances) {
            if let (&Applied::Cut(cut), Target::Kind(kind)) =
                (&instance.applied, instance.element.target)
                && cut.overlaps(range)
            {
                restored = restored.with_override(&take, kind, instance.element.t, |o| {
                    o.decision = Some(Decision::Keep);
                });
            }
        }
        Ok(restored)
    }

    /// The cut with the override of (`take`, `kind`, `at`) changed by `change`, added when
    /// there is none, in canonical order.
    pub fn with_override(
        &self,
        take: &Id,
        kind: ElementKind,
        at: Micros,
        change: impl FnOnce(&mut Override),
    ) -> Cut {
        let mut cut = self.clone();
        let index = match cut
            .overrides
            .iter()
            .position(|o| o.take == *take && o.kind == kind && o.at == at)
        {
            Some(index) => index,
            None => {
                cut.overrides.push(Override {
                    take: take.clone(),
                    kind,
                    at,
                    decision: None,
                    effects: Vec::new(),
                });
                cut.overrides.len() - 1
            }
        };
        change(&mut cut.overrides[index]);
        let order: Vec<Id> = cut
            .chapters
            .iter()
            .flat_map(|c| &c.takes)
            .map(|b| b.take.clone())
            .collect();
        cut.overrides
            .sort_by_key(|o| (order.iter().position(|t| *t == o.take), o.at, o.kind));
        cut
    }

    /// Sets the layout over `range`, replacing and splitting the changes it overlaps and
    /// merging it with equal neighbours. A switch written on an edge `range` shares with a
    /// change it replaces stays on that edge.
    pub fn set_layout(
        &self,
        at: TakeAt,
        range: TimeRange,
        layout: Layout,
    ) -> Result<Cut, EditError> {
        self.take(at)?;
        Ok(self.with_take(at, |edit| {
            let mut change = LayoutChange::new(layout, range);
            for old in &edit.layouts {
                if old.range.from() == range.from() {
                    change.enter.clone_from(&old.enter);
                }
                if old.range.to() == range.to() {
                    change.exit.clone_from(&old.exit);
                }
            }
            clear_layouts(&mut edit.layouts, range);
            let index = edit
                .layouts
                .partition_point(|c| c.range.to() <= range.from());
            edit.layouts.insert(index, change);
            merge_layouts(&mut edit.layouts);
        }))
    }

    /// Writes how the layout switch at `edge` of the block at `at` moves, or with `None` gives
    /// it back to the format's `layout-switch`. The switch belongs to the change starting at
    /// `edge` (its `in`), else to the one ending there (its `out`).
    pub fn set_switch(
        &self,
        at: TakeAt,
        edge: Micros,
        switch: Option<crate::Transition>,
    ) -> Result<Cut, EditError> {
        let layouts = &self.take(at)?.layouts;
        let starts = layouts.iter().position(|c| c.range.from() == edge);
        let ends = layouts.iter().position(|c| c.range.to() == edge);
        if starts.is_none() && ends.is_none() {
            return Err(EditError::NoLayoutEdge(edge));
        }
        Ok(self.with_take(at, |edit| match starts {
            Some(i) => edit.layouts[i].enter = switch,
            None => edit.layouts[ends.expect("checked above")].exit = switch,
        }))
    }

    /// Returns `range` to the format's default layout.
    pub fn clear_layout(&self, at: TakeAt, range: TimeRange) -> Result<Cut, EditError> {
        self.take(at)?;
        Ok(self.with_take(at, |edit| clear_layouts(&mut edit.layouts, range)))
    }

    /// Adds a person's zoom at `scale` percent over `range`, replacing the zooms it overlaps.
    pub fn add_zoom(&self, at: TakeAt, range: TimeRange, scale: u16) -> Result<Cut, EditError> {
        self.take(at)?;
        Ok(self.with_take(at, |edit| {
            edit.zooms.retain(|z| !z.range.overlaps(range));
            let index = edit
                .zooms
                .partition_point(|z| z.range.from() < range.from());
            edit.zooms.insert(
                index,
                Zoom {
                    scale: scale.max(100),
                    range,
                    center: None,
                    shape: ZoomShape::default(),
                },
            );
        }))
    }

    /// Moves the zoom or overlay `placed` of the block at `at` to `range`. Zooms it now
    /// overlaps are dropped, so zooms never stack.
    pub fn set_range(
        &self,
        at: TakeAt,
        placed: Placed,
        range: TimeRange,
    ) -> Result<Cut, EditError> {
        self.placed(at, placed)?;
        Ok(self.with_take(at, |edit| match placed {
            Placed::Zoom(index) => {
                let mut zoom = edit.zooms.remove(index);
                zoom.range = range;
                edit.zooms.retain(|z| !z.range.overlaps(range));
                let index = edit
                    .zooms
                    .partition_point(|z| z.range.from() < range.from());
                edit.zooms.insert(index, zoom);
            }
            Placed::Overlay(index) => edit.overlays[index].range = range,
        }))
    }

    /// Sets the zoom `index` of the block at `at` to `scale` percent, at least 100.
    pub fn set_zoom_scale(&self, at: TakeAt, index: usize, scale: u16) -> Result<Cut, EditError> {
        self.placed(at, Placed::Zoom(index))?;
        Ok(self.with_take(at, |edit| {
            let zoom = &mut edit.zooms[index];
            zoom.scale = scale.max(100);
        }))
    }

    /// Sets how the zoom `index` of the block at `at` frames and ramps. A quarter framing
    /// raises the scale to the 200 percent it needs.
    pub fn set_zoom_shape(
        &self,
        at: TakeAt,
        index: usize,
        shape: ZoomShape,
    ) -> Result<Cut, EditError> {
        self.placed(at, Placed::Zoom(index))?;
        Ok(self.with_take(at, |edit| {
            let zoom = &mut edit.zooms[index];
            zoom.scale = zoom.scale.max(shape.framing.min_scale());
            zoom.shape = shape;
        }))
    }

    /// Sets or, with `None`, removes the music bed. Gains above 0 dB are lowered to 0.
    pub fn set_music(&self, music: Option<Music>) -> Cut {
        Cut {
            music: music.map(|m| Music {
                gain: m.gain.min(0),
                duck: m.duck.min(0),
                ..m
            }),
            ..self.clone()
        }
    }

    /// Removes the zoom or overlay `placed` of the block at `at`.
    pub fn remove_placed(&self, at: TakeAt, placed: Placed) -> Result<Cut, EditError> {
        self.placed(at, placed)?;
        Ok(self.with_take(at, |edit| match placed {
            Placed::Zoom(index) => {
                edit.zooms.remove(index);
            }
            Placed::Overlay(index) => {
                edit.overlays.remove(index);
            }
        }))
    }

    /// Removes the lock's zoom, overlay or sound `instance` of `take` through the override of
    /// its element: a sound a person placed loses its `sfx` node, any other zoom, overlay or
    /// sound is turned off (`zoom off`, `overlay <template> off`, `sfx off`). `None` for other
    /// instances and for instances of no element kind.
    pub fn remove_instance(&self, take: &Id, instance: &Instance) -> Option<Cut> {
        let Target::Kind(kind) = instance.element.target else {
            return None;
        };
        let key = match &instance.applied {
            Applied::Zoom { .. } => EffectKey::Zoom,
            Applied::Overlay { template, .. } => EffectKey::Overlay(template.clone()),
            Applied::Sfx { .. } => EffectKey::Sfx,
            _ => return None,
        };
        let at = instance
            .provenance
            .override_at
            .unwrap_or(instance.element.t);
        let placed_sound = key == EffectKey::Sfx && instance.provenance.layer == Layer::Override;
        let mut cut = self.with_override(take, kind, at, |o| {
            o.effects.retain(|e| e.key() != key);
            if !placed_sound {
                o.effects.push(OverrideEffect::Off(key));
                o.effects.sort_by_key(OverrideEffect::key);
            }
        });
        cut.overrides
            .retain(|o| o.decision.is_some() || !o.effects.is_empty());
        Some(cut)
    }

    fn placed(&self, at: TakeAt, placed: Placed) -> Result<(), EditError> {
        let edit = self.take(at)?;
        let exists = match placed {
            Placed::Zoom(index) => index < edit.zooms.len(),
            Placed::Overlay(index) => index < edit.overlays.len(),
        };
        exists
            .then_some(())
            .ok_or(EditError::UnknownPlaced(at, placed))
    }

    /// The transcript's words that lie in the addressed take block, each with its state, for
    /// rendering struck-through words: removed by a person's cut, by one of the lock's cuts, or
    /// kept. A word is judged by its midpoint, so a word cut at its edge counts as removed or
    /// kept by where most of it is.
    pub fn transcript_view<'a>(
        &self,
        at: TakeAt,
        transcript: &'a Transcript,
        lock: &Lock,
    ) -> Result<Vec<WordView<'a>>, EditError> {
        let edit = self.take(at)?;
        if transcript.take() != &edit.take {
            return Err(EditError::WrongTranscript {
                expected: edit.take.clone(),
                got: transcript.take().clone(),
            });
        }
        let cuts: Vec<(TimeRange, Target)> = lock
            .take(&edit.take)
            .into_iter()
            .flat_map(|t| &t.instances)
            .filter_map(|i| match i.applied {
                Applied::Cut(range) => Some((range, i.element.target)),
                _ => None,
            })
            .collect();
        let inside = |r: TimeRange, t: Micros| r.from() <= t && t < r.to();
        Ok(transcript
            .words()
            .iter()
            .filter_map(|word| {
                let middle = Micros((word.range.from().0 + word.range.to().0) / 2);
                let covering = edit.ranges.iter().find(|e| inside(e.range(), middle))?;
                let state = match covering {
                    RangeEdit::Remove(_) => WordState::Removed(RemovedBy::Manual),
                    RangeEdit::Keep(_) => match cuts.iter().find(|(r, _)| inside(*r, middle)) {
                        Some((_, target)) => WordState::Removed(RemovedBy::Rule(*target)),
                        None => WordState::Kept,
                    },
                };
                Some(WordView { word, state })
            })
            .collect())
    }

    /// Places `overlay`, a written node, in the block of the main cut that keeps `take` at the
    /// overlay's start, over the block's other overlays: how a person or a tool places an
    /// overlay preset ([`crate::OverlayPreset::place`]).
    pub fn place_overlay(&self, take: &Id, overlay: Overlay) -> Result<Cut, EditError> {
        let at = overlay.range.from();
        let kept = TimeRange::new(at, Micros(at.0 + 1)).expect("a microsecond");
        let mut cut = self.clone();
        let block = cut
            .chapters
            .iter_mut()
            .flat_map(|c| &mut c.takes)
            .find(|edit| edit.take == *take && edit.keeps(kept))
            .ok_or_else(|| EditError::NotKept {
                take: take.clone(),
                at,
            })?;
        block.overlays.push(overlay);
        Ok(cut)
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

/// Something worth a person's look, see [`Cut::looks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// A finding the rules were not sure of. It plays until it is answered.
    Unsure { at: TakeAt, review: Review },
    /// A restart or a misspoken passage the rules cut.
    Restart {
        at: TakeAt,
        kind: ElementKind,
        t: Micros,
        range: TimeRange,
    },
    /// A beat whose proposal may not be what the person wants.
    Beat(RowAt),
}

/// Composing the main cut from the attempts of several takes, see [`Lineup`] and [0040].
///
/// [0040]: ../../../docs/decisions/0040-the-cut-proposes-itself.md
impl Cut {
    /// A person's picks among the attempts of `row`, those whose middle lies in an attempt, in
    /// the order the cut plays them and then in canonical order. Mostly one: the attempt they
    /// switched the beat to. Several when they kept a proposal that plays more than one.
    pub fn picked(&self, row: &Row) -> Vec<&Pick> {
        let mut picked: Vec<&Pick> = self
            .picks
            .iter()
            .filter(|p| {
                row.attempts
                    .iter()
                    .any(|a| a.take == p.take && holds(a.range, middle(p.range)))
            })
            .collect();
        let place = |pick: &Pick| {
            self.chapters
                .iter()
                .flat_map(|c| &c.takes)
                .position(|edit| {
                    edit.take == pick.take
                        && edit.ranges.iter().any(|r| r.range().overlaps(pick.range))
                })
                .unwrap_or(usize::MAX)
        };
        picked.sort_by_key(|pick| place(pick));
        picked
    }

    /// Picks `chosen`, attempts of `row` or parts of them, in place of the row's picks. None
    /// takes the picks away, so the row's proposal plays again.
    pub fn set_pick(&self, row: &Row, chosen: &[Attempt]) -> Cut {
        let mut cut = self.clone();
        cut.picks.retain(|p| {
            !row.attempts
                .iter()
                .any(|a| a.take == p.take && a.range.overlaps(p.range))
        });
        cut.picks.extend(chosen.iter().map(|chosen| Pick {
            take: chosen.take.clone(),
            range: chosen.range,
        }));
        cut.picks.sort_by(|a, b| a.order().cmp(&b.order()));
        cut
    }

    /// How much of `range` of `take` the main cut keeps, over every block of the take.
    pub fn kept_of(&self, take: &Id, range: TimeRange) -> Micros {
        let kept = self
            .chapters
            .iter()
            .flat_map(|c| &c.takes)
            .filter(|edit| edit.take == *take)
            .flat_map(|edit| &edit.ranges)
            .filter_map(|piece| match piece {
                RangeEdit::Keep(kept) => kept.intersect(range),
                RangeEdit::Remove(_) => None,
            })
            .map(|r| r.duration().0)
            .sum();
        Micros(kept)
    }

    /// The pieces that play `row`, in order: a person's picks, else what the cut holds of a
    /// row a person edited, else the row's proposal. A pick of nearly a whole attempt follows
    /// the attempt when a new transcript moves its edges. `new` is a take that was just
    /// appended whole: the cut holds all of it, and none of it by a person's choice.
    pub fn plays(&self, lineup: &Lineup, row: &Row, new: Option<&Id>) -> Vec<Attempt> {
        let picked = self.picked(row);
        if picked.is_empty() {
            return self
                .held_by_hand(lineup, row, new)
                .unwrap_or_else(|| row.proposal.clone());
        }
        picked
            .into_iter()
            .map(|pick| {
                let whole = row.attempts.iter().find(|a| {
                    a.take == pick.take
                        && pick
                            .range
                            .intersect(a.range)
                            .is_some_and(|both| 10 * both.duration().0 >= 9 * a.range.duration().0)
                });
                let piece = whole.cloned().unwrap_or_else(|| Attempt {
                    take: pick.take.clone(),
                    range: pick.range,
                });
                lineup.padded(&piece)
            })
            .collect()
    }

    /// What a person may want to look at before the video ships, in no order: findings the
    /// rules were not sure of, restarts and misspoken passages the rules cut and nobody
    /// answered, and beats whose proposal is worth a look ([`Row::look`]) and that a person
    /// neither picked nor edited.
    pub fn looks(&self, lineup: &Lineup, lock: &Lock) -> Vec<Look> {
        let mut looks: Vec<Look> = self
            .review_queue(lock)
            .map(|(at, review)| Look::Unsure { at, review })
            .collect();
        for (chapter, c) in self.chapters.iter().enumerate() {
            for (take, edit) in c.takes.iter().enumerate() {
                let instances = lock.take(&edit.take).into_iter().flat_map(|t| &t.instances);
                looks.extend(
                    instances.filter_map(|i| match (&i.applied, i.element.target) {
                        (
                            &Applied::Cut(range),
                            Target::Kind(kind @ (ElementKind::Retake | ElementKind::Misspoken)),
                        ) if i.provenance.rule.is_some()
                            && edit.ranges.iter().any(|r| {
                                matches!(r, RangeEdit::Keep(_)) && r.range().overlaps(range)
                            }) =>
                        {
                            Some(Look::Restart {
                                at: TakeAt { chapter, take },
                                kind,
                                t: i.element.t,
                                range,
                            })
                        }
                        _ => None,
                    }),
                );
            }
        }
        looks.extend(
            lineup
                .rows()
                .filter(|(_, row)| {
                    row.look
                        && self.picked(row).is_empty()
                        && self.held_by_hand(lineup, row, None).is_none()
                })
                .map(|(at, _)| Look::Beat(at)),
        );
        looks
    }

    /// Answers the finding of `kind` at `t` of `take`: `keep` plays it, `cut` cuts it whatever
    /// the rules' confidence. Either way it is no longer something to look at.
    pub fn answer(&self, take: &Id, kind: ElementKind, t: Micros, decision: Decision) -> Cut {
        self.with_override(take, kind, t, |o| o.decision = Some(decision))
    }

    /// What the cut holds of `row`, in cut order, when a person cut words in it or answered a
    /// finding in it. Such a row is theirs: a proposal never replaces it.
    fn held_by_hand(&self, lineup: &Lineup, row: &Row, new: Option<&Id>) -> Option<Vec<Attempt>> {
        let mut held = Vec::new();
        let mut by_hand = false;
        let blocks = self.chapters.iter().flat_map(|c| &c.takes);
        for edit in blocks.filter(|edit| Some(&edit.take) != new) {
            for attempt in row.attempts.iter().filter(|a| a.take == edit.take) {
                let reach = lineup.padded(attempt).range;
                let mut inside = edit
                    .ranges
                    .iter()
                    .filter_map(|r| r.range().intersect(reach));
                let Some(first) = inside.next() else {
                    continue;
                };
                let last = inside.next_back().unwrap_or(first);
                by_hand |= edit.ranges.iter().any(|r| {
                    matches!(r, RangeEdit::Remove(_)) && r.range().overlaps(attempt.range)
                }) || self
                    .overrides
                    .iter()
                    .any(|o| o.take == edit.take && holds(attempt.range, o.at));
                held.extend(
                    TimeRange::new(first.from(), last.to()).map(|range| Attempt {
                        take: edit.take.clone(),
                        range,
                    }),
                );
            }
        }
        by_hand.then_some(held)
    }

    /// Builds the main cut from the lineup: one chapter per group under its title, each row
    /// played by [`Cut::plays`]. Neighbouring pieces of one take join into one block that keeps
    /// what lies between them, unless an attempt of that take lies there. A row without
    /// attempts is left out, and a take the lineup does not know, such as one without words,
    /// keeps its blocks after the composed chapters. Without beats and without a row that has
    /// two attempts there is nothing to choose, and the cut stays as it is. `new` is the take
    /// that was just appended, see [`Cut::plays`].
    pub fn compose(&self, lineup: &Lineup, new: Option<&Id>) -> Result<Cut, EditError> {
        let everything: Vec<&Attempt> = lineup.rows().flat_map(|(_, r)| &r.attempts).collect();
        if everything.is_empty() || !(lineup.scripted || lineup.contested()) {
            return Ok(self.clone());
        }
        let mut chapters = Vec::new();
        for group in &lineup.groups {
            let mut spans: Vec<Attempt> = Vec::new();
            for piece in group
                .rows
                .iter()
                .flat_map(|row| self.plays(lineup, row, new))
            {
                if let Some(last) = spans.last_mut()
                    && last.take == piece.take
                    && last.range.to() <= piece.range.from()
                    && window(last.range.to(), piece.range.from()).is_none_or(|gap| {
                        !everything
                            .iter()
                            .any(|a| a.take == piece.take && a.range.overlaps(gap))
                    })
                {
                    last.range = TimeRange::new(last.range.from(), piece.range.to())
                        .expect("the later piece ends after the earlier starts");
                    continue;
                }
                spans.push(piece);
            }
            if spans.is_empty() {
                continue;
            }
            chapters.push(Chapter {
                title: group.title.clone(),
                takes: spans
                    .iter()
                    .map(|a| self.material(&a.take, a.range))
                    .collect(),
            });
        }
        let lined = lineup.takes();
        for chapter in &self.chapters {
            let takes: Vec<TakeEdit> = chapter
                .takes
                .iter()
                .filter(|edit| !lined.contains(&edit.take))
                .cloned()
                .collect();
            if !takes.is_empty() {
                chapters.push(Chapter {
                    title: chapter.title.clone(),
                    takes,
                });
            }
        }
        Cut {
            chapters,
            ..self.clone()
        }
        .settle_takes()
    }

    /// A block of `take` over `range` with what the cut's blocks of that take have there: their
    /// keeps and removes, layouts, zooms and overlays. Where no block covers the take's
    /// media, the block keeps it.
    pub fn material(&self, take: &Id, range: TimeRange) -> TakeEdit {
        let mut out = TakeEdit {
            take: take.clone(),
            ranges: Vec::new(),
            layouts: Vec::new(),
            zooms: Vec::new(),
            overlays: Vec::new(),
        };
        // A take that appears twice gives what its first block has.
        for piece in self
            .chapters
            .iter()
            .flat_map(|c| &c.takes)
            .filter(|edit| edit.take == *take)
            .filter_map(|edit| restrict(edit, range))
        {
            for r in piece.ranges {
                if free(r.range(), out.ranges.iter().map(|o| o.range())) {
                    out.ranges.push(r);
                }
            }
            for l in piece.layouts {
                if free(l.range, out.layouts.iter().map(|o| o.range)) {
                    out.layouts.push(l);
                }
            }
            for z in piece.zooms {
                if free(z.range, out.zooms.iter().map(|o| o.range)) {
                    out.zooms.push(z);
                }
            }
            for o in piece.overlays {
                if !out.overlays.contains(&o) {
                    out.overlays.push(o);
                }
            }
        }
        out.ranges.sort_by_key(|r| r.range().from());
        let mut filled = Vec::with_capacity(out.ranges.len() * 2 + 1);
        let mut cursor = range.from();
        for piece in out.ranges.drain(..) {
            filled.extend(window(cursor, piece.range().from()).map(RangeEdit::Keep));
            cursor = piece.range().to();
            filled.push(piece);
        }
        filled.extend(window(cursor, range.to()).map(RangeEdit::Keep));
        out.ranges = merge_adjacent(filled);
        out.layouts.sort_by_key(|l| l.range.from());
        out.zooms.sort_by_key(|z| z.range.from());
        out
    }

    /// Drops the overrides of takes that left the main cut and puts the rest in canonical order;
    /// refuses when a derived cut still uses a take that left.
    fn settle_takes(mut self) -> Result<Cut, EditError> {
        let mut order: Vec<Id> = Vec::new();
        for edit in self.chapters.iter().flat_map(|c| &c.takes) {
            if !order.contains(&edit.take) {
                order.push(edit.take.clone());
            }
        }
        for derived in &self.derived {
            if let Some(clip) = derived.clips.iter().find(|c| !order.contains(&c.take)) {
                return Err(EditError::TakeInDerived {
                    derived: derived.name.clone(),
                    take: clip.take.clone(),
                });
            }
        }
        self.overrides.retain(|o| order.contains(&o.take));
        self.overrides
            .sort_by_key(|o| (order.iter().position(|t| *t == o.take), o.at, o.kind));
        Ok(self)
    }
}

/// Whether `range` overlaps none of `taken`.
fn free(range: TimeRange, mut taken: impl Iterator<Item = TimeRange>) -> bool {
    !taken.any(|t| t.overlaps(range))
}

/// `[from, to)`, or `None` when it is empty.
fn window(from: Micros, to: Micros) -> Option<TimeRange> {
    TimeRange::new(from, to)
}

/// What of `edit` lies inside `window`: ranges, layouts and zooms cut to it, and the
/// overlays that start in it. `None` when no kept or removed media is left.
fn restrict(edit: &TakeEdit, window: TimeRange) -> Option<TakeEdit> {
    let ranges: Vec<RangeEdit> = edit
        .ranges
        .iter()
        .filter_map(|piece| {
            let inside = piece.range().intersect(window)?;
            Some(piece.with_range(inside))
        })
        .collect();
    if ranges.is_empty() {
        return None;
    }
    Some(TakeEdit {
        take: edit.take.clone(),
        ranges,
        layouts: edit
            .layouts
            .iter()
            .filter_map(|l| {
                let range = l.range.intersect(window)?;
                // A switch only stays on an edge the window leaves where it was.
                Some(LayoutChange {
                    layout: l.layout,
                    range,
                    enter: l.enter.clone().filter(|_| range.from() == l.range.from()),
                    exit: l.exit.clone().filter(|_| range.to() == l.range.to()),
                })
            })
            .collect(),
        zooms: edit
            .zooms
            .iter()
            .filter_map(|z| {
                Some(Zoom {
                    range: z.range.intersect(window)?,
                    ..*z
                })
            })
            .collect(),
        overlays: edit
            .overlays
            .iter()
            .filter(|o| holds(window, o.range.from()))
            .cloned()
            .collect(),
    })
}

fn same_kind(a: RangeEdit, b: RangeEdit) -> bool {
    std::mem::discriminant(&a) == std::mem::discriminant(&b)
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
        let with_range = |range| old.with_range(range);
        let before = TimeRange::new(range.from(), inside.from());
        let after = TimeRange::new(inside.to(), range.to());
        out.extend(before.map(with_range));
        out.push(change(with_range(inside)));
        out.extend(after.map(with_range));
    }
    edit.ranges = merge_adjacent(out);
}

/// Joins neighbours that meet and are both kept or both removed.
fn merge_adjacent(items: Vec<RangeEdit>) -> Vec<RangeEdit> {
    let mut out: Vec<RangeEdit> = Vec::with_capacity(items.len());
    for item in items {
        match out.last_mut() {
            Some(last) if last.range().to() == item.range().from() && same_kind(*last, item) => {
                let range = TimeRange::new(last.range().from(), item.range().to())
                    .expect("adjacent ranges join into a non-empty range");
                *last = last.with_range(range);
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
        if let Some(before) = TimeRange::new(change.range.from(), range.from()) {
            out.push(LayoutChange {
                range: before,
                exit: None,
                ..change.clone()
            });
        }
        if let Some(after) = TimeRange::new(range.to(), change.range.to()) {
            out.push(LayoutChange {
                range: after,
                enter: None,
                ..change
            });
        }
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
                last.exit = change.exit;
            }
            _ => out.push(change),
        }
    }
    *layouts = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    fn keep(from: u64, to: u64) -> RangeEdit {
        RangeEdit::Keep(range(from, to))
    }

    fn removed(from: u64, to: u64) -> RangeEdit {
        RangeEdit::Remove(range(from, to))
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

    /// A lock of `t01` with a sure silence cut, an unsure pause and a filler cut.
    fn lock() -> Lock {
        Lock::parse(
            "lock 1\n\ntake t01 {\n    cut from=1_000_000 to=2_000_000 on=silence t=1_000_000 layer=format rule=\"silence\"\n    review from=6_000_000 to=7_000_000 on=pause t=6_000_000 layer=format rule=\"pause\"\n    cut from=8_000_000 to=9_000_000 on=filler t=8_000_000 layer=format rule=\"filler\"\n}\n",
        )
        .unwrap()
    }

    #[test]
    fn answers_and_restores_write_overrides_on_the_element() {
        let (cut, lock) = (cut(vec![keep(0, 10_000_000)]), lock());
        let review = range(6_000_000, 7_000_000);
        let queue: Vec<_> = cut.review_queue(&lock).collect();
        assert_eq!(queue.len(), 1);
        assert_eq!(
            (queue[0].1.range, queue[0].1.kind),
            (review, ElementKind::Pause)
        );

        let cut_it = cut.decide(AT, review, Decision::Cut, &lock).unwrap();
        let kept = cut_it.decide(AT, review, Decision::Keep, &lock).unwrap();
        assert_eq!(kept.overrides.len(), 1, "one override per element");
        assert!(
            kept.to_kdl()
                .ends_with("override t01 pause at=6_000_000 {\n    keep\n}\n")
        );
        assert_eq!(
            cut.decide(AT, range(1, 2), Decision::Keep, &lock),
            Err(EditError::NotInReview(range(1, 2)))
        );

        let manual = cut
            .delete_words(
                AT,
                &transcript(),
                range(3_300_000, 3_400_000),
                &Lock::default(),
            )
            .unwrap();
        let restored = manual.restore(AT, range(0, 5_000_000), &lock).unwrap();
        assert_eq!(ranges(&restored), [keep(0, 10_000_000)]);
        let kinds: Vec<_> = restored.overrides.iter().map(|o| (o.kind, o.at)).collect();
        assert_eq!(kinds, [(ElementKind::Silence, Micros(1_000_000))]);
        assert_eq!(Cut::parse(&restored.to_kdl()).unwrap(), restored);
    }

    fn ranges(cut: &Cut) -> &[RangeEdit] {
        &cut.chapters[0].takes[0].ranges
    }

    #[test]
    fn append_take_adds_a_canonical_chapter_that_keeps_the_whole_take() {
        let appended = cut(vec![keep(0, 1_000_000)]).append_take(
            Id::new("t02").unwrap(),
            "Take t02",
            range(0, 12_500_000),
        );
        assert_eq!(appended.chapters.len(), 2);
        assert!(appended.to_kdl().ends_with(
            "chapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=12_500_000\n    }\n}\n"
        ));
    }

    #[test]
    fn delete_in_the_middle_splits_the_keep() {
        let c = cut(vec![keep(0, 11_000_000)]);
        // Words 2 and 3 are 2.2 s to 3.2 s and 3.3 s to 4.3 s. The cut starts half the gap
        // before word 2 and ends half the gap before word 4, taking the pause after word 3.
        let after = c
            .delete_words(
                AT,
                &transcript(),
                range(2_500_000, 3_500_000),
                &Lock::default(),
            )
            .unwrap();
        assert_eq!(
            ranges(&after),
            [
                keep(0, 2_150_000),
                removed(2_150_000, 4_350_000),
                keep(4_350_000, 11_000_000)
            ]
        );
    }

    #[test]
    fn delete_at_the_edges_leaves_one_keep() {
        let c = cut(vec![keep(0, 11_000_000)]);
        let head = c
            .delete_words(AT, &transcript(), range(0, 100), &Lock::default())
            .unwrap();
        assert_eq!(
            ranges(&head),
            [removed(0, 1_050_000), keep(1_050_000, 11_000_000)]
        );
        let tail = c
            .delete_words(
                AT,
                &transcript(),
                range(10_000_000, 10_500_000),
                &Lock::default(),
            )
            .unwrap();
        assert_eq!(
            ranges(&tail),
            [
                keep(0, 9_850_000),
                removed(9_850_000, 10_980_000),
                keep(10_980_000, 11_000_000)
            ]
        );
    }

    #[test]
    fn delete_snaps_to_word_boundaries() {
        let c = cut(vec![keep(0, 11_000_000)]);
        // Starts inside word 1 (1.1 s to 2.1 s), ends in the gap after word 2.
        let after = c
            .delete_words(
                AT,
                &transcript(),
                range(1_500_000, 3_250_000),
                &Lock::default(),
            )
            .unwrap();
        assert_eq!(
            ranges(&after),
            [
                keep(0, 1_050_000),
                removed(1_050_000, 3_250_000),
                keep(3_250_000, 11_000_000)
            ]
        );
    }

    #[test]
    fn adjacent_removals_merge() {
        let c = cut(vec![removed(0, 1_100_000), keep(1_100_000, 11_000_000)]);
        let merged = c
            .delete_words(
                AT,
                &transcript(),
                range(1_100_000, 2_000_000),
                &Lock::default(),
            )
            .unwrap();
        assert_eq!(
            ranges(&merged),
            [removed(0, 2_150_000), keep(2_150_000, 11_000_000)]
        );
    }

    #[test]
    fn delete_leaves_a_rules_cut_in_the_pause_it_takes() {
        // A person cut "uh", a rule the silence after it. The longer pause follows "it", so
        // its cut takes the pause before, up to the silence.
        let mut spoken = Transcript::new(id());
        for (text, from, to) in [
            ("ship", 100_000, 600_000),
            ("uh", 1_100_000, 1_400_000),
            ("it", 2_100_000, 2_400_000),
            ("now", 3_500_000, 3_900_000),
        ] {
            spoken
                .push(Word {
                    text: text.to_owned(),
                    range: range(from, to),
                    confidence: 90,
                })
                .unwrap();
        }
        let c = cut(vec![
            keep(0, 1_000_000),
            removed(1_000_000, 1_500_000),
            keep(1_500_000, 4_000_000),
        ]);
        let lock = Lock::parse(
            "lock 1\n\ntake t01 {\n    cut from=1_500_000 to=2_000_000 on=silence t=1_500_000 layer=format rule=\"silence\"\n}\n",
        )
        .unwrap();
        let after = c
            .delete_words(AT, &spoken, range(2_100_000, 2_400_000), &lock)
            .unwrap();
        assert_eq!(
            ranges(&after),
            [
                keep(0, 1_000_000),
                removed(1_000_000, 1_500_000),
                keep(1_500_000, 2_000_000),
                removed(2_000_000, 2_480_000),
                keep(2_480_000, 4_000_000)
            ]
        );
    }

    #[test]
    fn restore_returns_removals_to_kept_media() {
        let c = cut(vec![
            keep(0, 1_000_000),
            removed(1_000_000, 2_000_000),
            keep(2_000_000, 3_000_000),
        ]);
        let lock = Lock::default();
        let after = c.restore(AT, range(1_500_000, 3_000_000), &lock).unwrap();
        assert_eq!(
            ranges(&after),
            [
                keep(0, 1_000_000),
                removed(1_000_000, 1_500_000),
                keep(1_500_000, 3_000_000),
            ]
        );
        let all = c.restore(AT, range(0, 3_000_000), &lock).unwrap();
        assert_eq!(ranges(&all), [keep(0, 3_000_000)]);
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
    fn a_switch_stays_on_its_edge_through_layout_edits() {
        let dissolve = Some(crate::Transition::new(crate::TransitionKind::Dissolve));
        let switches = |c: &Cut| -> Vec<(bool, bool)> {
            c.chapters[0].takes[0]
                .layouts
                .iter()
                .map(|l| (l.enter.is_some(), l.exit.is_some()))
                .collect()
        };
        let split = cut(vec![keep(0, 100)])
            .set_layout(AT, range(10, 50), Layout::Split)
            .unwrap();
        let both = split
            .set_switch(AT, Micros(10), dissolve.clone())
            .and_then(|c| c.set_switch(AT, Micros(50), dissolve.clone()))
            .unwrap();
        assert_eq!(switches(&both), [(true, true)]);
        assert_eq!(
            both.set_switch(AT, Micros(30), None),
            Err(EditError::NoLayoutEdge(Micros(30)))
        );
        // A change in the middle leaves the outer edges their switches, and joins back.
        let middle = both.set_layout(AT, range(20, 30), Layout::Face).unwrap();
        assert_eq!(
            switches(&middle),
            [(true, false), (false, false), (false, true)]
        );
        let joined = middle.set_layout(AT, range(20, 30), Layout::Split).unwrap();
        assert_eq!(joined, both);
        // Another layout over the same range keeps the switches on its edges.
        let content = both.set_layout(AT, range(10, 50), Layout::Content).unwrap();
        assert_eq!(switches(&content), [(true, true)]);
        let back = both.set_switch(AT, Micros(10), None).unwrap();
        assert_eq!(switches(&back), [(false, true)]);
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

    fn zooms(c: &Cut) -> Vec<(u16, u64, u64)> {
        c.chapters[0].takes[0]
            .zooms
            .iter()
            .map(|z| (z.scale, z.range.from().0, z.range.to().0))
            .collect()
    }

    #[test]
    fn zooms_replace_the_ones_they_overlap_and_stay_ordered() {
        let c = cut(vec![keep(0, 100)])
            .add_zoom(AT, range(40, 60), 120)
            .unwrap()
            .add_zoom(AT, range(10, 20), 130)
            .unwrap();
        assert_eq!(zooms(&c), [(130, 10, 20), (120, 40, 60)]);
        let moved = c.set_range(AT, Placed::Zoom(0), range(15, 45)).unwrap();
        assert_eq!(zooms(&moved), [(130, 15, 45)]);
        let removed = c.remove_placed(AT, Placed::Zoom(1)).unwrap();
        assert_eq!(zooms(&removed), [(130, 10, 20)]);
        assert_eq!(
            c.remove_placed(AT, Placed::Overlay(0)),
            Err(EditError::UnknownPlaced(AT, Placed::Overlay(0)))
        );
    }

    #[test]
    fn a_zoom_takes_a_new_scale() {
        let c = cut(vec![keep(0, 100)])
            .add_zoom(AT, range(10, 20), 130)
            .unwrap();
        let scaled = c.set_zoom_scale(AT, 0, 200).unwrap();
        assert_eq!(zooms(&scaled), [(200, 10, 20)]);
        assert_eq!(zooms(&c.set_zoom_scale(AT, 0, 50).unwrap())[0].0, 100);
        assert_eq!(
            c.set_zoom_scale(AT, 1, 150),
            Err(EditError::UnknownPlaced(AT, Placed::Zoom(1)))
        );
    }

    #[test]
    fn the_music_bed_is_set_with_gains_at_most_0_and_removed() {
        let c = cut(vec![keep(0, 100)]);
        let bed = c.set_music(Some(Music {
            file: "sources/bed.wav".into(),
            gain: 3,
            duck: -12,
        }));
        let music = bed.music.as_ref().unwrap();
        assert_eq!((music.gain, music.duck), (0, -12));
        assert!(
            bed.to_kdl()
                .starts_with("music \"sources/bed.wav\" gain=0 duck=-12\n")
        );
        assert_eq!(bed.set_music(None), c);
    }

    #[test]
    fn transcript_view_marks_removed_words_and_who_removed_them() {
        let silence = RemovedBy::Rule(Target::Kind(ElementKind::Silence));
        let c = cut(vec![
            keep(0, 3_300_000),
            removed(3_300_000, 4_400_000),
            keep(4_400_000, 5_500_000),
        ]);
        let t = transcript();
        let view = c.transcript_view(AT, &t, &lock()).unwrap();
        let states: Vec<_> = view.iter().map(|v| v.state).collect();
        assert_eq!(
            states,
            [
                WordState::Kept,
                WordState::Removed(silence),
                WordState::Kept,
                WordState::Removed(RemovedBy::Manual),
                WordState::Kept,
            ]
        );
        assert_eq!(view[3].word.text, "w3");
    }

    #[test]
    fn removing_a_rule_zoom_turns_it_off_on_its_element() {
        let zoom = Instance {
            applied: Applied::Zoom {
                scale: 112,
                range: range(2_000_000, 3_000_000),
                center: None,
                shape: ZoomShape::default(),
            },
            element: crate::ElementRef {
                target: Target::Kind(ElementKind::Emphasis),
                t: Micros(2_000_000),
            },
            provenance: crate::Provenance {
                layer: Layer::Format,
                rule: None,
                override_at: None,
            },
        };
        let removed = cut(vec![keep(0, 11_000_000)])
            .remove_instance(&id(), &zoom)
            .unwrap();
        assert!(
            removed
                .to_kdl()
                .ends_with("override t01 emphasis at=2_000_000 {\n    zoom off\n}\n")
        );
    }

    #[test]
    fn bad_addresses_and_selections_are_errors() {
        let c = cut(vec![keep(0, 11_000_000)]);
        let missing = TakeAt {
            chapter: 0,
            take: 3,
        };
        assert_eq!(
            c.restore(missing, range(0, 1), &Lock::default()),
            Err(EditError::UnknownTake(missing))
        );
        assert_eq!(
            c.delete_words(
                AT,
                &transcript(),
                range(1_000_100, 1_000_200),
                &Lock::default()
            ),
            Err(EditError::NoWords)
        );
        let other = Transcript::new(Id::new("t02").unwrap());
        assert!(matches!(
            c.delete_words(AT, &other, range(0, 1), &Lock::default()),
            Err(EditError::WrongTranscript { .. })
        ));
    }

    fn s(seconds: f64) -> Micros {
        Micros((seconds * 1_000_000.0) as u64)
    }

    fn secs(from: f64, to: f64) -> TimeRange {
        TimeRange::new(s(from), s(to)).unwrap()
    }

    fn attempt(take: &str, from: f64, to: f64) -> Attempt {
        Attempt {
            take: Id::new(take).unwrap(),
            range: secs(from, to),
        }
    }

    /// Two takes, each with three beats at 1 to 9 s, 11 to 19 s and 21 to 29 s, the first two
    /// beats under "Intro" and the third under "Demo". The second take is proposed.
    fn lineup() -> Lineup {
        let row = |text: &str, from: f64, to: f64| crate::Row {
            text: text.to_owned(),
            attempts: vec![attempt("t01", from, to), attempt("t02", from, to)],
            scores: vec![80, 100],
            proposal: vec![attempt("t02", from, to)],
            look: false,
        };
        Lineup {
            groups: vec![
                crate::Group {
                    title: "Intro".into(),
                    rows: vec![row("one", 1., 9.), row("two", 11., 19.)],
                },
                crate::Group {
                    title: "Demo".into(),
                    rows: vec![row("three", 21., 29.)],
                },
            ],
            scripted: true,
            ..Lineup::default()
        }
    }

    const fn row_at(group: usize, row: usize) -> RowAt {
        RowAt { group, row }
    }

    /// The first take as a rough cut with a person's cut in its second beat, and the second
    /// appended after it as recording leaves it.
    fn recorded() -> Cut {
        Cut::parse(
            "chapter \"Take t01\" {\n    take t01 {\n        keep from=0 to=12_000_000\n        remove from=12_000_000 to=13_000_000 by=manual\n        keep from=13_000_000 to=30_000_000\n    }\n}\n\nchapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=30_000_000\n    }\n}\n",
        )
        .unwrap()
    }

    fn blocks(cut: &Cut) -> Vec<(String, String, u64, u64)> {
        cut.chapters
            .iter()
            .flat_map(|c| c.takes.iter().map(move |t| (c, t)))
            .map(|(c, t)| {
                (
                    c.title.clone(),
                    t.take.to_string(),
                    t.ranges[0].range().from().0 / 1_000_000,
                    t.ranges[t.ranges.len() - 1].range().to().0 / 1_000_000,
                )
            })
            .collect()
    }

    #[test]
    fn compose_plays_the_proposal_and_leaves_a_beat_a_person_cut_in() {
        let new = Id::new("t02").unwrap();
        let composed = recorded().compose(&lineup(), Some(&new)).unwrap();
        // Beat two keeps the first take, where a person cut a second, between two beats of
        // the proposed second take.
        assert_eq!(
            blocks(&composed),
            [
                ("Intro".into(), "t02".into(), 1, 9),
                ("Intro".into(), "t01".into(), 11, 19),
                ("Demo".into(), "t02".into(), 21, 29),
            ]
        );
        assert!(composed.to_kdl().contains("remove from=12_000_000"));
        // Composing what is composed changes nothing.
        assert_eq!(composed.compose(&lineup(), None).unwrap(), composed);
    }

    #[test]
    fn a_pick_plays_instead_of_the_proposal_and_one_per_beat() {
        let lineup = lineup();
        let row = lineup.row(row_at(1, 0)).unwrap();
        let picked = recorded().set_pick(row, &[attempt("t02", 21., 29.)]);
        let picked = picked.set_pick(row, &[attempt("t01", 21., 29.)]);
        assert_eq!(picked.picks.len(), 1);
        assert_eq!(Cut::parse(&picked.to_kdl()).unwrap(), picked);
        let new = Id::new("t02").unwrap();
        let composed = picked.compose(&lineup, Some(&new)).unwrap();
        assert_eq!(blocks(&composed)[2], ("Demo".into(), "t01".into(), 21, 29));
        // A pick binds to its attempt after the attempt moved a little.
        let mut moved = lineup.clone();
        moved.groups[1].rows[0].attempts[0] = attempt("t01", 21.4, 29.2);
        assert_eq!(
            picked.plays(&moved, &moved.groups[1].rows[0], None),
            [attempt("t01", 21.4, 29.2)]
        );
        let cleared = picked.set_pick(row, &[]);
        assert!(cleared.picks.is_empty());
    }

    #[test]
    fn compose_joins_a_takes_neighbouring_beats_and_keeps_a_take_without_words() {
        let mut lineup = lineup();
        for (_, row) in [(0, 0), (0, 1)].iter().zip(&mut lineup.groups[0].rows) {
            row.proposal = vec![row.attempts[0].clone()];
        }
        let mut cut = Cut::parse(
            "chapter \"Take t01\" {\n    take t01 {\n        keep from=0 to=30_000_000\n    }\n}\n\nchapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=30_000_000\n    }\n}\n\nchapter \"B-roll\" {\n    take t03 {\n        keep from=0 to=5_000_000\n    }\n}\n",
        )
        .unwrap();
        cut = cut.compose(&lineup, None).unwrap();
        assert_eq!(
            blocks(&cut),
            [
                ("Intro".into(), "t01".into(), 1, 19),
                ("Demo".into(), "t02".into(), 21, 29),
                ("B-roll".into(), "t03".into(), 0, 5),
            ]
        );
    }

    #[test]
    fn looks_list_unsure_cuts_cut_restarts_and_doubtful_beats_until_answered() {
        let lock = Lock::parse(
            "lock 1\n\ntake t02 {\n    cut from=2_000_000 to=3_000_000 on=retake t=2_000_000 layer=format rule=\"retake\"\n    cut from=4_000_000 to=4_500_000 on=silence t=4_000_000 layer=format rule=\"silence\"\n    review from=6_000_000 to=7_000_000 on=filler t=6_000_000 layer=format rule=\"filler\"\n}\n",
        )
        .unwrap();
        let mut lineup = lineup();
        lineup.groups[1].rows[0].look = true;
        let new = Id::new("t02").unwrap();
        let cut = Cut::parse(
            "chapter \"Take t01\" {\n    take t01 {\n        keep from=0 to=30_000_000\n    }\n}\n\nchapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=30_000_000\n    }\n}\n",
        )
        .unwrap()
        .compose(&lineup, Some(&new))
        .unwrap();
        let looks = cut.looks(&lineup, &lock);
        // The silence is no stumble: nobody needs to hear it.
        assert_eq!(looks.len(), 3, "{looks:?}");
        assert!(
            matches!(looks[0], Look::Unsure { review, .. } if review.kind == ElementKind::Filler)
        );
        assert!(matches!(
            looks[1],
            Look::Restart {
                kind: ElementKind::Retake,
                ..
            }
        ));
        assert_eq!(looks[2], Look::Beat(row_at(1, 0)));
        // A pick answers the beat. The findings are answered by overrides, which the next lock
        // carries as their provenance.
        let row = lineup.row(row_at(1, 0)).unwrap();
        let kept = cut.set_pick(row, &cut.plays(&lineup, row, None));
        assert_eq!(kept.looks(&lineup, &lock).len(), 2);
        let answered = kept.answer(&new, ElementKind::Retake, s(2.), Decision::Cut);
        assert!(
            answered
                .to_kdl()
                .contains("override t02 retake at=2_000_000 {\n    cut\n}")
        );
    }

    #[test]
    fn without_beats_and_nothing_said_twice_the_cut_stays() {
        let mut lineup = lineup();
        lineup.scripted = false;
        for group in &mut lineup.groups {
            for row in &mut group.rows {
                row.attempts.truncate(1);
            }
        }
        assert_eq!(recorded().compose(&lineup, None).unwrap(), recorded());
    }
}
