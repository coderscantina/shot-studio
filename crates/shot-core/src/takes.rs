//! The takes of a video lined up against each other: a row per script beat with every take's
//! attempt at it, each attempt's score and the pieces that play the row when no person chose
//! ([0040]). The Takes view and [`Cut::compose`](crate::Cut::compose) read it.
//!
//! An attempt comes from the take's `beat` elements when it was recorded with the prompter:
//! from each beat the prompter showed to the next, trimmed to the words said in it. A take
//! without them is aligned to the script by word overlap: its sentences go to the beats whose
//! text shares the most words with them, in order, and back to an earlier beat only when a
//! sentence clearly says it again. Without beats in the script, the rows are the sentence groups
//! of the first take; a later take's sentences that say a row again are attempts at it, and the
//! rest are new rows.
//!
//! [0040]: ../../../docs/decisions/0040-the-cut-proposes-itself.md

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use crate::transcript::EDGE_PAD;
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
    /// The take's elements: its `beat` elements place its attempts when it has them, and its
    /// fillers, retakes, misspoken words and pauses lower an attempt's score.
    pub elements: Option<&'a Elements>,
}

/// The rows of the Takes view, grouped under chapters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lineup {
    pub groups: Vec<Group>,
    /// Whether the rows are the script's beats. Otherwise they are what the takes said.
    pub scripted: bool,
    /// Every take's word ranges, for [`Lineup::padded`].
    pub words: BTreeMap<Id, Vec<TimeRange>>,
}

/// A chapter of the script, or without beats a chapter of the cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub title: String,
    pub rows: Vec<Row>,
}

/// A beat, or a sentence group of a take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The beat's bullet, or the sentence group's words.
    pub text: String,
    /// Every take's attempts at it, by take and then time, so a later one is newer. A take that
    /// went back to the beat has more than one.
    pub attempts: Vec<Attempt>,
    /// Each attempt's score from 0 to 100, in the order of `attempts`, see [`score`].
    pub scores: Vec<u8>,
    /// What plays the row when no person chose, in order: the best scored attempt, the newest
    /// of equals, with each of its sentences that another attempt says cleaner, or as clean
    /// and later, taken from that attempt. Padded into the gaps, see [`Lineup::padded`].
    pub proposal: Vec<Attempt>,
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
/// Going back to an earlier row must win this many shared words over going on.
const BACK: usize = 2;
/// A piece reaches at most this far into the gap before and after its words.
const JOIN_PAD: Micros = Micros(300_000);
/// Sentences this alike ([`alike`]) say the same thing.
const SAME_SENTENCE: f64 = 0.5;

impl Lineup {
    /// Lines up `takes` against the beats of `script`. `cut` names the groups when the script
    /// has no beats.
    pub fn new(script: &Script, cut: &Cut, takes: &[TakeWords<'_>]) -> Self {
        let mut takes: Vec<&TakeWords<'_>> = takes
            .iter()
            .filter(|t| !t.transcript.words().is_empty())
            .collect();
        takes.sort_by_key(|t| take_order(t.transcript.take()));
        let scripted = script.chapters.iter().any(|c| !c.beats.is_empty());
        let mut lineup = Self {
            groups: Vec::new(),
            scripted,
            words: takes
                .iter()
                .map(|t| {
                    let words = t.transcript.words().iter().map(|w| w.range).collect();
                    (t.transcript.take().clone(), words)
                })
                .collect(),
        };
        if scripted {
            lineup.groups = Self::of_script(script);
            let texts: Vec<Vec<String>> = script
                .chapters
                .iter()
                .flat_map(|c| &c.beats)
                .map(|b| content_words(&format!("{} {}", b.text, b.script)))
                .collect();
            let places: Vec<RowAt> = lineup.rows().map(|(at, _)| at).collect();
            for take in &takes {
                let found = beat_attempts(take, places.len()).unwrap_or_else(|| {
                    let taken = |row: usize| {
                        lineup
                            .row(places[row])
                            .is_some_and(|r| !r.attempts.is_empty())
                    };
                    align(take.transcript.words(), &texts, taken)
                });
                for (row, range) in found {
                    lineup.push(places[row], take.transcript.take(), range);
                }
            }
        } else {
            for take in &takes {
                lineup.add_unscripted(take.transcript, cut);
            }
        }
        lineup.propose(&takes);
        lineup
    }

    fn of_script(script: &Script) -> Vec<Group> {
        script
            .chapters
            .iter()
            .filter(|c| !c.beats.is_empty())
            .map(|c| Group {
                title: c.title.clone(),
                rows: c.beats.iter().map(|b| Row::new(b.text.clone())).collect(),
            })
            .collect()
    }

    fn push(&mut self, at: RowAt, take: &Id, range: TimeRange) {
        self.groups[at.group].rows[at.row].attempts.push(Attempt {
            take: take.clone(),
            range,
        });
    }

    /// A take of a video without beats: each sentence that says a row again is an attempt at
    /// it, and the rest are new rows, sentence groups under the title of the cut's chapter that
    /// keeps the group's start, or the take's name.
    fn add_unscripted(&mut self, transcript: &Transcript, cut: &Cut) {
        let take = transcript.take();
        let words = transcript.words();
        let texts: Vec<HashSet<String>> = self
            .rows()
            .map(|(_, row)| content_words(&row.text).into_iter().collect())
            .collect();
        let places: Vec<RowAt> = self.rows().map(|(at, _)| at).collect();
        let sentences = sentences(words);
        let mut said_again: Vec<Option<usize>> = sentences
            .iter()
            .map(|s| said_again(&words[s.clone()], &texts))
            .collect();
        // A sentence between two that say one row again belongs to that attempt.
        for i in 1..said_again.len().saturating_sub(1) {
            if said_again[i].is_none() && said_again[i - 1] == said_again[i + 1] {
                said_again[i] = said_again[i - 1];
            }
        }
        let mut i = 0;
        while i < sentences.len() {
            let row = said_again[i];
            let run = said_again[i..].iter().take_while(|r| **r == row).count();
            let stretch = &words[sentences[i].start..sentences[i + run - 1].end];
            match row {
                Some(row) => self.push(places[row], take, span(stretch)),
                None => {
                    for group in groups(stretch) {
                        let at = self.new_row(group, take, cut);
                        self.push(at, take, span(group));
                    }
                }
            }
            i += run;
        }
    }

    fn new_row(&mut self, words: &[Word], take: &Id, cut: &Cut) -> RowAt {
        let start = words[0].range.from();
        let title = cut
            .chapters
            .iter()
            .find(|c| {
                c.takes.iter().any(|edit| {
                    edit.take == *take
                        && TimeRange::new(start, Micros(start.0 + 1)).is_some_and(|t| edit.keeps(t))
                })
            })
            .map_or_else(|| format!("Take {take}"), |c| c.title.clone());
        let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        let row = Row::new(text.join(" "));
        match self.groups.last_mut() {
            Some(group) if group.title == title => group.rows.push(row),
            _ => self.groups.push(Group {
                title,
                rows: vec![row],
            }),
        }
        let group = self.groups.len() - 1;
        RowAt {
            group,
            row: self.groups[group].rows.len() - 1,
        }
    }

    /// Scores every attempt and writes each row's proposal.
    fn propose(&mut self, takes: &[&TakeWords<'_>]) {
        let facts: BTreeMap<&Id, Facts<'_>> = takes
            .iter()
            .map(|t| (t.transcript.take(), Facts::of(t)))
            .collect();
        let mut proposals = Vec::new();
        for (at, row) in self.rows() {
            let beat: HashSet<String> = content_words(&row.text).into_iter().collect();
            let scores: Vec<u8> = row
                .attempts
                .iter()
                .map(|a| facts.get(&a.take).map_or(0, |f| f.score(a.range, &beat)))
                .collect();
            let pieces = proposal(&row.attempts, &scores, &facts);
            let padded = self.joined(pieces);
            proposals.push((at, scores, padded));
        }
        for (at, scores, proposal) in proposals {
            let row = &mut self.groups[at.group].rows[at.row];
            row.scores = scores;
            row.proposal = proposal;
        }
    }

    /// `pieces` padded, with neighbours of one take joined where no word lies between them.
    fn joined(&self, pieces: Vec<Attempt>) -> Vec<Attempt> {
        let mut out: Vec<Attempt> = Vec::new();
        for piece in pieces {
            let piece = self.padded(&piece);
            match out.last_mut() {
                Some(last)
                    if last.take == piece.take
                        && last.range.to() <= piece.range.from()
                        && !self.speaks(&piece.take, last.range.to(), piece.range.from()) =>
                {
                    last.range = TimeRange::new(last.range.from(), piece.range.to())
                        .expect("the later piece ends after the earlier starts");
                }
                _ => out.push(piece),
            }
        }
        out
    }

    /// Whether a word of `take` starts in `[from, to)`.
    fn speaks(&self, take: &Id, from: Micros, to: Micros) -> bool {
        self.words.get(take).is_some_and(|words| {
            let i = words.partition_point(|w| w.from() < from);
            words.get(i).is_some_and(|w| w.from() < to)
        })
    }

    /// `piece` reaching into the gaps around its words, so a join between takes keeps a short
    /// pause and never clips a word's edge: half the gap to the take's word before and after,
    /// at most [`JOIN_PAD`], and [`EDGE_PAD`] where the take has no word there.
    pub fn padded(&self, piece: &Attempt) -> Attempt {
        let Some(words) = self.words.get(&piece.take) else {
            return piece.clone();
        };
        let range = piece.range;
        let first = words.partition_point(|w| middle(*w) < range.from());
        let last = words.partition_point(|w| middle(*w) < range.to());
        let pad = |gap: u64| JOIN_PAD.0.min(gap / 2);
        let from = match first.checked_sub(1).map(|i| words[i].to()) {
            Some(before) => range.from().0 - pad(range.from().0.saturating_sub(before.0)),
            None => range.from().0.saturating_sub(EDGE_PAD.0),
        };
        let to = match words.get(last).map(|w| w.from()) {
            Some(after) => range.to().0 + pad(after.0.saturating_sub(range.to().0)),
            None => range.to().0 + EDGE_PAD.0,
        };
        Attempt {
            take: piece.take.clone(),
            range: TimeRange::new(Micros(from), Micros(to)).expect("padding only widens"),
        }
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

    /// Whether some row has more than one attempt, so there is something to choose.
    pub fn contested(&self) -> bool {
        self.rows().any(|(_, row)| row.attempts.len() > 1)
    }
}

impl Row {
    fn new(text: String) -> Self {
        Self {
            text,
            attempts: Vec::new(),
            scores: Vec::new(),
            proposal: Vec::new(),
        }
    }
}

/// What scoring reads of one take.
struct Facts<'a> {
    words: &'a [Word],
    /// Fillers, retakes and misspoken words.
    stumbles: Vec<TimeRange>,
    /// Retakes and misspoken words alone: what makes a sentence a flub.
    flubs: Vec<TimeRange>,
    /// Pauses mid-sentence.
    pauses: Vec<TimeRange>,
}

impl<'a> Facts<'a> {
    fn of(take: &TakeWords<'a>) -> Self {
        let of = |kinds: &[ElementKind]| -> Vec<TimeRange> {
            take.elements
                .into_iter()
                .flat_map(Elements::iter)
                .filter(|e| kinds.contains(&e.kind))
                .map(|e| e.range())
                .collect()
        };
        Self {
            words: take.transcript.words(),
            stumbles: of(&[
                ElementKind::Filler,
                ElementKind::Retake,
                ElementKind::Misspoken,
            ]),
            flubs: of(&[ElementKind::Retake, ElementKind::Misspoken]),
            pauses: of(&[ElementKind::Pause]),
        }
    }

    /// How many of `words` lie in one of `spans`.
    fn hit(words: &[Word], spans: &[TimeRange]) -> usize {
        words
            .iter()
            .filter(|w| spans.iter().any(|s| holds(*s, middle(w.range))))
            .count()
    }

    /// The share of `words` that is no stumble, a pause counting as one word.
    fn clean(&self, words: &[Word], stumbles: &[TimeRange]) -> f64 {
        let Some(range) = words.first().zip(words.last()) else {
            return 0.0;
        };
        let pauses = self
            .pauses
            .iter()
            .filter(|p| range.0.range.from() <= p.from() && p.to() <= range.1.range.to())
            .count();
        let stumbled = Self::hit(words, stumbles) + pauses;
        1.0 - (stumbled as f64 / words.len() as f64).min(1.0)
    }

    /// An attempt's score from 0 to 100: half for how few of its words are stumbles, 35 for
    /// how much of the beat's `beat` words it says, 15 for ending on a sentence end.
    fn score(&self, range: TimeRange, beat: &HashSet<String>) -> u8 {
        let words = words_in(self.words, range);
        let Some(last) = words.last() else {
            return 0;
        };
        let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        let said: HashSet<String> = content_words(&text.join(" ")).into_iter().collect();
        let close = if beat.is_empty() {
            1.0
        } else {
            beat.intersection(&said).count() as f64 / beat.len() as f64
        };
        let complete = if ends_sentence(last) { 1.0 } else { 0.0 };
        let score = 50.0 * self.clean(words, &self.stumbles) + 35.0 * close + 15.0 * complete;
        score.round() as u8
    }

    /// The sentences of an attempt that are no flub: less than half of each is a retake or a
    /// misspoken word. Each with its content words and how clean it is, 0 to 100.
    fn sentences(&self, range: TimeRange) -> Vec<Said> {
        let words = words_in(self.words, range);
        sentences(words)
            .into_iter()
            .filter_map(|s| {
                let words = &words[s];
                let flubbed = Self::hit(words, &self.flubs);
                if 2 * flubbed >= words.len() {
                    return None;
                }
                let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
                Some(Said {
                    range: span(words),
                    content: content_words(&text.join(" ")).into_iter().collect(),
                    clean: (100.0 * self.clean(words, &self.flubs)).round() as u8,
                })
            })
            .collect()
    }
}

/// A sentence of an attempt.
struct Said {
    range: TimeRange,
    content: HashSet<String>,
    clean: u8,
}

/// The pieces that play a row, see [`Row::proposal`]: the best scored attempt, the newest of
/// equals, and for each of its sentences the cleanest saying of it among the attempts, the
/// newest of equals. Unpadded.
fn proposal(attempts: &[Attempt], scores: &[u8], facts: &BTreeMap<&Id, Facts<'_>>) -> Vec<Attempt> {
    let Some(base) = (0..attempts.len()).max_by_key(|&i| (scores[i], i)) else {
        return Vec::new();
    };
    let said = |i: usize| {
        facts
            .get(&attempts[i].take)
            .map_or_else(Vec::new, |f| f.sentences(attempts[i].range))
    };
    let slots = said(base);
    // Per slot of the base: the attempt that wins it and its sentence there.
    let mut winners: Vec<(u8, usize, TimeRange)> =
        slots.iter().map(|s| (s.clean, base, s.range)).collect();
    for other in (0..attempts.len()).filter(|&i| i != base) {
        for sentence in said(other) {
            let slot = slots
                .iter()
                .enumerate()
                .map(|(i, slot)| (alike(&slot.content, &sentence.content), i))
                .filter(|(alike, _)| *alike >= SAME_SENTENCE)
                .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
            if let Some((_, slot)) = slot {
                let (clean, by, _) = winners[slot];
                // A later sentence of the same attempt is its newer try at the slot.
                if (sentence.clean, other) >= (clean, by) || by == other {
                    winners[slot] = (sentence.clean, other, sentence.range);
                }
            }
        }
    }
    // The base whole, cut open at each sentence another attempt wins.
    let of_base = |from: Micros, to: Micros| {
        let words = facts.get(&attempts[base].take)?.words;
        let rest = words_in(words, TimeRange::new(from, to)?);
        (!rest.is_empty()).then(|| Attempt {
            take: attempts[base].take.clone(),
            range: span(rest),
        })
    };
    if winners.iter().all(|&(_, by, _)| by == base) {
        return vec![attempts[base].clone()];
    }
    let mut pieces = Vec::new();
    let mut cursor = attempts[base].range.from();
    for (slot, &(_, by, range)) in slots.iter().zip(&winners) {
        if by == base {
            continue;
        }
        pieces.extend(of_base(cursor, slot.range.from()));
        pieces.push(Attempt {
            take: attempts[by].take.clone(),
            range,
        });
        cursor = slot.range.to();
    }
    pieces.extend(of_base(cursor, attempts[base].range.to()));
    pieces
}

/// How alike two sets of content words are, 0 to 1: the share of all their words both have.
fn alike(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let all = a.union(b).count();
    if all == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / all as f64
}

/// The row a sentence of `words` says again: the one it shares the most content words with, at
/// least three and half of its own.
fn said_again(words: &[Word], rows: &[HashSet<String>]) -> Option<usize> {
    let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    let said: HashSet<String> = content_words(&text.join(" ")).into_iter().collect();
    rows.iter()
        .enumerate()
        .map(|(i, row)| (said.intersection(row).count(), i))
        .filter(|(shared, _)| *shared >= 3 && 2 * shared >= said.len())
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, i)| i)
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

fn ends_sentence(word: &Word) -> bool {
    word.text.trim_end().ends_with(['.', '?', '!'])
}

/// The words of each sentence, as indexes: up to a word that ends with `.`, `?` or `!`.
fn sentences(words: &[Word]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, word) in words.iter().enumerate() {
        if ends_sentence(word) || i + 1 == words.len() {
            out.push(start..i + 1);
            start = i + 1;
        }
    }
    out
}

/// Sentences joined into groups until a pause of [`GROUP_GAP`] or [`GROUP_WORDS`] words.
fn groups(words: &[Word]) -> Vec<&[Word]> {
    let mut out: Vec<&[Word]> = Vec::new();
    let mut start = 0;
    let mut end = 0;
    for sentence in sentences(words) {
        let gap = end > start
            && words[sentence.start]
                .range
                .from()
                .0
                .saturating_sub(words[end - 1].range.to().0)
                >= GROUP_GAP.0;
        if gap || end - start >= GROUP_WORDS {
            out.push(&words[start..end]);
            start = end;
        }
        end = sentence.end;
    }
    if end > start {
        out.push(&words[start..end]);
    }
    out
}

/// Assigns the take's sentences to rows so that the words they share with their row add up to
/// the most. A sentence goes on from the row before it or stays there, and goes back to an
/// earlier row only when that wins [`BACK`] shared words, which starts a second attempt at it.
/// A sentence that shares nothing stays with the row before it. The sentences before the first
/// that shares a word are a lead-in to that row, or belong to no row when `taken` says an
/// earlier take already has the row, so a retake's "okay, once more" is no attempt. Each run
/// of sentences on one row becomes an attempt from its first sentence to its last.
fn align(
    words: &[Word],
    rows: &[Vec<String>],
    taken: impl Fn(usize) -> bool,
) -> Vec<(usize, TimeRange)> {
    let sentences = sentences(words);
    if sentences.is_empty() || rows.is_empty() {
        return Vec::new();
    }
    let rows: Vec<HashSet<&str>> = rows
        .iter()
        .map(|r| r.iter().map(String::as_str).collect())
        .collect();
    let said: Vec<HashSet<String>> = sentences
        .iter()
        .map(|sentence| {
            let text: Vec<&str> = words[sentence.clone()]
                .iter()
                .map(|w| w.text.as_str())
                .collect();
            content_words(&text.join(" ")).into_iter().collect()
        })
        .collect();
    let score =
        |j: usize, row: &HashSet<&str>| said[j].iter().filter(|w| row.contains(w.as_str())).count();
    // best[j][i]: the most shared words of sentences 0..=j with sentence j on row i, a step
    // back paid for. from[j][i]: the row of sentence j - 1 on that path.
    let n = rows.len();
    let mut best: Vec<Vec<i64>> = Vec::with_capacity(sentences.len());
    let mut from: Vec<Vec<usize>> = Vec::with_capacity(sentences.len());
    for j in 0..sentences.len() {
        let mut line = Vec::with_capacity(n);
        let mut came = Vec::with_capacity(n);
        // The best later row to come back from, per row.
        let mut later = vec![(i64::MIN, 0); n + 1];
        if j > 0 {
            for i in (0..n).rev() {
                later[i] = if best[j - 1][i] >= later[i + 1].0 {
                    (best[j - 1][i], i)
                } else {
                    later[i + 1]
                };
            }
        }
        // The earliest row at or before this one that scores best so far.
        let mut before = (0, 0);
        for (i, row) in rows.iter().enumerate() {
            let mut prior = (0, i);
            if j > 0 {
                if best[j - 1][i] > before.0 || i == 0 {
                    before = (best[j - 1][i], i);
                }
                let back = (later[i + 1].0.saturating_sub(BACK as i64), later[i + 1].1);
                prior = if back.0 > before.0 { back } else { before };
            }
            line.push(score(j, row) as i64 + prior.0);
            came.push(prior.1);
        }
        best.push(line);
        from.push(came);
    }
    // Back from the last sentence, on the earliest row that scores best.
    let mut rows_of = vec![0; sentences.len()];
    let last = &best[sentences.len() - 1];
    let top = *last.iter().max().expect("rows are not empty");
    let mut row = last.iter().position(|&s| s == top).expect("the top is in");
    for j in (0..sentences.len()).rev() {
        rows_of[j] = row;
        row = from[j][row];
    }
    let lead = (0..sentences.len())
        .take_while(|&j| score(j, &rows[rows_of[j]]) == 0)
        .count();
    if lead == sentences.len() {
        return Vec::new();
    }
    let lead_in = !taken(rows_of[lead]);
    let mut out: Vec<(usize, TimeRange)> = Vec::new();
    for (j, sentence) in sentences.iter().enumerate() {
        if j < lead && !lead_in {
            continue;
        }
        let row = rows_of[j.max(lead)];
        let range = span(&words[sentence.clone()]);
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

    fn with<'a>(take: &'a Transcript, elements: Option<&'a Elements>) -> TakeWords<'a> {
        TakeWords {
            transcript: take,
            elements,
        }
    }

    /// The proposal of each row as take and tenths of a second.
    fn proposals(lineup: &Lineup) -> Vec<Vec<(String, u64, u64)>> {
        lineup
            .rows()
            .map(|(_, row)| {
                row.proposal
                    .iter()
                    .map(|a| {
                        (
                            a.take.to_string(),
                            a.range.from().0 / 100_000,
                            a.range.to().0 / 100_000,
                        )
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_take_that_goes_back_to_a_beat_has_a_second_attempt_at_it() {
        let take = said(
            "t01",
            0,
            "We install the speech model with a checksum. Then pick a channel folder in git. \
             Again we install the speech model with its checksum. Now press record and talk.",
        );
        let lineup = Lineup::new(&script(), &Cut::default(), &[with(&take, None)]);
        assert_eq!(
            texts(&lineup),
            [
                vec![("t01".into(), 0), ("t01".into(), 92)],
                vec![("t01".into(), 48)],
                vec![("t01".into(), 144)],
            ]
        );
    }

    #[test]
    fn what_a_retake_says_before_its_first_beat_is_no_attempt() {
        let first = said(
            "t01",
            0,
            "Hello there. First we install the speech model. Then pick a channel folder.",
        );
        let second = said(
            "t02",
            0,
            "Okay once more. Then pick a channel folder in git.",
        );
        let lineup = Lineup::new(
            &script(),
            &Cut::default(),
            &[with(&first, None), with(&second, None)],
        );
        // The first take's greeting leads into its first beat; the retake's does not.
        assert_eq!(
            texts(&lineup),
            [
                vec![("t01".into(), 0)],
                vec![("t01".into(), 64), ("t02".into(), 28)],
                vec![],
            ]
        );
    }

    #[test]
    fn the_cleaner_attempt_is_proposed_and_the_newer_of_equals() {
        let first = said("t01", 0, "We install the the speech model with a checksum.");
        let second = said("t02", 0, "We install the speech model with a checksum.");
        let third = said("t03", 0, "We install the speech model with a checksum.");
        let mut stumble = Elements::new(id("t01"));
        stumble
            .push(Element::over(
                ElementKind::Retake,
                TimeRange::new(Micros(800_000), Micros(1_200_000)).unwrap(),
                "whisper",
            ))
            .unwrap();
        let takes = [
            with(&first, Some(&stumble)),
            with(&second, None),
            with(&third, None),
        ];
        let lineup = Lineup::new(&script(), &Cut::default(), &takes);
        let row = &lineup.groups[0].rows[0];
        assert!(row.scores[0] < row.scores[1], "{:?}", row.scores);
        assert_eq!(row.scores[1], 100);
        // The piece reaches the pad into the take's edges.
        assert_eq!(proposals(&lineup)[0], [("t03".into(), 0, 31)]);
        assert_eq!(row.proposal[0].range.to(), Micros(3_100_000 + EDGE_PAD.0));
    }

    #[test]
    fn a_retake_of_one_sentence_replaces_that_sentence_only() {
        let first = said(
            "t01",
            1,
            "We install the speech model. It verifies a checksum on disk. Then we go on.",
        );
        let second = said("t02", 1, "It verifies the checksum on your disk.");
        let script = Script::parse(
            "# A\n\n## Setup\n\n- Install the speech model, it verifies a checksum on disk, go on\n",
        )
        .unwrap();
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&second, None)],
        );
        // The first take until its second sentence, the retake's sentence, the first take on.
        // Each piece reaches 300 ms into the 1.7 s pauses around it.
        assert_eq!(
            proposals(&lineup)[0],
            [
                ("t01".into(), 9, 32),
                ("t02".into(), 9, 37),
                ("t01".into(), 83, 101),
            ]
        );
    }

    #[test]
    fn without_beats_a_later_take_competes_where_it_says_a_row_again() {
        let script = Script::parse("# A\n").unwrap();
        let first = said(
            "t01",
            0,
            "The router keeps routes next to pages. The loader runs on the server.",
        );
        let second = said(
            "t02",
            0,
            "The loader always runs on the server. One more thing about caching layers.",
        );
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&second, None)],
        );
        let rows: Vec<&str> = lineup.rows().map(|(_, r)| r.text.as_str()).collect();
        assert_eq!(
            rows,
            [
                "The router keeps routes next to pages.",
                "The loader runs on the server.",
                "One more thing about caching layers."
            ]
        );
        assert_eq!(
            texts(&lineup),
            [
                vec![("t01".into(), 0)],
                vec![("t01".into(), 44), ("t02".into(), 0)],
                vec![("t02".into(), 44)],
            ]
        );
        assert!(lineup.contested());
        assert_eq!(proposals(&lineup)[1][0].0, "t02");
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
