//! The takes of a video lined up against each other: a row per script beat with every take's
//! attempt at it, for the Takes view and for composing a cut from several takes.
//!
//! An attempt comes from the take's `beat` elements when it was recorded with the prompter:
//! from each beat the prompter showed to the next, trimmed to the words said in it. A take
//! without them is aligned to the script by word overlap: its sentences go in order to the
//! beats whose text shares the most words with them. Without beats in the script, the rows are
//! the sentence groups of the first take, and the other takes align to those.

use std::collections::HashSet;

use crate::{Attr, Cut, ElementKind, Elements, Id, Micros, Script, TimeRange, Transcript, Word};

/// One take's try at a row: a stretch of its media time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Attempt {
    pub take: Id,
    pub range: TimeRange,
}

/// What [`Lineup::new`] reads of one take.
#[derive(Debug, Clone, Copy)]
pub struct TakeWords<'a> {
    pub transcript: &'a Transcript,
    /// The take's elements, whose `beat` elements place its attempts when it has them.
    pub elements: Option<&'a Elements>,
}

/// The rows of the Takes view, grouped under chapters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lineup {
    pub groups: Vec<Group>,
}

/// A chapter of the script, or without beats a chapter of the cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub title: String,
    pub rows: Vec<Row>,
}

/// A beat, or a sentence group of the first take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The beat's bullet, or the sentence group's words.
    pub text: String,
    /// Every take's attempts at it, by take and then time. A take that repeated the beat with
    /// the prompter's Again has more than one.
    pub attempts: Vec<Attempt>,
}

/// A row by its group and its place in the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowAt {
    pub group: usize,
    pub row: usize,
}

/// A new sentence group starts after a pause this long, or once a group has [`GROUP_WORDS`].
const GROUP_GAP: Micros = Micros(1_000_000);
const GROUP_WORDS: usize = 40;

impl Lineup {
    /// Lines up `takes` against the beats of `script`. `cut` names the groups when the script
    /// has no beats.
    pub fn new(script: &Script, cut: &Cut, takes: &[TakeWords<'_>]) -> Self {
        let mut takes: Vec<&TakeWords<'_>> = takes
            .iter()
            .filter(|t| !t.transcript.words().is_empty())
            .collect();
        takes.sort_by_key(|t| take_order(t.transcript.take()));
        let has_beats = script.chapters.iter().any(|c| !c.beats.is_empty());
        let mut lineup = if has_beats {
            Self::of_script(script)
        } else {
            match takes.first() {
                Some(first) => Self::of_take(first.transcript, cut),
                None => return Self::default(),
            }
        };
        let texts: Vec<Vec<String>> = if has_beats {
            script
                .chapters
                .iter()
                .flat_map(|c| &c.beats)
                .map(|b| content_words(&format!("{} {}", b.text, b.script)))
                .collect()
        } else {
            lineup
                .rows()
                .map(|(_, row)| content_words(&row.text))
                .collect()
        };
        let places: Vec<RowAt> = lineup.rows().map(|(at, _)| at).collect();
        for (index, take) in takes.iter().enumerate() {
            let words = take.transcript.words();
            let id = take.transcript.take();
            let found = match beat_attempts(take, places.len()) {
                Some(found) => found,
                None if !has_beats && index == 0 => {
                    // The rows are this take's own sentence groups.
                    groups(words)
                        .into_iter()
                        .enumerate()
                        .map(|(row, words)| (row, span(words)))
                        .collect()
                }
                None => align(words, &texts),
            };
            for (row, range) in found {
                let at = places[row];
                lineup.groups[at.group].rows[at.row].attempts.push(Attempt {
                    take: id.clone(),
                    range,
                });
            }
        }
        lineup
    }

    fn of_script(script: &Script) -> Self {
        Self {
            groups: script
                .chapters
                .iter()
                .filter(|c| !c.beats.is_empty())
                .map(|c| Group {
                    title: c.title.clone(),
                    rows: c
                        .beats
                        .iter()
                        .map(|b| Row {
                            text: b.text.clone(),
                            attempts: Vec::new(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Rows from the sentence groups of `transcript`, under the title of the cut's chapter
    /// that keeps the group's start, or the take's name.
    fn of_take(transcript: &Transcript, cut: &Cut) -> Self {
        let take = transcript.take();
        let mut lineup = Self::default();
        for words in groups(transcript.words()) {
            let start = words[0].range.from();
            let title = cut
                .chapters
                .iter()
                .find(|c| {
                    c.takes.iter().any(|edit| {
                        edit.take == *take
                            && TimeRange::new(start, Micros(start.0 + 1))
                                .is_some_and(|t| edit.keeps(t))
                    })
                })
                .map_or_else(|| format!("Take {take}"), |c| c.title.clone());
            let row = Row {
                text: words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                attempts: Vec::new(),
            };
            match lineup.groups.last_mut() {
                Some(group) if group.title == title => group.rows.push(row),
                _ => lineup.groups.push(Group {
                    title,
                    rows: vec![row],
                }),
            }
        }
        lineup
    }

    /// Every row with its place, in order.
    pub fn rows(&self) -> impl Iterator<Item = (RowAt, &Row)> {
        self.groups.iter().enumerate().flat_map(|(group, g)| {
            g.rows
                .iter()
                .enumerate()
                .map(move |(row, r)| (RowAt { group, row }, r))
        })
    }

    pub fn row(&self, at: RowAt) -> Option<&Row> {
        self.groups.get(at.group)?.rows.get(at.row)
    }

    /// The row that holds `attempt`, by take and the middle of its range.
    pub fn row_of(&self, attempt: &Attempt) -> Option<RowAt> {
        let middle = middle(attempt.range);
        self.rows().find_map(|(at, row)| {
            row.attempts
                .iter()
                .any(|a| a.take == attempt.take && holds(a.range, middle))
                .then_some(at)
        })
    }

    /// The takes with an attempt, in take order.
    pub fn takes(&self) -> Vec<Id> {
        let mut takes: Vec<Id> = Vec::new();
        for (_, row) in self.rows() {
            for attempt in &row.attempts {
                if !takes.contains(&attempt.take) {
                    takes.push(attempt.take.clone());
                }
            }
        }
        takes.sort_by_key(take_order);
        takes
    }
}

/// Shorter ids first, so `t9` comes before `t10`.
pub(crate) fn take_order(take: &Id) -> (usize, String) {
    (take.as_str().len(), take.as_str().to_owned())
}

pub(crate) fn middle(range: TimeRange) -> Micros {
    Micros((range.from().0 + range.to().0) / 2)
}

pub(crate) fn holds(range: TimeRange, t: Micros) -> bool {
    range.from() <= t && t < range.to()
}

/// The words of `words` whose middle lies in `range`.
pub fn words_in(words: &[Word], range: TimeRange) -> &[Word] {
    let start = words.partition_point(|w| middle(w.range) < range.from());
    let end = words.partition_point(|w| middle(w.range) < range.to());
    &words[start..end.max(start)]
}

/// Attempts from the take's `beat` elements: each beat the prompter showed, up to the next,
/// trimmed to its words. `None` when the take has no beat elements.
fn beat_attempts(take: &TakeWords<'_>, rows: usize) -> Option<Vec<(usize, TimeRange)>> {
    let beats: Vec<(Micros, usize)> = take
        .elements?
        .of_kind(ElementKind::Beat)
        .filter_map(|e| {
            let index = usize::try_from(e.integer(Attr::Index)?).ok()?;
            (1..=rows).contains(&index).then_some((e.t, index - 1))
        })
        .collect();
    if beats.is_empty() {
        return None;
    }
    let words = take.transcript.words();
    let end = words.last().map_or(Micros(0), |w| w.range.to());
    let found = beats
        .iter()
        .enumerate()
        .filter_map(|(i, &(t, row))| {
            let until = beats.get(i + 1).map_or(end, |&(next, _)| next);
            let said = words_in(words, TimeRange::new(t, until)?);
            (!said.is_empty()).then(|| (row, span(said)))
        })
        .collect();
    Some(found)
}

fn span(words: &[Word]) -> TimeRange {
    TimeRange::new(words[0].range.from(), words[words.len() - 1].range.to())
        .expect("words are ordered and not empty")
}

/// The words of each sentence: up to a word that ends with `.`, `?` or `!`.
fn sentences(words: &[Word]) -> Vec<&[Word]> {
    words
        .split_inclusive(|w| w.text.trim_end().ends_with(['.', '?', '!']))
        .collect()
}

/// Sentences joined into groups until a pause of [`GROUP_GAP`] or [`GROUP_WORDS`] words.
fn groups(words: &[Word]) -> Vec<&[Word]> {
    let mut out: Vec<&[Word]> = Vec::new();
    let mut start = 0;
    let mut end = 0;
    for sentence in sentences(words) {
        let gap = end > start
            && sentence[0]
                .range
                .from()
                .0
                .saturating_sub(words[end - 1].range.to().0)
                >= GROUP_GAP.0;
        if gap || end - start >= GROUP_WORDS {
            out.push(&words[start..end]);
            start = end;
        }
        end += sentence.len();
    }
    if end > start {
        out.push(&words[start..end]);
    }
    out
}

/// Assigns the take's sentences, in order, to rows in order, so that the words they share with
/// their row add up to the most. A sentence that shares nothing stays with the row before it.
/// Each row the take reached becomes an attempt from its first sentence to its last.
fn align(words: &[Word], rows: &[Vec<String>]) -> Vec<(usize, TimeRange)> {
    let sentences = sentences(words);
    if sentences.is_empty() || rows.is_empty() {
        return Vec::new();
    }
    let rows: Vec<HashSet<&str>> = rows
        .iter()
        .map(|r| r.iter().map(String::as_str).collect())
        .collect();
    let score = |sentence: &[Word], row: &HashSet<&str>| {
        let text: Vec<&str> = sentence.iter().map(|w| w.text.as_str()).collect();
        content_words(&text.join(" "))
            .iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|w| row.contains(w.as_str()))
            .count()
    };
    // best[j][i]: the most shared words of sentences 0..=j with sentence j on row i.
    let n = rows.len();
    let mut best: Vec<Vec<usize>> = Vec::with_capacity(sentences.len());
    for (j, sentence) in sentences.iter().enumerate() {
        let mut line = Vec::with_capacity(n);
        let mut before = 0;
        for (i, row) in rows.iter().enumerate() {
            if j > 0 {
                before = before.max(best[j - 1][i]);
            }
            line.push(score(sentence, row) + before);
        }
        best.push(line);
    }
    // Back from the last sentence, each on the earliest row that scores best.
    let mut rows_of = vec![0; sentences.len()];
    let mut limit = n - 1;
    for j in (0..sentences.len()).rev() {
        let line = &best[j][..=limit];
        let top = *line.iter().max().expect("rows are not empty");
        limit = line
            .iter()
            .position(|&s| s == top)
            .expect("the top is in the line");
        rows_of[j] = limit;
    }
    let mut out: Vec<(usize, TimeRange)> = Vec::new();
    for (sentence, &row) in sentences.iter().zip(&rows_of) {
        let range = span(sentence);
        match out.last_mut() {
            Some((last, span)) if *last == row => {
                *span = TimeRange::new(span.from(), range.to()).expect("sentences are ordered");
            }
            _ => out.push((row, range)),
        }
    }
    out
}

/// Words that tell texts apart: lowercase letters and digits, three or more, without the most
/// common English words and a plural `s`.
fn content_words(text: &str) -> Vec<String> {
    const COMMON: &[&str] = &[
        "the", "and", "you", "that", "this", "with", "for", "are", "was", "but", "not", "have",
        "has", "from", "they", "what", "your", "can", "will", "all", "one", "our", "its", "it's",
        "there", "here", "then", "than", "into", "just", "also", "out", "now", "let", "lets",
        "how", "who", "why", "when", "which", "where", "some", "any", "every", "each",
    ];
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| w.chars().count() >= 3 && !COMMON.contains(&w.as_str()))
        .map(|w| match w.strip_suffix('s') {
            Some(stem) if stem.chars().count() >= 3 && !stem.ends_with('s') => stem.to_owned(),
            _ => w,
        })
        .collect()
}

/// Which words of `a` and of `b` the other does not have in the same order, compared ignoring
/// case and punctuation: the words outside their longest common run.
pub fn word_diff(a: &[&str], b: &[&str]) -> (Vec<bool>, Vec<bool>) {
    let key = |w: &str| -> String {
        w.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let (a_keys, b_keys): (Vec<String>, Vec<String>) = (
        a.iter().map(|w| key(w)).collect(),
        b.iter().map(|w| key(w)).collect(),
    );
    // common[i][j]: the longest common run of a[i..] and b[j..].
    let mut common = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            common[i][j] = if a_keys[i] == b_keys[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let (mut a_diff, mut b_diff) = (vec![true; a.len()], vec![true; b.len()]);
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a_keys[i] == b_keys[j] {
            a_diff[i] = false;
            b_diff[j] = false;
            i += 1;
            j += 1;
        } else if common[i + 1][j] >= common[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (a_diff, b_diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Element;

    fn id(take: &str) -> Id {
        Id::new(take).unwrap()
    }

    /// A transcript of `text` from `start` s, one word per 400 ms, and a 2 s pause after each
    /// sentence.
    fn said(take: &str, start: u64, text: &str) -> Transcript {
        let mut transcript = Transcript::new(id(take));
        let mut t = start * 1_000_000;
        for word in text.split_whitespace() {
            transcript
                .push(Word {
                    text: word.to_owned(),
                    range: TimeRange::new(Micros(t), Micros(t + 300_000)).unwrap(),
                    confidence: 90,
                })
                .unwrap();
            t += if word.ends_with('.') {
                2_000_000
            } else {
                400_000
            };
        }
        transcript
    }

    fn script() -> Script {
        Script::parse(
            "# A\n\n## Setup\n\n- Install the speech model with a checksum\n- Pick a channel folder in git\n\n## Recording\n\n- Press record and talk\n",
        )
        .unwrap()
    }

    fn texts(lineup: &Lineup) -> Vec<Vec<(String, u64)>> {
        lineup
            .rows()
            .map(|(_, row)| {
                row.attempts
                    .iter()
                    .map(|a| (a.take.to_string(), a.range.from().0 / 100_000))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn sentences_align_to_the_beats_they_share_words_with() {
        let first = said(
            "t01",
            0,
            "Hello there. First we install the speech model. It checks the checksum. \
             Then pick a channel folder. It is a git repository. Now press record. And talk.",
        );
        // The second take skips the folder and goes back to nothing.
        let second = said(
            "t02",
            0,
            "We install the model once. Then we press record and just talk.",
        );
        let takes = [
            TakeWords {
                transcript: &second,
                elements: None,
            },
            TakeWords {
                transcript: &first,
                elements: None,
            },
        ];
        let lineup = Lineup::new(&script(), &Cut::default(), &takes);
        assert_eq!(
            lineup
                .groups
                .iter()
                .map(|g| g.title.as_str())
                .collect::<Vec<_>>(),
            ["Setup", "Recording"]
        );
        // "Hello there." shares nothing and stays with the first beat; "It is a git
        // repository." shares "git" with the folder beat.
        assert_eq!(
            texts(&lineup),
            [
                vec![("t01".into(), 0), ("t02".into(), 0)],
                vec![("t01".into(), 96)],
                vec![("t01".into(), 168), ("t02".into(), 36)],
            ]
        );
        assert_eq!(lineup.takes(), [id("t01"), id("t02")]);
        let attempt = &lineup.groups[1].rows[0].attempts[0];
        assert_eq!(lineup.row_of(attempt), Some(RowAt { group: 1, row: 0 }));
    }

    #[test]
    fn beat_elements_place_attempts_and_again_repeats_one() {
        let transcript = said(
            "t01",
            0,
            "Install the model. Install the speech model. Pick a folder. Press record.",
        );
        let mut elements = Elements::new(id("t01"));
        // Beat 1, Again at 2 s, Next to 2 at 5.5 s, Next to 3 at 8.5 s.
        for (t, index) in [(0, 1), (2_000_000, 1), (5_500_000, 2), (8_500_000, 3)] {
            elements
                .push(
                    Element::at(ElementKind::Beat, Micros(t), "script")
                        .with_integer(Attr::Index, index),
                )
                .unwrap();
        }
        let takes = [TakeWords {
            transcript: &transcript,
            elements: Some(&elements),
        }];
        let lineup = Lineup::new(&script(), &Cut::default(), &takes);
        let rows: Vec<Vec<(u64, u64)>> = lineup
            .rows()
            .map(|(_, r)| {
                r.attempts
                    .iter()
                    .map(|a| (a.range.from().0 / 100_000, a.range.to().0 / 100_000))
                    .collect()
            })
            .collect();
        assert_eq!(
            rows,
            [vec![(0, 11), (28, 43)], vec![(60, 71)], vec![(88, 95)]]
        );
    }

    #[test]
    fn without_beats_the_rows_are_the_first_takes_sentence_groups() {
        let script = Script::parse("# A\n").unwrap();
        let first = said("t01", 0, "One two three. Four five six.");
        let second = said("t02", 0, "Four five six.");
        let takes = [
            TakeWords {
                transcript: &first,
                elements: None,
            },
            TakeWords {
                transcript: &second,
                elements: None,
            },
        ];
        let lineup = Lineup::new(&script, &Cut::default(), &takes);
        assert_eq!(lineup.groups.len(), 1);
        assert_eq!(lineup.groups[0].title, "Take t01");
        let rows: Vec<&str> = lineup.rows().map(|(_, r)| r.text.as_str()).collect();
        assert_eq!(rows, ["One two three.", "Four five six."]);
        assert_eq!(
            texts(&lineup),
            [
                vec![("t01".into(), 0)],
                vec![("t01".into(), 28), ("t02".into(), 0)]
            ]
        );
    }

    #[test]
    fn the_diff_marks_words_outside_the_common_run() {
        let (a, b) = word_diff(
            &["We", "ship", "on", "Friday."],
            &["we", "always", "ship", "Friday"],
        );
        assert_eq!(a, [false, false, true, false]);
        assert_eq!(b, [false, true, false, false]);
    }
}
