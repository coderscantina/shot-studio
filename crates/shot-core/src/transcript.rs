//! A take's word-timed transcript.
//!
//! Derived data, never a project file: analysis produces it into the cache while recording, and
//! editing reads it to turn deleted words into removed ranges in `cut.kdl`.

use crate::{Id, TimeRange};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Micros;

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
