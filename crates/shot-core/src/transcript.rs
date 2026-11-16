//! A take's word-timed transcript.
//!
//! Derived data, never a project file: analysis produces it into the cache while recording, and
//! editing reads it to turn deleted words into removed ranges in `cut.kdl`.

use crate::{Id, Micros, TimeRange};

/// How far past a word's measured edges a cut keeps out, so a soft onset ("f", "h") or the
/// decay of the last sound stays with its word. Never more than half the gap to a neighbour.
pub const EDGE_PAD: Micros = Micros(80_000);

/// One recognized word in a take's media time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub text: String,
    pub range: TimeRange,
    /// Recognition confidence in percent, 0 to 100.
    pub confidence: u8,
}

/// The words of one take, in time order and not overlapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    take: Id,
    words: Vec<Word>,
}

impl Transcript {
    pub fn new(take: Id) -> Self {
        Self {
            take,
            words: Vec::new(),
        }
    }

    pub fn take(&self) -> &Id {
        &self.take
    }

    pub fn words(&self) -> &[Word] {
        &self.words
    }

    /// Appends a word as streaming recognition produces it. Returns the word back if it starts
    /// before the previous word ends, so the order invariant holds.
    pub fn push(&mut self, word: Word) -> Result<(), Word> {
        if self
            .words
            .last()
            .is_some_and(|last| word.range.from() < last.range.to())
        {
            return Err(word);
        }
        self.words.push(word);
        Ok(())
    }
}

/// Where a cut before `word` lands: [`EDGE_PAD`] before its start, at most halfway into the gap
/// after the word `before` it.
pub fn cut_before(before: Option<TimeRange>, word: TimeRange) -> Micros {
    let pad = before.map_or(EDGE_PAD, |b| half_gap(b.to(), word.from()));
    Micros(word.from().0.saturating_sub(pad.0))
}

/// Where a cut after `word` lands: [`EDGE_PAD`] past its end, at most halfway into the gap to the
/// word `after` it.
pub fn cut_after(word: TimeRange, after: Option<TimeRange>) -> Micros {
    let pad = after.map_or(EDGE_PAD, |a| half_gap(word.to(), a.from()));
    Micros(word.to().0 + pad.0)
}

fn half_gap(end: Micros, start: Micros) -> Micros {
    Micros(EDGE_PAD.0.min(start.0.saturating_sub(end.0) / 2))
}

/// The media a cut of the words `first` to `last` removes, given the words around them. A cut
/// keeps one pause, the longer of the two around the words, so the speech closes up as if the
/// words were never said, and its edges sit in the gaps, see [`cut_before`] and [`cut_after`].
pub fn word_cut(
    before: Option<TimeRange>,
    first: TimeRange,
    last: TimeRange,
    after: Option<TimeRange>,
) -> TimeRange {
    let gap = |a: Micros, b: Micros| b.0.saturating_sub(a.0);
    let pause_before = before.map(|b| gap(b.to(), first.from()));
    let pause_after = after.map(|a| gap(last.to(), a.from()));
    let (from, to) = match (before, after) {
        (Some(before), Some(_)) if pause_after > pause_before => {
            (cut_after(before, Some(first)), cut_after(last, after))
        }
        (_, Some(after)) => (cut_before(before, first), cut_before(Some(last), after)),
        (_, None) => (cut_before(before, first), cut_after(last, None)),
    };
    TimeRange::new(from, to).expect("a cut holds its words")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    #[test]
    fn a_word_cut_keeps_the_longer_pause_and_pads_into_the_gaps() {
        // Short pause before, long after: the cut takes the pause before.
        assert_eq!(
            word_cut(
                Some(range(0, 1_000_000)),
                range(1_100_000, 1_500_000),
                range(1_600_000, 2_000_000),
                Some(range(3_000_000, 3_400_000)),
            ),
            range(1_050_000, 2_080_000)
        );
        // Long pause before, short after: the next word follows the pause before.
        assert_eq!(
            word_cut(
                Some(range(0, 1_000_000)),
                range(2_000_000, 2_500_000),
                range(2_600_000, 3_000_000),
                Some(range(3_100_000, 3_400_000)),
            ),
            range(1_920_000, 3_050_000)
        );
        // At the end of the take, the cut runs a pad past the last word.
        assert_eq!(
            word_cut(None, range(500_000, 900_000), range(500_000, 900_000), None),
            range(420_000, 980_000)
        );
    }

    fn word(text: &str, from: u64, to: u64) -> Word {
        Word {
            text: text.to_owned(),
            range: TimeRange::new(Micros(from), Micros(to)).unwrap(),
            confidence: 90,
        }
    }

    #[test]
    fn keeps_words_in_order() {
        let mut transcript = Transcript::new(Id::new("t01").unwrap());
        transcript.push(word("ship", 0, 400_000)).unwrap();
        transcript.push(word("it", 400_000, 600_000)).unwrap();
        assert!(transcript.push(word("late", 500_000, 900_000)).is_err());
        assert_eq!(transcript.words().len(), 2);
    }
}
