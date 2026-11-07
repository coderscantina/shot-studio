//! Text read off the screen: phrases the voice says while they show on screen, found in the
//! lines Vision reads. The same reading as the highlight rule of milestone 07, which moves here
//! with the rule becoming a style rule.

use shot_core::{TimeRange, Word};

use crate::vision::{NormRect, TextLine};

/// A phrase read off the screen must be at least this many words.
const MIN_WORDS: usize = 3;

/// A phrase said while it shows on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadOut {
    /// From its first word to its last.
    pub said: TimeRange,
    /// The words as the transcript has them.
    pub text: String,
    /// Where it shows, in fractions of the frame.
    pub rect: NormRect,
}

/// The longest run of at least [`MIN_WORDS`] consecutive words that appears in a line on screen
/// while it is said. `screen` holds the lines seen at sample times, in time order; a word is
/// matched against the latest sample at or before it.
pub fn read_outs(words: &[Word], screen: &[(shot_core::Micros, Vec<TextLine>)]) -> Vec<ReadOut> {
    let keys: Vec<String> = words.iter().map(|w| normalize(&w.text)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let at = words[i].range.from();
        let index = screen.partition_point(|(t, _)| *t <= at);
        let Some((_, lines)) = index.checked_sub(1).map(|i| &screen[i]) else {
            i += 1;
            continue;
        };
        let mut best: Option<(usize, NormRect)> = None;
        for line in lines {
            let text = normalize(&line.text);
            let mut n = 0;
            while i + n < keys.len()
                && !keys[i + n].is_empty()
                && word_offset(&text, &keys[i..=i + n].join(" ")).is_some()
            {
                n += 1;
            }
            if n >= MIN_WORDS && best.is_none_or(|(len, _)| n > len) {
                let phrase = keys[i..i + n].join(" ");
                let start = word_offset(&text, &phrase).expect("matched above");
                best = Some((n, span_in(line, &text, start..start + phrase.len())));
            }
        }
        let Some((n, rect)) = best else {
            i += 1;
            continue;
        };
        if let Some(said) = TimeRange::new(at, words[i + n - 1].range.to()) {
            let text = words[i..i + n]
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            out.push(ReadOut { said, text, rect });
        }
        i += n;
    }
    out
}

/// Lowercase letters and digits, everything else a single space.
fn normalize(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_owned()
}

/// Where `phrase` starts in `text` on word boundaries.
fn word_offset(text: &str, phrase: &str) -> Option<usize> {
    text.match_indices(phrase).map(|(i, _)| i).find(|&i| {
        let before = i == 0 || text.as_bytes()[i - 1] == b' ';
        let end = i + phrase.len();
        let after = end == text.len() || text.as_bytes()[end] == b' ';
        before && after
    })
}

/// The box of a byte range of `normalized`, the normalized text of `line`: the range's first
/// and last letters found back in the line's own text.
fn span_in(line: &TextLine, normalized: &str, range: std::ops::Range<usize>) -> NormRect {
    // The original character index of each normalized letter or digit.
    let letters: Vec<usize> = line
        .text
        .chars()
        .enumerate()
        .filter(|(_, c)| c.is_alphanumeric())
        .flat_map(|(i, c)| std::iter::repeat_n(i, c.to_lowercase().count()))
        .collect();
    let letter_index = |byte: usize| {
        normalized[..byte]
            .chars()
            .filter(|c| c.is_alphanumeric())
            .count()
    };
    let first = letters[letter_index(range.start)];
    let last = letters[letter_index(range.end) - 1];
    let to_bytes = |chars: usize| {
        line.text
            .char_indices()
            .nth(chars)
            .map_or(line.text.len(), |(i, _)| i)
    };
    line.span(to_bytes(first)..to_bytes(last + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::Micros;

    #[test]
    fn a_phrase_read_off_the_screen_is_found_with_its_box_and_two_words_are_not() {
        let words: Vec<Word> = "Open the Project Settings panel now. Then settings."
            .split(' ')
            .enumerate()
            .map(|(i, w)| Word {
                text: w.to_owned(),
                range: TimeRange::new(Micros(1_000_000 + i as u64 * 400_000), {
                    Micros(1_000_000 + i as u64 * 400_000 + 300_000)
                })
                .unwrap(),
                confidence: 90,
            })
            .collect();
        let line = TextLine {
            text: "> Project settings panel".to_owned(),
            rect: NormRect {
                x: 0.1,
                y: 0.5,
                w: 0.24,
                h: 0.02,
            },
        };
        let found = read_outs(&words, &[(Micros(0), vec![line])]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "Project Settings panel");
        assert_eq!(found[0].said.from(), Micros(1_800_000));
        assert!((found[0].rect.x - 0.12).abs() < 1e-6, "{:?}", found[0].rect);
    }
}
