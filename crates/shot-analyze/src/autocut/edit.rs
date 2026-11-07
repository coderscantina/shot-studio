//! Rule edits of one take block: removals and review items written with their rule, keeping the
//! block's invariants (ranges in order and not overlapping, review items inside kept material).

use shot_core::{RangeEdit, RemovedBy, Review, RuleKind, TakeEdit, TimeRange};

/// Removes the kept parts of `range` as `remove ... by=<rule>`. Material already removed keeps
/// its `by`. Review items are cut back to what stays kept.
pub fn remove(edit: &mut TakeEdit, range: TimeRange, rule: RuleKind) {
    let mut out = Vec::with_capacity(edit.ranges.len() + 2);
    for piece in edit.ranges.drain(..) {
        let RangeEdit::Keep(kept) = piece else {
            out.push(piece);
            continue;
        };
        let Some(inside) = kept.intersect(range) else {
            out.push(piece);
            continue;
        };
        out.extend(TimeRange::new(kept.from(), inside.from()).map(RangeEdit::Keep));
        out.push(RangeEdit::Remove {
            range: inside,
            by: RemovedBy::Rule(rule),
        });
        out.extend(TimeRange::new(inside.to(), kept.to()).map(RangeEdit::Keep));
    }
    edit.ranges = merge(out);
    fit_reviews(edit);
}

/// Queues the kept parts of `range` as `review ... by=<rule>`, leaving out parts that overlap
/// another review item. Returns how many items it added.
pub fn review(edit: &mut TakeEdit, range: TimeRange, rule: RuleKind) -> usize {
    let pieces: Vec<TimeRange> = kept_parts(edit, range)
        .filter(|piece| !edit.reviews.iter().any(|r| r.range.overlaps(*piece)))
        .collect();
    for piece in &pieces {
        let at = edit
            .reviews
            .partition_point(|r| r.range.from() < piece.from());
        edit.reviews.insert(
            at,
            Review {
                range: *piece,
                by: rule,
                kept: false,
            },
        );
    }
    pieces.len()
}

/// Undoes a rule's open work: its removals come back as kept material and its open review items
/// go. Its `ignore` items stay, they are a person's decisions.
pub fn strip(edit: &mut TakeEdit, rule: RuleKind) {
    let restored = edit
        .ranges
        .drain(..)
        .map(|piece| match piece {
            RangeEdit::Remove {
                range,
                by: RemovedBy::Rule(by),
            } if by == rule => RangeEdit::Keep(range),
            other => other,
        })
        .collect();
    edit.ranges = merge(restored);
    edit.reviews.retain(|r| r.kept || r.by != rule);
}

/// Turns the removal `range` of `rule` back into kept material, for moving its edges.
pub fn unremove(edit: &mut TakeEdit, range: TimeRange, rule: RuleKind) {
    let restored = edit
        .ranges
        .drain(..)
        .map(|piece| match piece {
            RangeEdit::Remove {
                range: removed,
                by: RemovedBy::Rule(by),
            } if by == rule && removed == range => RangeEdit::Keep(removed),
            other => other,
        })
        .collect();
    edit.ranges = merge(restored);
}

/// Whether `range` overlaps a removal, review item or `ignore` of `rule`: the rule has already
/// had its say about that material.
pub fn decided(edit: &TakeEdit, range: TimeRange, rule: RuleKind) -> bool {
    edit.ranges.iter().any(|piece| {
        matches!(piece, RangeEdit::Remove { range: removed, by: RemovedBy::Rule(by) }
            if *by == rule && removed.overlaps(range))
    }) || edit
        .reviews
        .iter()
        .any(|r| r.by == rule && r.range.overlaps(range))
}

fn kept_parts(edit: &TakeEdit, range: TimeRange) -> impl Iterator<Item = TimeRange> + '_ {
    edit.ranges.iter().filter_map(move |piece| match piece {
        RangeEdit::Keep(kept) => kept.intersect(range),
        RangeEdit::Remove { .. } => None,
    })
}

/// Cuts every review item down to the kept material it lies in, splitting it where a removal
/// now runs through it.
fn fit_reviews(edit: &mut TakeEdit) {
    let reviews = std::mem::take(&mut edit.reviews);
    for review in reviews {
        let parts: Vec<TimeRange> = kept_parts(edit, review.range).collect();
        edit.reviews
            .extend(parts.into_iter().map(|range| Review { range, ..review }));
    }
}

fn merge(pieces: Vec<RangeEdit>) -> Vec<RangeEdit> {
    let mut out: Vec<RangeEdit> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let joined = match (out.last(), piece) {
            (Some(RangeEdit::Keep(a)), RangeEdit::Keep(b)) if a.to() == b.from() => {
                TimeRange::new(a.from(), b.to()).map(RangeEdit::Keep)
            }
            (
                Some(RangeEdit::Remove { range: a, by }),
                RangeEdit::Remove { range: b, by: next },
            ) if a.to() == b.from() && *by == next => {
                TimeRange::new(a.from(), b.to()).map(|range| RangeEdit::Remove { range, by: next })
            }
            _ => None,
        };
        match joined {
            Some(joined) => *out.last_mut().expect("joined with the last piece") = joined,
            None => out.push(piece),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Id, Micros};

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    fn block() -> TakeEdit {
        TakeEdit {
            take: Id::new("t01").unwrap(),
            ranges: vec![
                RangeEdit::Keep(range(0, 10)),
                RangeEdit::Remove {
                    range: range(10, 20),
                    by: RemovedBy::Manual,
                },
                RangeEdit::Keep(range(20, 40)),
            ],
            layouts: Vec::new(),
            reviews: Vec::new(),
            zooms: Vec::new(),
            overlays: Vec::new(),
        }
    }

    #[test]
    fn a_removal_keeps_other_removals_and_splits_reviews_around_it() {
        let mut edit = block();
        assert_eq!(review(&mut edit, range(22, 38), RuleKind::Pause), 1);
        remove(&mut edit, range(5, 30), RuleKind::Silence);
        let silence = |from, to| RangeEdit::Remove {
            range: range(from, to),
            by: RemovedBy::Rule(RuleKind::Silence),
        };
        assert_eq!(
            edit.ranges,
            [
                RangeEdit::Keep(range(0, 5)),
                silence(5, 10),
                RangeEdit::Remove {
                    range: range(10, 20),
                    by: RemovedBy::Manual
                },
                silence(20, 30),
                RangeEdit::Keep(range(30, 40)),
            ]
        );
        assert_eq!(edit.reviews.len(), 1);
        assert_eq!(edit.reviews[0].range, range(30, 38));
        assert!(decided(&edit, range(0, 6), RuleKind::Silence));
        assert!(!decided(&edit, range(0, 6), RuleKind::Filler));
    }

    #[test]
    fn stripping_a_rule_restores_its_removals_and_keeps_ignores() {
        let mut edit = block();
        remove(&mut edit, range(30, 35), RuleKind::Filler);
        review(&mut edit, range(2, 4), RuleKind::Filler);
        edit.reviews.push(Review {
            range: range(22, 24),
            by: RuleKind::Filler,
            kept: true,
        });
        strip(&mut edit, RuleKind::Filler);
        assert_eq!(edit.ranges, block().ranges);
        assert_eq!(edit.reviews.len(), 1);
        assert!(edit.reviews[0].kept);
    }
}
