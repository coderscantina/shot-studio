//! The `highlight` template: a lime box on the screen recording around the text you react to.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `x`, `y`, `width`, `height` | the box in pixels of the screen track, like zoom centers |
//! | `text` | optional, the words the box was matched to, shown in the inspector |
//!
//! The box follows the layout and the zoom, like the cursor, and draws only where the layout
//! shows the screen. [`matches`] finds the phrases you say that are on screen at that moment,
//! from Vision's text lines of the screen track.

use shot_core::{Micros, Overlay, ParamValue, TimeRange, Word};

use super::{ParamKind, ParamSpec, Params, Stage, TemplateSpec, overlay};
use crate::drawing::{Rgba, Shape};
use crate::layout::Rect;

pub const TEMPLATE: &str = "highlight";

pub const SPEC: TemplateSpec = TemplateSpec {
    name: TEMPLATE,
    about: "a box around text on the screen",
    params: &[
        ParamSpec::required("x", ParamKind::Integer, "left edge, screen pixels"),
        ParamSpec::required("y", ParamKind::Integer, "top edge, screen pixels"),
        ParamSpec::required("width", ParamKind::Integer, "screen pixels"),
        ParamSpec::required("height", ParamKind::Integer, "screen pixels"),
        ParamSpec::optional("text", ParamKind::Text, "the words it marks"),
    ],
    animated: true,
};

/// The default `enter` and `exit`.
const DRAW_ON: Micros = Micros(200_000);
/// How long a highlight stays after the last matched word.
pub const AFTER: Micros = Micros(1_500_000);
/// The fewest words a spoken phrase needs to count as reading the screen.
const MIN_WORDS: usize = 3;

/// The box of a `highlight` overlay in screen pixels.
pub fn rect(overlay: &Overlay) -> Option<Rect> {
    let p = Params(overlay);
    let (x, y) = (p.int("x")? as f32, p.int("y")? as f32);
    let (w, h) = (p.int("width")? as f32, p.int("height")? as f32);
    Some(Rect::new(x, y, x + w, y + h))
}

pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    let (Some(box_), Some((layer, view))) = (rect(overlay), stage.screen.as_ref()) else {
        return;
    };
    let presence = Params(overlay).presence(DRAW_ON).at(stage.t);
    if presence <= 0.0 {
        return;
    }
    let pad = 10.0;
    let r = view.map_rect(Rect::new(
        box_.left - pad,
        box_.top - pad,
        box_.right + pad,
        box_.bottom + pad,
    ));
    // Draws on from the center outward.
    let grow = 0.85 + 0.15 * presence;
    let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
    let r = Rect::new(
        cx - r.width() / 2.0 * grow,
        cy - r.height() / 2.0 * grow,
        cx + r.width() / 2.0 * grow,
        cy + r.height() / 2.0 * grow,
    );
    let s = view.scale();
    stage.drawing.over.push(Shape::Clip {
        rect: layer.frame,
        radius: layer.radius,
        shapes: vec![
            Shape::Rect {
                rect: r,
                radius: 10.0 * s,
                color: Rgba::hex(0xC8F25A, 0.16 * presence),
            },
            Shape::Outline {
                rect: r,
                radius: 10.0 * s,
                width: (6.0 * s).max(2.0),
                color: Rgba::hex(0xC8F25A, presence),
            },
        ],
    });
}

/// A line of text on the screen track, in its pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenLine {
    pub text: String,
    pub rect: Rect,
}

impl ScreenLine {
    /// The part of the line that shows the byte range `chars` of its text, assuming evenly
    /// wide characters.
    fn span(&self, chars: std::ops::Range<usize>) -> Rect {
        let count = self.text.chars().count().max(1) as f32;
        let start = self.text[..chars.start].chars().count() as f32;
        let end = self.text[..chars.end].chars().count() as f32;
        let w = self.rect.width();
        Rect::new(
            self.rect.left + w * start / count,
            self.rect.top,
            self.rect.left + w * end / count,
            self.rect.bottom,
        )
    }
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

/// Phrases of `words` read off the screen: the longest run of at least three consecutive words
/// that appears in a line on screen while it is said. `screen` holds the lines seen at sample
/// times, in time order; a word is matched against the latest sample at or before it.
///
/// Each match becomes a `highlight` from its first word until [`AFTER`] past its last word,
/// skipping matches that overlap an existing highlight.
pub fn matches(
    words: &[Word],
    screen: &[(Micros, Vec<ScreenLine>)],
    existing: &[Overlay],
) -> Vec<Overlay> {
    let keys: Vec<String> = words.iter().map(|w| normalize(&w.text)).collect();
    let mut out: Vec<Overlay> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let at = words[i].range.from();
        let index = screen.partition_point(|(t, _)| *t <= at);
        let Some((_, lines)) = index.checked_sub(1).map(|i| &screen[i]) else {
            i += 1;
            continue;
        };
        let normalized: Vec<String> = lines.iter().map(|l| normalize(&l.text)).collect();
        let mut best: Option<(usize, usize, Rect)> = None;
        for (line, text) in lines.iter().zip(&normalized) {
            let mut n = 0;
            loop {
                let phrase = keys[i..(i + n + 1).min(keys.len())].join(" ");
                if i + n >= keys.len() || keys[i + n].is_empty() || !contains_words(text, &phrase) {
                    break;
                }
                n += 1;
            }
            if n >= MIN_WORDS && best.is_none_or(|(len, _, _)| n > len) {
                let phrase = keys[i..i + n].join(" ");
                let start = word_offset(text, &phrase).expect("matched above");
                best = Some((n, start, span_in(line, text, start..start + phrase.len())));
            }
        }
        match best {
            Some((n, _, rect)) => {
                let last = &words[i + n - 1];
                let range = TimeRange::new(at, Micros(last.range.to().0 + AFTER.0));
                let taken = |r: TimeRange| {
                    existing
                        .iter()
                        .chain(&out)
                        .any(|o| o.template.as_str() == TEMPLATE && o.range.overlaps(r))
                };
                if let Some(range) = range.filter(|r| !taken(*r)) {
                    let text = words[i..i + n]
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    out.push(overlay(
                        TEMPLATE,
                        range,
                        vec![
                            ("x", ParamValue::Integer(rect.left.round() as i64)),
                            ("y", ParamValue::Integer(rect.top.round() as i64)),
                            ("width", ParamValue::Integer(rect.width().round() as i64)),
                            ("height", ParamValue::Integer(rect.height().round() as i64)),
                            ("text", ParamValue::Text(text)),
                        ],
                    ));
                }
                i += n;
            }
            None => i += 1,
        }
    }
    out
}

/// Where `phrase` shows in `lines`, on word boundaries, ignoring case and punctuation.
pub fn find_on_screen(lines: &[ScreenLine], phrase: &str) -> Option<Rect> {
    let phrase = normalize(phrase);
    if phrase.is_empty() {
        return None;
    }
    lines.iter().find_map(|line| {
        let text = normalize(&line.text);
        let start = word_offset(&text, &phrase)?;
        Some(span_in(line, &text, start..start + phrase.len()))
    })
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

fn contains_words(text: &str, phrase: &str) -> bool {
    word_offset(text, phrase).is_some()
}

/// The box of a byte range of `normalized`, the normalized text of `line`: the range's first
/// and last letters found back in the line's own text.
fn span_in(line: &ScreenLine, normalized: &str, range: std::ops::Range<usize>) -> Rect {
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

    fn words(text: &str, start: u64) -> Vec<Word> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, w)| Word {
                text: w.to_owned(),
                range: TimeRange::new(
                    Micros(start + i as u64 * 400_000),
                    Micros(start + i as u64 * 400_000 + 300_000),
                )
                .unwrap(),
                confidence: 90,
            })
            .collect()
    }

    #[test]
    fn a_phrase_read_off_the_screen_gets_a_box_around_it() {
        let said = words(
            "and here it says, never deploy on Fridays. Wow.",
            10_000_000,
        );
        let screen = vec![(
            Micros(9_000_000),
            vec![
                ScreenLine {
                    text: "Title: Friday rules".into(),
                    rect: Rect::new(100.0, 100.0, 900.0, 140.0),
                },
                ScreenLine {
                    text: "Rule 1: Never deploy on Fridays!".into(),
                    rect: Rect::new(100.0, 400.0, 1380.0, 440.0),
                },
            ],
        )];
        let found = matches(&said, &screen, &[]);
        assert_eq!(found.len(), 1);
        let h = &found[0];
        // "never" is the 5th word.
        assert_eq!(h.range.from(), Micros(10_000_000 + 4 * 400_000));
        assert_eq!(Params(h).text("text"), Some("never deploy on Fridays."));
        let r = rect(h).unwrap();
        // "never deploy on fridays" starts at character 8 of 32 and runs to the "!".
        assert!((r.left - 420.0).abs() <= 1.0, "{r:?}");
        assert!((r.right - 1340.0).abs() <= 1.0, "{r:?}");
        assert!(super::super::check(h).is_ok());
        assert!(matches(&said, &screen, &found).is_empty());
    }

    #[test]
    fn two_words_or_text_shown_later_do_not_count() {
        let said = words("never deploy", 10_000_000);
        let screen = vec![(
            Micros(9_000_000),
            vec![ScreenLine {
                text: "never deploy on fridays".into(),
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            }],
        )];
        assert!(matches(&said, &screen, &[]).is_empty());
        let later = words("never deploy on fridays", 1_000_000);
        assert!(matches(&later, &screen, &[]).is_empty());
    }
}
