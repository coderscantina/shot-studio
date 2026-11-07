//! The pacing's punch-ins: 112% on every other jump cut and on emphasis.

use shot_core::{Micros, RangeEdit, TakeEdit, TimeRange, Word, Zoom, ZoomBy};

use crate::Speech;

pub const PUNCH_IN: u16 = 112;
/// Kept stretches shorter than this get no punch-in, it would flicker.
const MIN_ZOOM: Micros = Micros(1_000_000);
/// An emphasis zoom holds at most this long.
const MAX_EMPHASIS: Micros = Micros(3_000_000);
/// A word this much louder than the take's median word is emphasis.
pub(crate) const EMPHASIS_DB: f32 = 6.0;
const MIN_EMPHASIS_WORD: Micros = Micros(200_000);
/// At most one emphasis zoom starts within this.
const EMPHASIS_SPACING: Micros = Micros(10_000_000);

/// Replaces the block's `jump-cut` and `emphasis` zooms with ones for the current ranges and
/// leaves every other zoom alone. A new zoom that would overlap another is left out.
pub fn place(edit: &mut TakeEdit, words: &[Word], speech: &Speech) {
    edit.zooms
        .retain(|z| !matches!(z.by, Some(ZoomBy::JumpCut | ZoomBy::Emphasis)));
    let mut wanted = jump_cuts(edit);
    wanted.extend(emphasis(edit, words, speech));
    for zoom in wanted {
        if edit.zooms.iter().any(|z| z.range.overlaps(zoom.range)) {
            continue;
        }
        let at = edit
            .zooms
            .partition_point(|z| z.range.from() < zoom.range.from());
        edit.zooms.insert(at, zoom);
    }
}

fn punch_in(range: TimeRange, by: ZoomBy) -> Zoom {
    Zoom {
        scale: PUNCH_IN,
        range,
        center: None,
        by: Some(by),
    }
}

/// A jump cut is a removal with kept material on both sides. The kept stretch after every
/// second one plays punched in, so the frame alternates across cuts.
fn jump_cuts(edit: &TakeEdit) -> Vec<Zoom> {
    let mut zooms = Vec::new();
    let mut count = 0;
    for window in edit.ranges.windows(3) {
        let [
            RangeEdit::Keep(_),
            RangeEdit::Remove { .. },
            RangeEdit::Keep(after),
        ] = window
        else {
            continue;
        };
        count += 1;
        if count % 2 == 0 && after.duration() >= MIN_ZOOM {
            zooms.push(punch_in(*after, ZoomBy::JumpCut));
        }
    }
    zooms
}

/// Kept emphasis punched in, at most one starting within [`EMPHASIS_SPACING`].
fn emphasis(edit: &TakeEdit, words: &[Word], speech: &Speech) -> Vec<Zoom> {
    let mut zooms = Vec::new();
    let mut next_allowed = Micros(0);
    for loud in loud_words(words, speech) {
        let from = loud.range.from();
        if from < next_allowed || !edit.keeps(loud.word) {
            continue;
        }
        let to = loud.range.to().max(Micros(from.0 + MIN_ZOOM.0));
        if let Some(range) = TimeRange::new(from, to) {
            zooms.push(punch_in(range, ZoomBy::Emphasis));
            next_allowed = Micros(from.0 + EMPHASIS_SPACING.0);
        }
    }
    zooms
}

/// A word said clearly louder than the take's typical word.
pub(crate) struct Loud {
    /// The word itself.
    pub word: TimeRange,
    /// From the word to the end of its clause, at most [`MAX_EMPHASIS`].
    pub range: TimeRange,
    /// How much louder than the median word, in dB.
    pub over: f32,
}

/// Words at least [`EMPHASIS_DB`] louder than the take's median word, in order.
pub(crate) fn loud_words(words: &[Word], speech: &Speech) -> Vec<Loud> {
    let levels: Vec<Option<f32>> = words.iter().map(|w| speech.level(w.range)).collect();
    let mut sorted: Vec<f32> = levels.iter().flatten().copied().collect();
    if sorted.len() < 20 {
        return Vec::new();
    }
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    let mut out = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let Some(level) = levels[i].filter(|l| *l >= median + EMPHASIS_DB) else {
            continue;
        };
        if word.range.duration() < MIN_EMPHASIS_WORD {
            continue;
        }
        let clause_end = words[i..]
            .iter()
            .find(|w| w.text.trim_end().ends_with(['.', '?', '!', ',']))
            .map_or(word.range.to(), |w| w.range.to());
        let from = word.range.from();
        let to = clause_end.min(Micros(from.0 + MAX_EMPHASIS.0));
        if let Some(range) = TimeRange::new(from, to) {
            out.push(Loud {
                word: word.range,
                range,
                over: level - median,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Id, RemovedBy, RuleKind};

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from * 1_000_000), Micros(to * 1_000_000)).unwrap()
    }

    #[test]
    fn every_second_jump_cut_punches_in_and_a_persons_zoom_stays() {
        let cut = |from, to| RangeEdit::Remove {
            range: range(from, to),
            by: RemovedBy::Rule(RuleKind::Silence),
        };
        let mine = Zoom {
            scale: 150,
            range: range(20, 22),
            center: None,
            by: None,
        };
        let mut edit = TakeEdit {
            take: Id::new("t01").unwrap(),
            ranges: vec![
                RangeEdit::Keep(range(0, 5)),
                cut(5, 6),
                RangeEdit::Keep(range(6, 10)),
                cut(10, 11),
                RangeEdit::Keep(range(11, 15)),
                cut(15, 16),
                RangeEdit::Keep(range(16, 20)),
                cut(20, 21),
                RangeEdit::Keep(range(21, 30)),
            ],
            layouts: Vec::new(),
            reviews: Vec::new(),
            zooms: vec![mine, punch_in(range(1, 2), ZoomBy::JumpCut)],
            overlays: Vec::new(),
        };
        place(&mut edit, &[], &Speech::default());
        let zooms: Vec<(TimeRange, Option<ZoomBy>)> =
            edit.zooms.iter().map(|z| (z.range, z.by)).collect();
        assert_eq!(
            zooms,
            [
                (range(11, 15), Some(ZoomBy::JumpCut)),
                (range(20, 22), None),
            ]
        );
    }
}
