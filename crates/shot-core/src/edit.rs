//! Edits of `cut.kdl` as pure functions: each returns the new `Cut` and touches only the one
//! take block it is aimed at, so the app, CLI and MCP make the same small diff.

use std::fmt;

use crate::takes::{holds, middle};
use crate::{
    Attempt, Chapter, Cut, Id, Layout, LayoutChange, Lineup, Micros, Music, RangeEdit, Rating,
    RemovedBy, Review, Row, RowAt, TakeEdit, TimeRange, Transcript, Word, Zoom,
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
                reviews: Vec::new(),
                zooms: Vec::new(),
                overlays: Vec::new(),
            }],
        });
        cut
    }

    /// Answers the open review item at `range`. Keep writes it as `ignore`, so the rule leaves
    /// the material alone when it runs again. Cut removes the material with the rule as `by`.
    pub fn decide(
        &self,
        at: TakeAt,
        range: TimeRange,
        decision: Decision,
    ) -> Result<Cut, EditError> {
        let index = self
            .take(at)?
            .reviews
            .iter()
            .position(|r| !r.kept && r.range == range)
            .ok_or(EditError::NotInReview(range))?;
        Ok(self.with_take(at, |edit| match decision {
            Decision::Keep => edit.reviews[index].kept = true,
            Decision::Cut => {
                let review = edit.reviews.remove(index);
                replace_in_ranges(edit, review.range, |piece| RangeEdit::Remove {
                    range: piece.range(),
                    by: RemovedBy::Rule(review.by),
                });
            }
        }))
    }

    /// The open review items of the main cut, in cut order.
    pub fn review_queue(&self) -> impl Iterator<Item = (TakeAt, Review)> + '_ {
        self.chapters.iter().enumerate().flat_map(|(chapter, c)| {
            c.takes.iter().enumerate().flat_map(move |(take, edit)| {
                edit.reviews
                    .iter()
                    .filter(|r| !r.kept)
                    .map(move |r| (TakeAt { chapter, take }, *r))
            })
        })
    }

    /// Removes the media of the words that overlap `selection` as `remove ... by=manual`. The
    /// cut runs from the start of the first word to the end of the last, so no word is half
    /// cut. Material that is already removed keeps its original `by`. Review items the cut
    /// touches are dropped, the person has decided about that material.
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
            edit.reviews.retain(|r| !r.range.overlaps(span));
        }))
    }

    /// Turns every removed part of `range` back into kept media, whoever removed it. A part a
    /// rule removed comes back under an `ignore` of that rule, so the rule does not remove it
    /// again when it runs.
    pub fn restore(&self, at: TakeAt, range: TimeRange) -> Result<Cut, EditError> {
        self.take(at)?;
        Ok(self.with_take(at, |edit| {
            let ignores: Vec<Review> = edit
                .ranges
                .iter()
                .filter_map(|piece| match *piece {
                    RangeEdit::Remove {
                        range: removed,
                        by: RemovedBy::Rule(rule),
                    } => Some(Review {
                        range: removed.intersect(range)?,
                        by: rule,
                        kept: true,
                    }),
                    _ => None,
                })
                .collect();
            replace_in_ranges(edit, range, |piece| RangeEdit::Keep(piece.range()));
            for ignore in ignores {
                let index = edit
                    .reviews
                    .partition_point(|r| r.range.from() < ignore.range.from());
                edit.reviews.insert(index, ignore);
            }
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
                    by: None,
                },
            );
        }))
    }

    /// Moves the zoom or overlay `placed` of the block at `at` to `range`. A zoom becomes a
    /// person's, and zooms it now overlaps are dropped, so zooms never stack.
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
                zoom.by = None;
                edit.zooms.retain(|z| !z.range.overlaps(range));
                let index = edit
                    .zooms
                    .partition_point(|z| z.range.from() < range.from());
                edit.zooms.insert(index, zoom);
            }
            Placed::Overlay(index) => edit.overlays[index].range = range,
        }))
    }

    /// Sets the zoom `index` of the block at `at` to `scale` percent, at least 100. The zoom
    /// becomes a person's.
    pub fn set_zoom_scale(&self, at: TakeAt, index: usize, scale: u16) -> Result<Cut, EditError> {
        self.placed(at, Placed::Zoom(index))?;
        Ok(self.with_take(at, |edit| {
            let zoom = &mut edit.zooms[index];
            zoom.scale = scale.max(100);
            zoom.by = None;
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

/// Composing the main cut from the attempts of several takes, see [`Lineup`].
impl Cut {
    /// The rating of `attempt`: the one of its take whose range's middle lies in the attempt.
    pub fn rating(&self, attempt: &Attempt) -> Option<&Rating> {
        self.ratings
            .iter()
            .find(|r| r.take == attempt.take && holds(attempt.range, middle(r.range)))
    }

    /// Gives `attempt` 1 to 5 `stars`, or with `None` takes its stars away. A circle stays.
    pub fn rate(&self, attempt: &Attempt, stars: Option<u8>) -> Cut {
        let stars = stars.map(|s| s.clamp(1, 5));
        let mut cut = self.clone();
        match cut.rating_index(attempt) {
            Some(index) => {
                let rating = &mut cut.ratings[index];
                rating.stars = stars;
                if !rating.pick {
                    rating.range = attempt.range;
                }
            }
            None => cut.ratings.push(Rating {
                take: attempt.take.clone(),
                range: attempt.range,
                stars,
                pick: false,
            }),
        }
        cut.settle_ratings();
        cut
    }

    /// Circles `chosen`, an attempt of `row` or a part of one, as the row's pick, and takes the
    /// circle from the row's other attempts. `None` takes the row's circle away.
    pub fn set_pick(&self, row: &Row, chosen: Option<&Attempt>) -> Cut {
        let mut cut = self.clone();
        for attempt in &row.attempts {
            if let Some(index) = cut.rating_index(attempt) {
                let rating = &mut cut.ratings[index];
                rating.pick = false;
                rating.range = attempt.range;
            }
        }
        if let Some(chosen) = chosen {
            match cut.rating_index(chosen) {
                Some(index) => {
                    cut.ratings[index].pick = true;
                    cut.ratings[index].range = chosen.range;
                }
                None => cut.ratings.push(Rating {
                    take: chosen.take.clone(),
                    range: chosen.range,
                    stars: None,
                    pick: true,
                }),
            }
        }
        cut.settle_ratings();
        cut
    }

    fn rating_index(&self, attempt: &Attempt) -> Option<usize> {
        self.ratings
            .iter()
            .position(|r| r.take == attempt.take && r.range.overlaps(attempt.range))
    }

    /// Drops ratings that say nothing and puts the rest in canonical order.
    fn settle_ratings(&mut self) {
        self.ratings.retain(|r| r.stars.is_some() || r.pick);
        self.ratings.sort_by(|a, b| {
            let key = |r: &Rating| (r.take.as_str().len(), r.take.to_string(), r.range.from());
            key(a).cmp(&key(b))
        });
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
                RangeEdit::Remove { .. } => None,
            })
            .map(|r| r.duration().0)
            .sum();
        Micros(kept)
    }

    /// Puts `with`, an attempt of the row at `at` or a part of one, into the main cut in place
    /// of whatever holds that row now. The material of every attempt of the row leaves the cut,
    /// kept or removed, with pauses around it that belong to no row, and `with` goes where the
    /// first of it was. When the cut has nothing of
    /// the row, `with` goes after the material of the nearest row before it, else before the
    /// nearest row after it, else into a chapter of its own at the end. The removes, reviews,
    /// layouts, zooms and overlays inside `with` come along from the take's blocks; where the
    /// cut has none of the take, `with` is kept whole.
    pub fn use_attempt(
        &self,
        lineup: &Lineup,
        at: RowAt,
        with: &Attempt,
    ) -> Result<Cut, EditError> {
        let row = lineup.row(at).ok_or(EditError::UnknownRow(at))?;
        let block = self.material(&with.take, with.range);
        let holds_a_row = |piece: &TakeEdit| {
            lineup.rows().flat_map(|(_, r)| &r.attempts).any(|a| {
                a.take == piece.take && piece.ranges.iter().any(|r| r.range().overlaps(a.range))
            })
        };
        let holes: Vec<&Attempt> = row.attempts.iter().collect();
        let mut chapters = Vec::with_capacity(self.chapters.len() + 1);
        let mut placed = None;
        for (c, chapter) in self.chapters.iter().enumerate() {
            let mut takes = Vec::with_capacity(chapter.takes.len() + 2);
            for edit in &chapter.takes {
                let mut mine: Vec<TimeRange> = holes
                    .iter()
                    .filter(|a| a.take == edit.take)
                    .map(|a| a.range)
                    .filter(|hole| edit.ranges.iter().any(|r| r.range().overlaps(*hole)))
                    .collect();
                if mine.is_empty() {
                    takes.push(edit.clone());
                    continue;
                }
                mine.sort_by_key(|r| r.from());
                // A piece left between holes that holds no row's media, only the pauses
                // around them, goes with its neighbours.
                let piece = |from, to| {
                    window(from, to)
                        .and_then(|w| restrict(edit, w))
                        .filter(|piece| holds_a_row(piece))
                };
                let mut cursor = Micros(0);
                for hole in mine {
                    takes.extend(piece(cursor, hole.from()));
                    if placed.is_none() {
                        placed = Some((c, takes.len()));
                        takes.push(block.clone());
                    }
                    cursor = cursor.max(hole.to());
                }
                takes.extend(piece(cursor, Micros(u64::MAX)));
            }
            chapters.push(Chapter {
                title: chapter.title.clone(),
                takes,
            });
        }
        let placed = match placed {
            Some(placed) => placed,
            None => insert_by_neighbours(self, &mut chapters, lineup, at, block),
        };
        join_around(&mut chapters, placed);
        // A chapter whose material all moved goes with it.
        let kept: Vec<Chapter> = chapters
            .into_iter()
            .zip(
                self.chapters
                    .iter()
                    .map(|c| !c.takes.is_empty())
                    .chain([true]),
            )
            .filter(|(c, had)| !c.takes.is_empty() || !had)
            .map(|(c, _)| c)
            .collect();
        Cut {
            chapters: kept,
            ..self.clone()
        }
        .settle_takes()
    }

    /// Builds the main cut from the lineup, one chapter per group under its title. Each row
    /// gets its circled attempt or part, else its best rated attempt, else the attempt the cut
    /// keeps most of now, else the newest take's; of equals the newest. Consecutive rows from
    /// one take join into one block that keeps what lies between them, unless an attempt of
    /// that take lies there. A row without attempts is left out.
    pub fn assemble(&self, lineup: &Lineup) -> Result<Cut, EditError> {
        let everything: Vec<&Attempt> = lineup.rows().flat_map(|(_, r)| &r.attempts).collect();
        let mut chapters = Vec::new();
        for group in &lineup.groups {
            let mut spans: Vec<Attempt> = Vec::new();
            for row in &group.rows {
                let Some(chosen) = self.choice(row) else {
                    continue;
                };
                if let Some(last) = spans.last_mut()
                    && last.take == chosen.take
                    && last.range.to() <= chosen.range.from()
                    && window(last.range.to(), chosen.range.from()).is_none_or(|gap| {
                        !everything
                            .iter()
                            .any(|a| a.take == chosen.take && a.range.overlaps(gap))
                    })
                {
                    last.range = TimeRange::new(last.range.from(), chosen.range.to())
                        .expect("the later attempt ends after the earlier starts");
                    continue;
                }
                spans.push(chosen);
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
        Cut {
            chapters,
            ..self.clone()
        }
        .settle_takes()
    }

    /// The attempt "Assemble picks" uses for `row`, see [`Cut::assemble`].
    pub fn choice(&self, row: &Row) -> Option<Attempt> {
        let picked = row.attempts.iter().find_map(|a| {
            let rating = self.rating(a).filter(|r| r.pick)?;
            Some(Attempt {
                take: a.take.clone(),
                range: rating.range,
            })
        });
        let rated = || {
            row.attempts
                .iter()
                .filter_map(|a| Some((self.rating(a)?.stars?, a)))
                .max_by_key(|(stars, _)| *stars)
                .map(|(_, a)| a.clone())
        };
        let held = || {
            row.attempts
                .iter()
                .map(|a| (self.kept_of(&a.take, a.range), a))
                .filter(|(kept, _)| kept.0 > 0)
                .max_by_key(|(kept, _)| *kept)
                .map(|(_, a)| a.clone())
        };
        picked
            .or_else(rated)
            .or_else(held)
            .or_else(|| row.attempts.last().cloned())
    }

    /// A block of `take` over `range` with what the cut's blocks of that take have there: their
    /// keeps and removes, reviews, layouts, zooms and overlays. Where no block covers the take's
    /// media, the block keeps it.
    pub fn material(&self, take: &Id, range: TimeRange) -> TakeEdit {
        let mut out = TakeEdit {
            take: take.clone(),
            ranges: Vec::new(),
            layouts: Vec::new(),
            reviews: Vec::new(),
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
            for r in piece.reviews {
                if free(r.range, out.reviews.iter().map(|o| o.range)) {
                    out.reviews.push(r);
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
        out.ranges = merge_adjacent(filled, same_kind);
        out.layouts.sort_by_key(|l| l.range.from());
        out.zooms.sort_by_key(|z| z.range.from());
        out.reviews.sort_by_key(|r| r.range.from());
        let reviews = std::mem::take(&mut out.reviews);
        out.reviews = reviews.into_iter().filter(|r| out.keeps(r.range)).collect();
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

/// What of `edit` lies inside `window`: ranges, layouts, reviews and zooms cut to it, and the
/// overlays that start in it. `None` when no kept or removed media is left.
fn restrict(edit: &TakeEdit, window: TimeRange) -> Option<TakeEdit> {
    let ranges: Vec<RangeEdit> = edit
        .ranges
        .iter()
        .filter_map(|piece| {
            let inside = piece.range().intersect(window)?;
            Some(match *piece {
                RangeEdit::Keep(_) => RangeEdit::Keep(inside),
                RangeEdit::Remove { by, .. } => RangeEdit::Remove { range: inside, by },
            })
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
                Some(LayoutChange {
                    range: l.range.intersect(window)?,
                    ..*l
                })
            })
            .collect(),
        reviews: edit
            .reviews
            .iter()
            .filter_map(|r| {
                Some(Review {
                    range: r.range.intersect(window)?,
                    ..*r
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

/// Puts `block` where the row at `at` belongs when the cut has none of it: right after the
/// block that holds the end of the nearest row before it, split where a later row's media
/// starts in that block; else right before the start of the nearest row after it; else in a
/// chapter of its own at the end. Returns where it went.
fn insert_by_neighbours(
    cut: &Cut,
    chapters: &mut Vec<Chapter>,
    lineup: &Lineup,
    at: RowAt,
    block: TakeEdit,
) -> (usize, usize) {
    let rows: Vec<&Row> = lineup.rows().map(|(_, row)| row).collect();
    let index = lineup.rows().position(|(r, _)| r == at).unwrap_or(0);
    // Where the media of `attempts` starts in each block that has some, in cut order.
    let starts = |attempts: &[Attempt]| {
        let mut found = Vec::new();
        for (c, chapter) in cut.chapters.iter().enumerate() {
            for (b, edit) in chapter.takes.iter().enumerate() {
                for attempt in attempts.iter().filter(|a| a.take == edit.take) {
                    if let Some(media) = edit
                        .ranges
                        .iter()
                        .find_map(|r| r.range().intersect(attempt.range))
                    {
                        found.push((c, b, media.from()));
                    }
                }
            }
        }
        found
    };
    let split_at = |chapters: &mut Vec<Chapter>, c: usize, b: usize, t: Micros, block| {
        let edit = chapters[c].takes.remove(b);
        let before = window(Micros(0), t).and_then(|w| restrict(&edit, w));
        let after = window(t, Micros(u64::MAX)).and_then(|w| restrict(&edit, w));
        let takes = &mut chapters[c].takes;
        let mut at = b;
        if let Some(before) = before {
            takes.insert(at, before);
            at += 1;
        }
        takes.insert(at, block);
        if let Some(after) = after {
            takes.insert(at + 1, after);
        }
        (c, at)
    };
    let before = rows[..index]
        .iter()
        .rev()
        .find_map(|row| starts(&row.attempts).last().copied());
    if let Some((c, b, t)) = before {
        let later = rows[index + 1..]
            .iter()
            .flat_map(|row| starts(&row.attempts))
            .filter(|&(lc, lb, lt)| (lc, lb) == (c, b) && lt > t)
            .map(|(_, _, lt)| lt)
            .min();
        return match later {
            Some(t) => split_at(chapters, c, b, t, block),
            None => {
                chapters[c].takes.insert(b + 1, block);
                (c, b + 1)
            }
        };
    }
    let after = rows[index + 1..]
        .iter()
        .find_map(|row| starts(&row.attempts).first().copied());
    match after {
        Some((c, b, t)) => split_at(chapters, c, b, t, block),
        None => {
            chapters.push(Chapter {
                title: lineup.groups[at.group].title.clone(),
                takes: vec![block],
            });
            (chapters.len() - 1, 0)
        }
    }
}

/// Joins the block at `placed` with its neighbours in the chapter where they are the same
/// take and meet without a gap, so putting back what was there leaves the block as it was.
fn join_around(chapters: &mut [Chapter], (c, b): (usize, usize)) {
    let takes = &mut chapters[c].takes;
    let mut b = b;
    if b > 0 && meets(&takes[b - 1], &takes[b]) {
        let next = takes.remove(b);
        b -= 1;
        join(&mut takes[b], next);
    }
    if b + 1 < takes.len() && meets(&takes[b], &takes[b + 1]) {
        let next = takes.remove(b + 1);
        join(&mut takes[b], next);
    }
}

fn meets(a: &TakeEdit, b: &TakeEdit) -> bool {
    a.take == b.take
        && match (a.ranges.last(), b.ranges.first()) {
            (Some(end), Some(start)) => end.range().to() == start.range().from(),
            _ => false,
        }
}

/// Appends `next`, which starts where `edit` ends, to `edit`, and joins what the seam split.
fn join(edit: &mut TakeEdit, next: TakeEdit) {
    let ranges = std::mem::take(&mut edit.ranges);
    edit.ranges = merge_adjacent(ranges.into_iter().chain(next.ranges).collect(), same_kind);
    edit.layouts.extend(next.layouts);
    merge_layouts(&mut edit.layouts);
    for review in next.reviews {
        match edit.reviews.last_mut() {
            Some(last)
                if last.range.to() == review.range.from()
                    && last.by == review.by
                    && last.kept == review.kept =>
            {
                last.range = TimeRange::new(last.range.from(), review.range.to())
                    .expect("adjacent ranges join into a non-empty range");
            }
            _ => edit.reviews.push(review),
        }
    }
    for zoom in next.zooms {
        match edit.zooms.last_mut() {
            Some(last)
                if last.range.to() == zoom.range.from()
                    && Zoom {
                        range: zoom.range,
                        ..*last
                    } == zoom =>
            {
                last.range = TimeRange::new(last.range.from(), zoom.range.to())
                    .expect("adjacent ranges join into a non-empty range");
            }
            _ => edit.zooms.push(zoom),
        }
    }
    edit.overlays.extend(next.overlays);
}

fn same_kind(a: RangeEdit, b: RangeEdit) -> bool {
    match (a, b) {
        (RangeEdit::Keep(_), RangeEdit::Keep(_)) => true,
        (RangeEdit::Remove { by: a, .. }, RangeEdit::Remove { by: b, .. }) => a == b,
        _ => false,
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
                    reviews: Vec::new(),
                    zooms: Vec::new(),
                    overlays: Vec::new(),
                }],
            }],
            ..Cut::default()
        }
    }

    #[test]
    fn queue_decisions_write_remove_or_ignore() {
        let mut cut = cut(vec![keep(0, 10_000_000)]);
        let item = Review {
            range: range(2_000_000, 3_000_000),
            by: RuleKind::Pause,
            kept: false,
        };
        cut.chapters[0].takes[0].reviews.push(item);
        assert_eq!(cut.review_queue().collect::<Vec<_>>(), [(AT, item)]);

        let kept = cut.decide(AT, item.range, Decision::Keep).unwrap();
        let take = &kept.chapters[0].takes[0];
        assert_eq!(take.reviews, [Review { kept: true, ..item }]);
        assert_eq!(take.ranges, [keep(0, 10_000_000)]);
        assert_eq!(kept.review_queue().count(), 0);
        assert_eq!(
            kept.decide(AT, item.range, Decision::Cut),
            Err(EditError::NotInReview(item.range))
        );

        let cut_it = cut.decide(AT, item.range, Decision::Cut).unwrap();
        let take = &cut_it.chapters[0].takes[0];
        assert!(take.reviews.is_empty());
        assert_eq!(
            take.ranges,
            [
                keep(0, 2_000_000),
                removed(2_000_000, 3_000_000, RemovedBy::Rule(RuleKind::Pause)),
                keep(3_000_000, 10_000_000),
            ]
        );
    }

    #[test]
    fn restoring_a_rule_cut_ignores_the_rule_there() {
        let cut = cut(vec![
            keep(0, 1_000_000),
            removed(1_000_000, 2_000_000, RemovedBy::Rule(RuleKind::Silence)),
            keep(2_000_000, 3_000_000),
        ]);
        let restored = cut.restore(AT, range(500_000, 2_500_000)).unwrap();
        let take = &restored.chapters[0].takes[0];
        assert_eq!(take.ranges, [keep(0, 3_000_000)]);
        assert_eq!(
            take.reviews,
            [Review {
                range: range(1_000_000, 2_000_000),
                by: RuleKind::Silence,
                kept: true,
            }]
        );

        // Cutting the restored material by hand ends the person's say over it.
        let deleted = restored
            .delete_words(AT, &transcript(), range(1_000_000, 1_200_000))
            .unwrap();
        assert!(deleted.chapters[0].takes[0].reviews.is_empty());
    }

    fn ranges(cut: &Cut) -> &[RangeEdit] {
        &cut.chapters[0].takes[0].ranges
    }

    const MANUAL: RemovedBy = RemovedBy::Manual;

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

    fn zooms(c: &Cut) -> Vec<(u16, u64, u64, bool)> {
        c.chapters[0].takes[0]
            .zooms
            .iter()
            .map(|z| (z.scale, z.range.from().0, z.range.to().0, z.by.is_none()))
            .collect()
    }

    #[test]
    fn zooms_replace_the_ones_they_overlap_and_stay_ordered() {
        let c = cut(vec![keep(0, 100)])
            .add_zoom(AT, range(40, 60), 120)
            .unwrap()
            .add_zoom(AT, range(10, 20), 130)
            .unwrap();
        assert_eq!(zooms(&c), [(130, 10, 20, true), (120, 40, 60, true)]);
        let moved = c.set_range(AT, Placed::Zoom(0), range(15, 45)).unwrap();
        assert_eq!(zooms(&moved), [(130, 15, 45, true)]);
        let removed = c.remove_placed(AT, Placed::Zoom(1)).unwrap();
        assert_eq!(zooms(&removed), [(130, 10, 20, true)]);
        assert_eq!(
            c.remove_placed(AT, Placed::Overlay(0)),
            Err(EditError::UnknownPlaced(AT, Placed::Overlay(0)))
        );
    }

    #[test]
    fn a_zoom_takes_a_new_scale_and_becomes_a_persons() {
        let mut c = cut(vec![keep(0, 100)])
            .add_zoom(AT, range(10, 20), 130)
            .unwrap();
        c.chapters[0].takes[0].zooms[0].by = Some(crate::ZoomBy::Emphasis);
        let scaled = c.set_zoom_scale(AT, 0, 200).unwrap();
        assert_eq!(zooms(&scaled), [(200, 10, 20, true)]);
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
    /// beats under "Intro" and the third under "Demo".
    fn lineup() -> Lineup {
        let row = |text: &str, from: f64, to: f64| crate::Row {
            text: text.to_owned(),
            attempts: vec![attempt("t01", from, to), attempt("t02", from, to)],
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
        }
    }

    const fn row_at(group: usize, row: usize) -> RowAt {
        RowAt { group, row }
    }

    /// The first take as a rough cut, and the second appended after it as recording leaves it.
    fn recorded() -> Cut {
        Cut::parse(
            "chapter \"Intro\" {\n    take t01 {\n        keep from=0 to=12_000_000\n        remove from=12_000_000 to=13_000_000 by=silence\n        keep from=13_000_000 to=30_000_000\n        zoom 120 from=15_000_000 to=16_000_000\n    }\n}\n\nchapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=14_000_000\n        remove from=14_000_000 to=15_000_000 by=filler\n        keep from=15_000_000 to=30_000_000\n    }\n}\n",
        )
        .unwrap()
    }

    #[test]
    fn use_this_puts_an_attempt_in_place_of_what_holds_its_beat() {
        let used = recorded()
            .use_attempt(&lineup(), row_at(0, 1), &attempt("t02", 11., 19.))
            .unwrap();
        assert_eq!(
            used.to_kdl(),
            "chapter \"Intro\" {\n    take t01 {\n        keep from=0 to=11_000_000\n    }\n    take t02 {\n        keep from=11_000_000 to=14_000_000\n        remove from=14_000_000 to=15_000_000 by=filler\n        keep from=15_000_000 to=19_000_000\n    }\n    take t01 {\n        keep from=19_000_000 to=30_000_000\n    }\n}\n\nchapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=11_000_000\n    }\n    take t02 {\n        keep from=19_000_000 to=30_000_000\n    }\n}\n"
        );
        // Every beat taken from t02 empties its chapter, which goes.
        let all = (0..3).fold(recorded(), |cut, i| {
            let (at, from) = [(row_at(0, 0), 1.), (row_at(0, 1), 11.), (row_at(1, 0), 21.)][i];
            cut.use_attempt(&lineup(), at, &attempt("t02", from, from + 8.))
                .unwrap()
        });
        assert!(all.chapters.iter().all(|c| c.title == "Intro"));
    }

    #[test]
    fn using_what_holds_a_beat_already_changes_nothing() {
        let cut = Cut::parse(
            "chapter \"Intro\" {\n    take t01 {\n        keep from=0 to=12_000_000\n        remove from=12_000_000 to=13_000_000 by=silence\n        keep from=13_000_000 to=30_000_000\n        review from=18_500_000 to=19_500_000 by=filler\n        zoom 120 from=10_500_000 to=11_500_000\n        overlay lower-third from=3_000_000 to=8_000_000 {\n            text \"Hi\"\n        }\n    }\n}\n",
        )
        .unwrap();
        let used = cut
            .use_attempt(&lineup(), row_at(0, 1), &attempt("t01", 11., 19.))
            .unwrap();
        assert_eq!(used, cut);
    }

    #[test]
    fn a_beat_the_cut_lacks_goes_after_the_beat_before_it() {
        let cut = Cut::parse(
            "chapter \"Intro\" {\n    take t01 {\n        keep from=0 to=10_000_000\n    }\n    take t01 {\n        keep from=20_000_000 to=30_000_000\n    }\n}\n",
        )
        .unwrap();
        let used = cut
            .use_attempt(&lineup(), row_at(0, 1), &attempt("t02", 12., 18.))
            .unwrap();
        let blocks: Vec<(String, u64)> = used.chapters[0]
            .takes
            .iter()
            .map(|b| (b.take.to_string(), b.ranges[0].range().from().0 / 1_000_000))
            .collect();
        assert_eq!(
            blocks,
            [("t01".into(), 0), ("t02".into(), 12), ("t01".into(), 20)]
        );
    }

    #[test]
    fn assemble_takes_the_circle_then_the_stars_then_what_the_cut_holds() {
        let lineup = lineup();
        let rows: Vec<&crate::Row> = lineup.rows().map(|(_, r)| r).collect();
        // Beat one: t01 has more stars. Beat two: a circled part of t02 beats t01's five stars.
        // Beat three: nothing rated, the cut holds both whole, and t02 is the newer.
        let cut = recorded()
            .rate(&attempt("t01", 1., 9.), Some(4))
            .rate(&attempt("t02", 1., 9.), Some(2))
            .rate(&attempt("t01", 11., 19.), Some(5))
            .set_pick(rows[1], Some(&attempt("t02", 12., 17.)));
        assert_eq!(
            cut.choice(rows[1]),
            Some(attempt("t02", 12., 17.)),
            "the circled part"
        );
        let assembled = cut.assemble(&lineup).unwrap();
        assert_eq!(
            assembled.to_kdl().split("chapter").next().unwrap(),
            "takes {\n    attempt t01 from=1_000_000 to=9_000_000 stars=4\n    attempt t01 from=11_000_000 to=19_000_000 stars=5\n    attempt t02 from=1_000_000 to=9_000_000 stars=2\n    attempt t02 from=12_000_000 to=17_000_000 pick=#true\n}\n\n"
        );
        let blocks: Vec<(&str, String, Vec<RangeEdit>)> = assembled
            .chapters
            .iter()
            .flat_map(|c| {
                c.takes
                    .iter()
                    .map(move |b| (c.title.as_str(), b.take.to_string(), b.ranges.clone()))
            })
            .collect();
        assert_eq!(
            blocks,
            [
                ("Intro", "t01".into(), vec![keep(1_000_000, 9_000_000)]),
                (
                    "Intro",
                    "t02".into(),
                    vec![
                        keep(12_000_000, 14_000_000),
                        removed(14_000_000, 15_000_000, RemovedBy::Rule(RuleKind::Filler)),
                        keep(15_000_000, 17_000_000)
                    ]
                ),
                ("Demo", "t02".into(), vec![keep(21_000_000, 29_000_000)]),
            ]
        );
    }

    #[test]
    fn assemble_joins_a_takes_consecutive_beats_and_drops_what_left() {
        let mut cut = recorded();
        cut.overrides.push(crate::Override {
            take: Id::new("t02").unwrap(),
            kind: crate::ElementKind::Filler,
            at: s(14.),
            decision: Some(Decision::Keep),
            effects: Vec::new(),
        });
        for (from, to) in [(1., 9.), (11., 19.), (21., 29.)] {
            cut = cut.rate(&attempt("t01", from, to), Some(3));
        }
        let assembled = cut.assemble(&lineup()).unwrap();
        // t01 for every beat: beats one and two join with the silence between them.
        assert_eq!(
            assembled.chapters[0].takes[0].ranges,
            [
                keep(1_000_000, 12_000_000),
                removed(12_000_000, 13_000_000, RemovedBy::Rule(RuleKind::Silence)),
                keep(13_000_000, 19_000_000)
            ]
        );
        assert_eq!(assembled.chapters[0].takes[0].zooms.len(), 1);
        assert!(assembled.overrides.is_empty(), "t02 left the cut");

        cut.derived.push(crate::DerivedCut {
            kind: crate::DerivedKind::Short,
            name: Id::new("teaser").unwrap(),
            captions: None,
            clips: vec![crate::Clip {
                take: Id::new("t02").unwrap(),
                range: secs(1., 5.),
            }],
        });
        assert_eq!(
            cut.assemble(&lineup()),
            Err(EditError::TakeInDerived {
                derived: Id::new("teaser").unwrap(),
                take: Id::new("t02").unwrap()
            })
        );
    }

    #[test]
    fn stars_and_circles_follow_their_attempt_and_round_trip() {
        let lineup = lineup();
        let row = lineup.row(row_at(0, 0)).unwrap();
        let first = attempt("t01", 1., 9.);
        // The attempt moved a little since it was rated: the rating still binds.
        let rated = Cut::default().rate(&first, Some(9));
        assert_eq!(
            rated.rating(&attempt("t01", 1.5, 9.5)).unwrap().stars,
            Some(5)
        );
        let picked = rated.set_pick(row, Some(&attempt("t02", 1., 9.)));
        let moved = picked.set_pick(row, Some(&first));
        assert_eq!(
            moved.ratings,
            [Rating {
                take: first.take.clone(),
                range: first.range,
                stars: Some(5),
                pick: true,
            }]
        );
        assert_eq!(Cut::parse(&moved.to_kdl()).unwrap(), moved);
        let cleared = moved.set_pick(row, None).rate(&first, None);
        assert!(cleared.ratings.is_empty());
    }
}
