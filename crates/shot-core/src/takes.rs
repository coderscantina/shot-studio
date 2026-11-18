//! The takes of a video lined up against each other: a row per script beat with every take's
//! attempt at it, each attempt's score and the pieces that play the row when no person chose
//! ([0040]). The Takes view and [`Cut::compose`](crate::Cut::compose) read it.
//!
//! An attempt comes from the take's `beat` elements when it was recorded with the prompter:
//! from each beat the prompter showed to the next, trimmed to the words said in it. A take
//! without them is aligned to the script by word overlap: its sentences go to the beats whose
//! text shares the most words with them, in order, and back to an earlier beat only when a
//! sentence clearly says it again. What a retake of a beat says before and after the sentences
//! that share a word with it is no attempt. Without beats in the script, the rows are the
//! sentence groups of the first take; a later take's sentences that say a row again are
//! attempts at it, and the rest are new rows.
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
    /// What plays the row when no person chose, in order: the best scored of the attempts that
    /// say the row whole, the newest of equals, with each of its sentences that another attempt
    /// says cleaner, or as clean and later, taken from that attempt. Padded into the gaps, see
    /// [`Lineup::padded`].
    pub proposal: Vec<Attempt>,
    /// Whether the proposal is worth a person's look: it plays more than one attempt, an
    /// older attempt over a newer one, or an attempt that leads by [`CLOSE`] points or less.
    /// Without beats also a row a later take left out between two rows it says again, and a
    /// row that take added there: it may say the row in other words, and both play until a
    /// person cuts one. What a retake says before, between and after the rows it says again is
    /// worth a look too: it may say rows in other words or be a remark that does not belong.
    pub look: bool,
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
/// A lead of at most this many points is close.
const CLOSE: u8 = 5;
/// Sentences this alike ([`alike`]) say the same thing.
const SAME_SENTENCE: f64 = 0.5;
/// An attempt says a row whole when it says this share of the content words that the attempt
/// with the most says: four fifths.
const WHOLE: (usize, usize) = (4, 5);
/// A sentence with this many content words is one an attempt that says the row whole must not
/// leave out.
const SUBSTANTIVE: usize = 2;

impl Lineup {
    /// Lines up `takes` against the beats of `script`. `cut` names the groups when the script
    /// has no beats, and its `undone` takes are left out: a person undid their recording.
    pub fn new(script: &Script, cut: &Cut, takes: &[TakeWords<'_>]) -> Self {
        let mut takes: Vec<&TakeWords<'_>> = takes
            .iter()
            .filter(|t| !t.transcript.words().is_empty())
            .filter(|t| !cut.undone.contains(t.transcript.take()))
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
                // Whether an earlier take has an attempt at the row.
                let taken = |row: usize| {
                    lineup
                        .row(places[row])
                        .is_some_and(|r| !r.attempts.is_empty())
                };
                let found = beat_attempts(take, &texts, taken)
                    .unwrap_or_else(|| align(take.transcript.words(), &texts, taken));
                for (row, range) in found {
                    lineup.push(places[row], take.transcript.take(), range);
                }
            }
            lineup.propose(&takes, texts);
        } else {
            lineup.groups.push(Group {
                title: String::new(),
                rows: Vec::new(),
            });
            for take in &takes {
                lineup.add_unscripted(take.transcript);
            }
            lineup.groups = under_chapters(std::mem::take(&mut lineup.groups[0].rows), cut);
            let texts = lineup.rows().map(|(_, r)| content_words(&r.text)).collect();
            lineup.propose(&takes, texts);
        }
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
    /// it, and the rest are new rows, sentence groups: after the rows there are, so a take that
    /// carries on is appended, or right after the row the take said just before them when it
    /// then says that row again or one of the next, at most one row on for each new sentence and
    /// one more, so a sentence added or reworded in a retake plays where it was said. The rows
    /// the take skips there, and with them the rows it added there, are worth a look
    /// ([`Row::look`]), as is one row it skips between two it says again with nothing new. A
    /// take that says again half of its sentences or more is a retake: what it says before the
    /// first row it says again goes before that row, what it says between two rows it says
    /// again goes after the first, and those and what it says after the last are worth a look,
    /// with the rows it leaves unsaid there when they are at most one for each new sentence. The
    /// rows wait in one group for [`under_chapters`].
    fn add_unscripted(&mut self, transcript: &Transcript) {
        let take = transcript.take();
        let words = transcript.words();
        let texts: Vec<Vec<HashSet<String>>> = self
            .rows()
            .map(|(_, row)| {
                let said: Vec<&str> = row.text.split_whitespace().collect();
                let ends = |i: usize| said[i].ends_with(['.', '?', '!']);
                sentence_ranges(said.len(), ends)
                    .into_iter()
                    .map(|s| content_set(said[s].iter().copied()))
                    .collect()
            })
            .collect();
        // Where each row the take may say again is now: a row put in before it moves it on.
        let mut places: Vec<RowAt> = self.rows().map(|(at, _)| at).collect();
        // How many rows the take put right after each row: its next ones go after those.
        let mut put_after = vec![0; places.len()];
        let sentences = sentences(words);
        let said_again: Vec<Option<usize>> = sentences
            .iter()
            .map(|s| said_again(&words[s.clone()], &texts))
            .collect();
        // The rows of `rows` that the take says nowhere.
        let unsaid = |rows: Range<usize>| -> Vec<usize> {
            rows.filter(|row| !said_again.contains(&Some(*row)))
                .collect()
        };
        // A take that says again half of its sentences or more is a retake of what is there.
        let retake = 2 * said_again.iter().flatten().count() >= sentences.len();
        let mut i = 0;
        while i < sentences.len() {
            let row = said_again[i];
            let run = said_again[i..].iter().take_while(|r| **r == row).count();
            let stretch = &words[sentences[i].start..sentences[i + run - 1].end];
            match row {
                Some(row) => {
                    self.push(places[row], take, span(stretch));
                    // One row left out right before this one: a sentence the retake dropped.
                    let before = i.checked_sub(1).and_then(|b| said_again[b]);
                    if let Some(before) = before.filter(|before| before + 2 == row) {
                        for row in unsaid(before + 1..row) {
                            self.groups[0].rows[places[row].row].look = true;
                        }
                    }
                }
                None => {
                    // New between two sentences said again, of one row or of a row and one
                    // that follows it: its place is after the first. The second may be a row on
                    // for each new sentence, which then says a row between them in other words.
                    // What a retake says before the first row it says again goes before that
                    // row, what it says between two rows that are further apart goes after the
                    // first, and what it says after the last stays at the end: each is worth a
                    // look, as a remark or as other words for the rows the retake leaves unsaid
                    // there, when those are at most a row for each new sentence. Anything else
                    // carries on, at the end.
                    let before = i.checked_sub(1).and_then(|b| said_again[b]);
                    let next = said_again.get(i + run).copied().flatten();
                    let between =
                        before.filter(|b| next.is_some_and(|n| n >= *b && n <= *b + 1 + run));
                    // The row the new rows go right after.
                    let after = between.or(before.filter(|_| retake && next.is_some()));
                    let (mut index, skipped, look) = match (between, before, next) {
                        (Some(row), _, Some(next)) => {
                            let skipped = unsaid(row + 1..next);
                            let look = !skipped.is_empty();
                            (Some(places[row].row + 1 + put_after[row]), skipped, look)
                        }
                        (None, None, Some(next)) if retake => {
                            let skipped = if next <= run {
                                unsaid(0..next)
                            } else {
                                Vec::new()
                            };
                            (Some(places[next].row), skipped, true)
                        }
                        (None, Some(before), None) if retake => {
                            let skipped = unsaid(before + 1..texts.len());
                            let reworded = skipped.len() <= run;
                            (None, if reworded { skipped } else { Vec::new() }, true)
                        }
                        (None, Some(before), Some(_)) if retake => {
                            let index = places[before].row + 1 + put_after[before];
                            (Some(index), Vec::new(), true)
                        }
                        _ => (None, Vec::new(), false),
                    };
                    for row in &skipped {
                        self.groups[0].rows[places[*row].row].look = true;
                    }
                    for group in groups(stretch) {
                        let at = self.new_row(group, index);
                        self.groups[0].rows[at.row].look = look;
                        for place in places.iter_mut().filter(|p| p.row >= at.row) {
                            place.row += 1;
                        }
                        index = index.map(|index| index + 1);
                        if let Some(row) = after {
                            put_after[row] += 1;
                        }
                        self.push(at, take, span(group));
                    }
                }
            }
            i += run;
        }
    }

    /// A new row of `words` at `index` of the waiting rows, or after them.
    fn new_row(&mut self, words: &[Word], index: Option<usize>) -> RowAt {
        let text: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        let rows = &mut self.groups[0].rows;
        let row = index.unwrap_or(rows.len());
        rows.insert(row, Row::new(text.join(" ")));
        RowAt { group: 0, row }
    }

    /// Scores every attempt and writes each row's proposal. `texts` are the content words of
    /// each row in order: for a beat the ones the takes were aligned to, its bullet and script.
    /// A row that is already worth a look stays so.
    fn propose(&mut self, takes: &[&TakeWords<'_>], texts: Vec<Vec<String>>) {
        let facts: BTreeMap<&Id, Facts<'_>> = takes
            .iter()
            .map(|t| (t.transcript.take(), Facts::of(t)))
            .collect();
        let mut proposals = Vec::new();
        for ((at, row), text) in self.rows().zip(texts) {
            let beat: HashSet<String> = text.into_iter().collect();
            let scores: Vec<u8> = row
                .attempts
                .iter()
                .map(|a| facts.get(&a.take).map_or(0, |f| f.score(a.range, &beat)))
                .collect();
            let (pieces, look) = proposal(&row.attempts, &scores, &facts);
            let padded = self.joined(pieces);
            proposals.push((at, scores, padded, look));
        }
        for (at, scores, proposal, look) in proposals {
            let row = &mut self.groups[at.group].rows[at.row];
            row.scores = scores;
            row.proposal = proposal;
            row.look |= look;
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
    /// pause and never clips a word's edge: from its first word to its last, then half the gap
    /// to the take's word before and after, at most [`JOIN_PAD`], and [`EDGE_PAD`] where the
    /// take has no word there. Padding a padded piece changes nothing.
    pub fn padded(&self, piece: &Attempt) -> Attempt {
        let Some(words) = self.words.get(&piece.take) else {
            return piece.clone();
        };
        let first = words.partition_point(|w| middle(*w) < piece.range.from());
        let last = words.partition_point(|w| middle(*w) < piece.range.to());
        if first >= last {
            return piece.clone();
        }
        let (from, to) = (words[first].from().0, words[last - 1].to().0);
        let pad = |gap: u64| JOIN_PAD.0.min(gap / 2);
        let from = match first.checked_sub(1).map(|i| words[i].to()) {
            Some(before) => from - pad(from.saturating_sub(before.0)),
            None => from.saturating_sub(EDGE_PAD.0),
        };
        let to = match words.get(last).map(|w| w.from()) {
            Some(after) => to + pad(after.0.saturating_sub(to)),
            None => to + EDGE_PAD.0,
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
            look: false,
        }
    }
}

/// What scoring reads of one take.
struct Facts<'a> {
    words: &'a [Word],
    /// Fillers, retakes and misspoken words.
    stumbles: Vec<TimeRange>,
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
        let said = content_set(words.iter().map(|w| w.text.as_str()));
        let close = if beat.is_empty() {
            1.0
        } else {
            beat.intersection(&said).count() as f64 / beat.len() as f64
        };
        let complete = if ends_sentence(last) { 1.0 } else { 0.0 };
        let score = 50.0 * self.clean(words, &self.stumbles) + 35.0 * close + 15.0 * complete;
        score.round() as u8
    }

    /// The sentences of an attempt, each with its content words and how clean it is, 0 to
    /// 100. A flubbed sentence is one too, so another attempt's saying of it can win.
    fn sentences(&self, range: TimeRange) -> Vec<Said> {
        let words = words_in(self.words, range);
        sentences(words)
            .into_iter()
            .map(|s| {
                let words = &words[s];
                let meant = words
                    .iter()
                    .filter(|w| !self.stumbles.iter().any(|s| holds(*s, middle(w.range))));
                Said {
                    range: span(words),
                    content: content_set(words.iter().map(|w| w.text.as_str())),
                    meant: content_set(meant.map(|w| w.text.as_str())),
                    clean: (100.0 * self.clean(words, &self.stumbles)).round() as u8,
                }
            })
            .collect()
    }
}

/// A sentence of an attempt.
struct Said {
    range: TimeRange,
    /// Its content words, by which sentences are matched.
    content: HashSet<String>,
    /// The content words outside its stumbles: what a sentence that replaces it must say.
    meant: HashSet<String>,
    /// The share of its words that is no stumble, 0 to 100.
    clean: u8,
}

/// The pieces that play a row, see [`Row::proposal`]. The frame is the best scored attempt,
/// the newest of equals, among those that say the row whole: [`WHOLE`] of what the attempt
/// with the most says, and at least half of every [`SUBSTANTIVE`] sentence of that attempt. So
/// a retake of one sentence never stands for the beat. Each sentence of the frame then comes from the attempt
/// that says it cleanest, the newest of equals, where that attempt's sentence says every
/// content word the frame's means, outside its own stumbles. Unpadded, with whether it is worth a look, see
/// [`Row::look`].
fn proposal(
    attempts: &[Attempt],
    scores: &[u8],
    facts: &BTreeMap<&Id, Facts<'_>>,
) -> (Vec<Attempt>, bool) {
    let said: Vec<Vec<Said>> = attempts
        .iter()
        .map(|a| {
            facts
                .get(&a.take)
                .map_or_else(Vec::new, |f| f.sentences(a.range))
        })
        .collect();
    let says = |i: usize| -> HashSet<&String> { said[i].iter().flat_map(|s| &s.meant).collect() };
    let Some(fullest) = (0..attempts.len()).max_by_key(|&i| (says(i).len(), i)) else {
        return (Vec::new(), false);
    };
    let most = says(fullest).len();
    let whole = |i: usize| {
        let says = says(i);
        let leaves_out = |sentence: &Said| {
            let shared = sentence.meant.iter().filter(|w| says.contains(w)).count();
            sentence.meant.len() >= SUBSTANTIVE
                && (shared as f64) < SAME_SENTENCE * sentence.meant.len() as f64
        };
        WHOLE.1 * says.len() >= WHOLE.0 * most && !said[fullest].iter().any(leaves_out)
    };
    let base = (0..attempts.len())
        .filter(|&i| whole(i))
        .max_by_key(|&i| (scores[i], i))
        .unwrap_or(fullest);
    let lead = (0..attempts.len())
        .filter(|&i| i != base)
        .map(|i| scores[base].saturating_sub(scores[i]))
        .min();
    let look = base + 1 < attempts.len() || lead.is_some_and(|lead| (1..=CLOSE).contains(&lead));
    let slots = &said[base];
    // Per slot of the base: the attempt that wins it and its sentence there.
    let mut winners: Vec<(u8, usize, TimeRange)> =
        slots.iter().map(|s| (s.clean, base, s.range)).collect();
    for other in (0..attempts.len()).filter(|&i| i != base) {
        for sentence in &said[other] {
            // What the slot means must be said outside the sentence's own stumbles. Of two
            // slots it is as much like, the cleaner one is the one it says again: the other
            // is the flub that the cleaner one already repeats.
            let slot = slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.meant.is_subset(&sentence.meant))
                .map(|(i, slot)| (alike(&slot.content, &sentence.content), slot.clean, i))
                .filter(|(alike, ..)| *alike >= SAME_SENTENCE)
                .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(b.2.cmp(&a.2)));
            if let Some((_, _, slot)) = slot {
                let (clean, by, _) = winners[slot];
                if (sentence.clean, other) >= (clean, by) {
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
        return (vec![attempts[base].clone()], look);
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
    (pieces, true)
}

/// The rows of a video without beats under the cut's chapters: a row goes under the chapter
/// that keeps the start of one of its attempts. A row the cut plays nothing of stays with the
/// row before it when the same take said both, or when the next row the cut plays, after the
/// rows of its take that the cut places nowhere either, is in that row's chapter, so a chapter
/// is not split by the passage a later take replaced or by the sentences it added, renamed or
/// not. Such a row at the very start opens the chapter of the next row the cut plays, so a
/// retake's new opening is no chapter of its own. Else it goes under its take's name. A take
/// that was just appended whole plays under its own name, `Take <take>`: that chapter holds the
/// take until it is composed and places none of its rows.
fn under_chapters(rows: Vec<Row>, cut: &Cut) -> Vec<Group> {
    // The chapter that plays a row, by its title, `placed` without the take's own chapter.
    let chapter = |row: &Row, placed: bool| -> Option<&str> {
        row.attempts.iter().find_map(|a| {
            let start = TimeRange::new(a.range.from(), Micros(a.range.from().0 + 1))?;
            let plays = |edit: &crate::TakeEdit| edit.take == a.take && edit.keeps(start);
            let own = format!("Take {}", a.take);
            let chapters = cut.chapters.iter().filter(|c| !placed || c.title != own);
            let chapter = chapters.into_iter().find(|c| c.takes.iter().any(plays))?;
            Some(chapter.title.as_str())
        })
    };
    let playing: Vec<Option<&str>> = rows.iter().map(|row| chapter(row, true)).collect();
    let holding: Vec<Option<&str>> = rows.iter().map(|row| chapter(row, false)).collect();
    let said: Vec<Option<Id>> = rows
        .iter()
        .map(|row| row.attempts.first().map(|a| a.take.clone()))
        .collect();
    let mut groups: Vec<Group> = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        let (before, said) = (i.checked_sub(1).and_then(|b| said[b].as_ref()), &said[i..]);
        // The rows of this take that follow and that the cut places nowhere go where this goes.
        let same = |j: &usize| playing[i + j].is_none() && said[*j] == said[0];
        let follow = (1..said.len()).take_while(same).count();
        let next = (i + 1 + follow..holding.len()).find_map(|j| Some((holding[j]?, j - i)));
        let inside = |group: &Group| {
            before == said[0].as_ref() || next.is_some_and(|(title, _)| title == group.title)
        };
        // A row at the very start that a later take said than the row that follows it.
        let opens = |take: &Id, next: usize| {
            let first = said[next].as_ref();
            first.is_some_and(|first| take_order(first) < take_order(take))
        };
        let title = match (playing[i], groups.last(), next, &said[0]) {
            (Some(title), ..) => title.to_owned(),
            (None, Some(group), ..) if inside(group) => group.title.clone(),
            (None, None, Some((title, next)), Some(take)) if opens(take, next) => title.to_owned(),
            (None, _, _, Some(take)) => format!("Take {take}"),
            (None, _, _, None) => String::new(),
        };
        match groups.last_mut() {
            Some(group) if group.title == title => group.rows.push(row),
            _ => groups.push(Group {
                title,
                rows: vec![row],
            }),
        }
    }
    groups
}

/// How alike two sets of content words are, 0 to 1: the share of all their words both have.
fn alike(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let all = a.union(b).count();
    if all == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / all as f64
}

/// The row a sentence of `words` says again: the earliest of those with the sentence most
/// like it ([`alike`]), at least [`SAME_SENTENCE`]. `rows` are each row's sentences. A new
/// sentence that shares words with a long row is therefore no retake of it.
fn said_again(words: &[Word], rows: &[Vec<HashSet<String>>]) -> Option<usize> {
    let said = content_set(words.iter().map(|w| w.text.as_str()));
    rows.iter()
        .enumerate()
        .filter_map(|(i, row)| {
            let most = row.iter().map(|s| alike(s, &said)).max_by(f64::total_cmp)?;
            (most >= SAME_SENTENCE).then_some((most, i))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))
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
/// trimmed to its words. The last one runs until the take stops, so it ends [`before_remark`]
/// when `taken` says an earlier take already has its row. `texts` are the content words of
/// each row. `None` when the take has no beat elements.
fn beat_attempts(
    take: &TakeWords<'_>,
    texts: &[Vec<String>],
    taken: impl Fn(usize) -> bool,
) -> Option<Vec<(usize, TimeRange)>> {
    let beats: Vec<(Micros, usize)> = take
        .elements?
        .of_kind(ElementKind::Beat)
        .filter_map(|e| {
            let index = usize::try_from(e.integer(Attr::Index)?).ok()?;
            (1..=texts.len())
                .contains(&index)
                .then_some((e.t, index - 1))
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
            let mut said = words_in(words, TimeRange::new(t, until)?);
            if i + 1 == beats.len() && taken(row) {
                said = before_remark(said, &texts[row]);
            }
            (!said.is_empty()).then(|| (row, span(said)))
        })
        .collect();
    Some(found)
}

/// `words` up to the last of their sentences that shares a content word with `row`: what a
/// retake says after it is a remark such as "okay, that one was better", not the row. All of
/// them when no sentence shares one.
fn before_remark<'a>(words: &'a [Word], row: &[String]) -> &'a [Word] {
    let shares = |sentence: &&Range<usize>| {
        let said = content_set(words[(*sentence).clone()].iter().map(|w| w.text.as_str()));
        row.iter().any(|word| said.contains(word))
    };
    let sentences = sentences(words);
    let end = sentences
        .iter()
        .rev()
        .find(shares)
        .map_or(words.len(), |s| s.end);
    &words[..end]
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
    sentence_ranges(words.len(), |i| ends_sentence(&words[i]))
}

/// `len` words as sentences, each up to a word that `ends` one.
fn sentence_ranges(len: usize, ends: impl Fn(usize) -> bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..len {
        if ends(i) || i + 1 == len {
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
/// A sentence that shares nothing stays with the row before it, and the sentences before the
/// first that shares a word lead into that row. Each run of sentences on one row becomes an
/// attempt from its first sentence to its last. Where `taken` says an earlier take already
/// has the row, the attempt is the retake only: a lead-in belongs to no row, so "okay, once
/// more" is no attempt, and the attempt ends [`before_remark`].
fn align(
    words: &[Word],
    texts: &[Vec<String>],
    taken: impl Fn(usize) -> bool,
) -> Vec<(usize, TimeRange)> {
    let sentences = sentences(words);
    if sentences.is_empty() || texts.is_empty() {
        return Vec::new();
    }
    let rows: Vec<HashSet<&str>> = texts
        .iter()
        .map(|r| r.iter().map(String::as_str).collect())
        .collect();
    let said: Vec<HashSet<String>> = sentences
        .iter()
        .map(|sentence| content_set(words[sentence.clone()].iter().map(|w| w.text.as_str())))
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
    let mut out: Vec<(usize, TimeRange)> = Vec::new();
    let mut j = if taken(rows_of[lead]) { lead } else { 0 };
    while j < sentences.len() {
        let row = rows_of[j.max(lead)];
        let on_row = |k: &usize| rows_of[(*k).max(lead)] == row;
        let run = (j..sentences.len()).take_while(on_row).count();
        let said = &words[sentences[j].start..sentences[j + run - 1].end];
        let said = if taken(row) {
            before_remark(said, &texts[row])
        } else {
            said
        };
        out.push((row, span(said)));
        j += run;
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

/// The [`content_words`] of `words` as a set.
fn content_set<'a>(words: impl Iterator<Item = &'a str>) -> HashSet<String> {
    let text: Vec<&str> = words.collect();
    content_words(&text.join(" ")).into_iter().collect()
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

    /// The words `cut` plays of `takes`, in cut order.
    fn spoken(cut: &Cut, takes: &[&Transcript]) -> String {
        let blocks = cut.chapters.iter().flat_map(|c| &c.takes);
        let words = blocks.flat_map(|edit| {
            let take = takes.iter().find(|t| t.take() == &edit.take).unwrap();
            let kept = take.words().iter().filter(|w| edit.keeps(w.range));
            kept.map(|w| w.text.as_str())
        });
        words.collect::<Vec<_>>().join(" ")
    }

    /// `takes` appended to a cut, each as the chapter named with it, and composed twice. The
    /// second compose changes nothing.
    fn composed(script: &Script, takes: &[(&Transcript, Option<&Elements>, &str)]) -> Cut {
        let whole = TimeRange::new(Micros(0), Micros(120_000_000)).unwrap();
        let words: Vec<TakeWords<'_>> = takes.iter().map(|t| with(t.0, t.1)).collect();
        let mut cut = Cut::default();
        for (take, _, title) in takes {
            cut = cut.append_take(take.take().clone(), *title, whole);
        }
        let new = takes.last().map(|t| t.0.take());
        let once = cut
            .compose(&Lineup::new(script, &cut, &words), new)
            .unwrap();
        let twice = once
            .compose(&Lineup::new(script, &once, &words), new)
            .unwrap();
        assert_eq!(once, twice);
        twice
    }

    #[test]
    fn a_remark_after_a_retake_of_a_beat_is_no_attempt() {
        let script = Script::parse(
            "# A\n\n## Setup\n\n- Create the project with cargo\n\n## Arguments\n\n- Add the clap crate to parse the arguments\n- Run it with a flag\n",
        )
        .unwrap();
        let full = "First we create the project with cargo. \
                    Then we add the clap crate to parse the arguments. Now run it with a flag.";
        let retake = "Then we add the clap crate to parse the arguments.";
        let remark = "Okay, I think that one was much better, let us keep that.";
        let first = said("t01", 0, full);
        let second = said("t02", 0, &format!("{retake} {remark}"));
        // Aligned by its words.
        let cut = composed(
            &script,
            &[(&first, None, "Take t01"), (&second, None, "Take t02")],
        );
        assert_eq!(spoken(&cut, &[&first, &second]), full);
        // Recorded with the prompter, which showed the beat until the take stopped.
        let mut beats = Elements::new(id("t02"));
        beats
            .push(Element::at(ElementKind::Beat, Micros(0), "script").with_integer(Attr::Index, 2))
            .unwrap();
        let cut = composed(
            &script,
            &[
                (&first, None, "Take t01"),
                (&second, Some(&beats), "Take t02"),
            ],
        );
        assert_eq!(spoken(&cut, &[&first, &second]), full);
        let lineup = Lineup::new(
            &script,
            &cut,
            &[with(&first, None), with(&second, Some(&beats))],
        );
        // The retake plays the beat, without its remark.
        assert_eq!(proposals(&lineup)[1], [("t02".into(), 0, 42)]);
        // A first take's closing words still belong to its last beat.
        let alone = said("t01", 0, &format!("{retake} {remark}"));
        let lineup = Lineup::new(&script, &Cut::default(), &[with(&alone, None)]);
        assert_eq!(proposals(&lineup)[1], [("t01".into(), 0, 103)]);
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
    fn a_retake_of_one_sentence_never_stands_for_the_whole_beat() {
        // Both score 100 against the bullet, and the second take says a third of the beat.
        let first = said(
            "t01",
            1,
            "We install the speech model. It verifies a checksum on disk. Then we go on.",
        );
        let second = said("t02", 1, "We install the speech model.");
        let script = Script::parse("# A\n\n## Setup\n\n- Install the speech model\n").unwrap();
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&second, None)],
        );
        assert_eq!(lineup.groups[0].rows[0].scores, [100, 100]);
        assert_eq!(
            proposals(&lineup)[0],
            [("t02".into(), 9, 29), ("t01".into(), 43, 101)]
        );
    }

    #[test]
    fn an_attempt_that_skips_the_beats_script_scores_lower() {
        let script = Script::parse(
            "# A\n\n## Setup\n\n- Install the speech model\n\n  It verifies a checksum on disk.\n",
        )
        .unwrap();
        let first = said(
            "t01",
            0,
            "Install the speech model. It verifies a checksum on disk.",
        );
        let second = said("t02", 0, "Install the speech model.");
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&second, None)],
        );
        let row = &lineup.groups[0].rows[0];
        assert_eq!(row.scores[0], 100);
        assert!(row.scores[1] < 85, "{:?}", row.scores);
    }

    #[test]
    fn a_later_sentence_that_says_less_or_holds_a_filler_does_not_replace() {
        let script = Script::parse("# A\n\n## Setup\n\n- The loader and the router\n").unwrap();
        let first = said(
            "t01",
            0,
            "The loader runs on the server before the page renders. The router keeps routes next to pages.",
        );
        // The first sentence drops "before the page renders", the second holds a filler.
        let second = said(
            "t02",
            0,
            "The loader runs on the server. The router basically keeps routes next to pages.",
        );
        let mut filler = Elements::new(id("t02"));
        filler
            .push(Element::over(
                ElementKind::Filler,
                TimeRange::new(Micros(5_200_000), Micros(5_500_000)).unwrap(),
                "whisper",
            ))
            .unwrap();
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&second, Some(&filler))],
        );
        assert_eq!(proposals(&lineup)[0], [("t01".into(), 0, 83)]);
    }

    #[test]
    fn a_flubbed_sentence_is_replaced_by_another_attempts_clean_one() {
        let first = said(
            "t01",
            1,
            "Install the speech model, install the speech model. It verifies a checksum on disk.",
        );
        let second = said("t02", 1, "Install the speech model.");
        // Half of the first sentence is a restart.
        let mut restart = Elements::new(id("t01"));
        restart
            .push(Element::over(
                ElementKind::Retake,
                TimeRange::new(Micros(1_000_000), Micros(2_500_000)).unwrap(),
                "whisper",
            ))
            .unwrap();
        let script = Script::parse("# A\n\n## Setup\n\n- Install the speech model\n").unwrap();
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, Some(&restart)), with(&second, None)],
        );
        assert_eq!(
            proposals(&lineup)[0],
            [("t02".into(), 9, 25), ("t01".into(), 55, 81)]
        );
    }

    #[test]
    fn a_sentence_that_says_a_needed_word_only_in_its_own_stumble_does_not_replace() {
        let script = Script::parse("# A\n\n## Setup\n\n- The loader and the router\n").unwrap();
        let first = said(
            "t01",
            0,
            "The loader, the loader, the loader runs on the server. The router keeps routes next to pages.",
        );
        // The retake says "server" only in the words it starts over, and then "client".
        let second = said("t02", 0, "The server, the loader runs on the client.");
        let stumble = |take: &str, to: u64| {
            let mut elements = Elements::new(id(take));
            elements
                .push(Element::over(
                    ElementKind::Retake,
                    TimeRange::new(Micros(0), Micros(to)).unwrap(),
                    "whisper",
                ))
                .unwrap();
            elements
        };
        let (flubbed, restarted) = (stumble("t01", 1_600_000), stumble("t02", 800_000));
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[
                with(&first, Some(&flubbed)),
                with(&second, Some(&restarted)),
            ],
        );
        let takes: Vec<String> = proposals(&lineup)[0].iter().map(|p| p.0.clone()).collect();
        assert_eq!(takes, ["t01"]);
    }

    #[test]
    fn a_retake_of_a_sentence_said_twice_replaces_the_clean_one() {
        let script = Script::parse("# A\n\n## Setup\n\n- The loader on the server\n").unwrap();
        // The first sentence is the flub the restart detector found, from 0 to 4.3 s. The
        // second, from 6 to 9.9 s, says it again.
        let first = said(
            "t01",
            0,
            "The loader run on the server before the page renders. The loader runs on the server before the page renders. Then the browser hydrates every component with cached data.",
        );
        let second = said(
            "t02",
            0,
            "The loader runs on the server before the page renders.",
        );
        let mut restart = Elements::new(id("t01"));
        restart
            .push(Element::over(
                ElementKind::Retake,
                TimeRange::new(Micros(0), Micros(4_300_000)).unwrap(),
                "whisper",
            ))
            .unwrap();
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, Some(&restart)), with(&second, None)],
        );
        let pieces = &proposals(&lineup)[0];
        assert_eq!(pieces.iter().filter(|p| p.0 == "t02").count(), 1);
        // Nothing of the first take's clean sentence plays beside the retake of it.
        let twice = |p: &&(String, u64, u64)| p.0 == "t01" && p.1 < 99 && p.2 > 60;
        assert_eq!(pieces.iter().find(twice), None, "{pieces:?}");
    }

    #[test]
    fn without_beats_a_sentence_added_between_two_said_again_is_a_row_after_theirs() {
        let script = Script::parse("# A\n").unwrap();
        // One word per 400 ms and no pause between sentences, `|` is a pause of 2 s.
        let fluent = |take: &str, text: &str| {
            let mut transcript = Transcript::new(id(take));
            let mut t = 0;
            for word in text.split_whitespace() {
                if word == "|" {
                    t += 2_000_000;
                    continue;
                }
                let range = TimeRange::new(Micros(t), Micros(t + 300_000)).unwrap();
                transcript
                    .push(Word {
                        text: word.to_owned(),
                        range,
                        confidence: 90,
                    })
                    .unwrap();
                t += 400_000;
            }
            transcript
        };
        let first = fluent(
            "t01",
            "Nightly backups copy the database offsite. Restores are tested every single week. | Caching comes last.",
        );
        let second = fluent(
            "t02",
            "Nightly backups copy the database offsite. Basically encrypt backups. Restores are tested every single week.",
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
                "Nightly backups copy the database offsite. Restores are tested every single week.",
                "Basically encrypt backups.",
                "Caching comes last."
            ]
        );
        assert_eq!(
            texts(&lineup),
            [
                vec![("t01".into(), 0), ("t02".into(), 0), ("t02".into(), 36)],
                vec![("t02".into(), 24)],
                vec![("t01".into(), 68)],
            ]
        );
        assert_eq!(proposals(&lineup)[1], [("t02".into(), 23, 35)]);
        // Added between two paragraphs said again, it is a row between theirs, not the last.
        let third = fluent(
            "t03",
            "Nightly backups copy the database offsite. Restores are tested every single week. | Always encrypt them first. | Caching comes last.",
        );
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&third, None)],
        );
        let rows: Vec<&str> = lineup.rows().map(|(_, r)| r.text.as_str()).collect();
        assert_eq!(
            rows[1..],
            ["Always encrypt them first.", "Caching comes last."]
        );
        assert_eq!(lineup.rows().nth(2).unwrap().1.attempts.len(), 2);
    }

    #[test]
    fn without_beats_a_take_that_carries_on_is_appended_after_a_sentence_it_repeats() {
        let script = Script::parse("# A\n").unwrap();
        let first = said(
            "t01",
            0,
            "Welcome to the channel. Let's look at the code. The parser reads every token. Thanks for watching today.",
        );
        let second = said(
            "t02",
            0,
            "Now for the second part. Let's look at the code. The renderer draws every frame. See you next week.",
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
                "Welcome to the channel.",
                "Let's look at the code.",
                "The parser reads every token.",
                "Thanks for watching today.",
                "Now for the second part.",
                "The renderer draws every frame.",
                "See you next week.",
            ]
        );
        assert!(lineup.rows().all(|(_, row)| !row.look));
    }

    #[test]
    fn without_beats_a_retakes_new_opening_and_ending_are_placed_and_worth_a_look() {
        let script = Script::parse("# A\n").unwrap();
        let (a, b, c) = (
            "The parser reads every token.",
            "The lexer splits the input.",
            "The renderer draws every frame.",
        );
        let first = said("t01", 0, &format!("{a} {b} {c}"));
        // The rows after `later` was recorded, each with whether it is worth a look.
        let rows = |later: &str| -> Vec<(String, bool)> {
            let second = said("t02", 0, later);
            let takes = [with(&first, None), with(&second, None)];
            let lineup = Lineup::new(&script, &Cut::default(), &takes);
            let rows = lineup.rows().map(|(_, row)| (row.text.clone(), row.look));
            rows.collect()
        };
        let row = |text: &str, look: bool| (text.to_owned(), look);
        // Chatter before the retake starts is not the end of the video.
        let chatter = "Okay take two here we go.";
        assert_eq!(
            rows(&format!("{chatter} {a} {b} {c}")),
            [
                row(chatter, true),
                row(a, false),
                row(b, false),
                row(c, false)
            ]
        );
        // A new opening may say the first row in other words.
        let opening = "Tokens come from our tokenizer.";
        assert_eq!(
            rows(&format!("{opening} {b} {c}")),
            [
                row(a, true),
                row(opening, true),
                row(b, false),
                row(c, false)
            ]
        );
        // A new ending may say the last row in other words.
        let ending = "Pixels get drawn last.";
        assert_eq!(
            rows(&format!("{a} {b} {ending}")),
            [
                row(a, false),
                row(b, false),
                row(c, true),
                row(ending, true)
            ]
        );
    }

    #[test]
    fn without_beats_a_retakes_remark_plays_where_it_was_said_and_is_worth_a_look() {
        let script = Script::parse("# A\n").unwrap();
        let whole = TimeRange::new(Micros(0), Micros(120_000_000)).unwrap();
        // The sentences the cut plays after `later` was recorded and composed, and the rows
        // worth a look. Composing again changes neither.
        let played = |first: &[&str], later: &str| -> (String, Vec<String>) {
            let first = said("t01", 0, &first.join(" "));
            let second = said("t02", 0, later);
            let takes = [with(&first, None), with(&second, None)];
            let mut cut = Cut::default()
                .append_take(id("t01"), "Intro", whole)
                .append_take(id("t02"), "Take t02", whole);
            let mut rounds = Vec::new();
            for _ in 0..2 {
                cut = cut
                    .compose(&Lineup::new(&script, &cut, &takes), Some(&id("t02")))
                    .unwrap();
                let blocks = cut.chapters.iter().flat_map(|c| &c.takes);
                let words = blocks.flat_map(|edit| {
                    let take = if edit.take == id("t01") {
                        &first
                    } else {
                        &second
                    };
                    let kept = take.words().iter().filter(|w| edit.keeps(w.range));
                    kept.map(|w| w.text.as_str())
                });
                let lineup = Lineup::new(&script, &cut, &takes);
                let looks = lineup.rows().filter(|(_, row)| row.look);
                rounds.push((
                    words.collect::<Vec<_>>().join(" "),
                    looks.map(|(_, row)| row.text.clone()).collect(),
                ));
            }
            assert_eq!(rounds[0], rounds[1]);
            rounds.remove(0)
        };
        let long = [
            "Welcome to the channel.",
            "The parser reads every token.",
            "The lexer splits the input.",
            "The renderer draws every frame.",
            "Caching comes last.",
            "Nightly backups copy the database offsite.",
            "Restores are tested every single week.",
            "Sessions live in the cookie store.",
            "Rotate the keys monthly.",
            "Thanks for watching today.",
        ];
        // `rows` of `of` with `remark` before the row at `at`, as the cut plays them.
        let with_remark = |of: &[&str], at: usize, remark: &str| -> (String, Vec<String>) {
            let mut rows = of.to_vec();
            rows.insert(at, remark);
            (rows.join(" "), vec![remark.to_owned()])
        };
        // A remark before a pickup from the middle plays before the row it leads into.
        let remark = "Okay from the caching part again.";
        assert_eq!(
            played(&long, &format!("{remark} {}", long[4..].join(" "))),
            with_remark(&long, 4, remark)
        );
        // A remark after a pickup stays at the end, and is marked whatever the take left unsaid.
        let remark = "Yeah that sounded better somehow.";
        assert_eq!(
            played(&long, &format!("{} {remark}", long[4])),
            with_remark(&long, 10, remark)
        );
        let remark = "Okay I think that was good.";
        assert_eq!(
            played(&long, &format!("{} {remark}", long[4..8].join(" "))),
            with_remark(&long, 10, remark)
        );
        // A remark between two pickups plays after the first.
        let remark = "Alright now the other sentence.";
        assert_eq!(
            played(&long, &format!("{} {remark} {}", long[2], long[7])),
            with_remark(&long, 3, remark)
        );
        // The same with three sentences.
        let short = &long[1..4];
        let remark = "Okay, take two.";
        assert_eq!(
            played(short, &format!("{remark} {}", short[2])),
            with_remark(short, 2, remark)
        );
        let remark = "That sounded right";
        assert_eq!(
            played(short, &format!("{} {remark}", short[0])),
            with_remark(short, 3, remark)
        );
    }

    #[test]
    fn without_beats_a_retakes_new_opening_is_in_the_chapter_of_the_row_it_leads_into() {
        let script = Script::parse("# A\n").unwrap();
        let text = "The parser reads every token. The lexer splits the input.";
        let first = said("t01", 0, text);
        let second = said("t02", 0, &format!("Okay take two here we go. {text}"));
        let whole = TimeRange::new(Micros(0), Micros(20_000_000)).unwrap();
        let takes = [with(&first, None), with(&second, None)];
        let mut cut = Cut::default()
            .append_take(id("t01"), "Intro", whole)
            .append_take(id("t02"), "Take t02", whole);
        for _ in 0..2 {
            let lineup = Lineup::new(&script, &cut, &takes);
            let groups: Vec<(&str, usize)> = lineup
                .groups
                .iter()
                .map(|g| (g.title.as_str(), g.rows.len()))
                .collect();
            assert_eq!(groups, [("Intro", 3)]);
            cut = cut.compose(&lineup, Some(&id("t02"))).unwrap();
            let titles: Vec<&str> = cut.chapters.iter().map(|c| c.title.as_str()).collect();
            assert_eq!(titles, ["Intro"]);
        }
    }

    #[test]
    fn a_take_whose_recording_was_undone_is_not_lined_up() {
        let script = Script::parse("# A\n").unwrap();
        let first = said("t01", 0, "The loader runs on the server.");
        let second = said("t02", 0, "The loader always runs on the server.");
        let cut = Cut::parse("takes {\n    undone t02\n}\n").unwrap();
        assert_eq!(Cut::parse(&cut.to_kdl()).unwrap(), cut);
        assert_eq!(cut.to_kdl(), "takes {\n    undone t02\n}\n");
        let lineup = Lineup::new(&script, &cut, &[with(&first, None), with(&second, None)]);
        assert_eq!(lineup.takes(), [id("t01")]);
    }

    #[test]
    fn without_beats_a_new_sentence_that_shares_words_with_a_long_row_is_appended() {
        let script = Script::parse("# A\n").unwrap();
        let first = said(
            "t01",
            0,
            "The router keeps routes next to pages, the loader runs on the server and the cache stores every rendered page.",
        );
        let second = said("t02", 0, "Our loader cache stores sessions too.");
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&second, None)],
        );
        assert_eq!(
            texts(&lineup),
            [vec![("t01".into(), 0)], vec![("t02".into(), 0)]]
        );
    }

    #[test]
    fn without_beats_a_renamed_chapter_stays_whole_around_a_later_takes_passage() {
        let script = Script::parse("# A\n").unwrap();
        let first = said(
            "t01",
            0,
            "The router keeps routes next to pages. The loader runs on the server. Caching comes last.",
        );
        let second = said("t02", 0, "The loader always runs on the server.");
        // The chapter a person renamed, with the second take's sentence composed into it.
        let cut = Cut::parse(
            "chapter \"Intro\" {\n    take t01 {\n        keep from=0 to=3_500_000\n    }\n    take t02 {\n        keep from=0 to=3_000_000\n    }\n    take t01 {\n        keep from=8_000_000 to=10_000_000\n    }\n}\n",
        )
        .unwrap();
        let lineup = Lineup::new(&script, &cut, &[with(&first, None), with(&second, None)]);
        let groups: Vec<(&str, usize)> = lineup
            .groups
            .iter()
            .map(|g| (g.title.as_str(), g.rows.len()))
            .collect();
        assert_eq!(groups, [("Intro", 3)]);
    }

    #[test]
    fn a_retake_of_the_first_sentence_keeps_the_sentence_only_the_first_take_says() {
        // The retake says four fifths of the beat's content words and scores as well.
        let line = "Install and configure the local speech model using the package manager.";
        let first = said("t01", 1, &format!("{line} Verify checksums."));
        let second = said("t02", 1, line);
        let script = Script::parse(&format!("# A\n\n## Setup\n\n- {line}\n")).unwrap();
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&second, None)],
        );
        assert_eq!(lineup.groups[0].rows[0].scores, [100, 100]);
        assert_eq!(
            proposals(&lineup)[0],
            [("t02".into(), 9, 53), ("t01".into(), 67, 77)]
        );
    }

    #[test]
    fn without_beats_a_reworded_sentence_plays_where_it_was_said() {
        let script = Script::parse("# A\n").unwrap();
        let first = said(
            "t01",
            0,
            "Welcome to the channel. The parser reads every token. The lexer splits the input. \
             The renderer draws every frame. Thanks for watching today.",
        );
        // The second sentence in other words: it shares "token" only.
        let second = said(
            "t02",
            0,
            "Welcome to the channel. Tokens come from our tokenizer. The lexer splits the input. \
             The renderer draws every frame. Thanks for watching today.",
        );
        let rows = |takes: &[TakeWords<'_>]| -> Vec<String> {
            let lineup = Lineup::new(&script, &Cut::default(), takes);
            lineup.rows().map(|(_, r)| r.text.clone()).collect()
        };
        assert_eq!(
            rows(&[with(&first, None), with(&second, None)]),
            [
                "Welcome to the channel.",
                "Tokens come from our tokenizer.",
                "The parser reads every token.",
                "The lexer splits the input.",
                "The renderer draws every frame.",
                "Thanks for watching today.",
            ]
        );
        // A take that says again its intro and outro, two of its three sentences, is a retake:
        // what it says between them plays after the intro, not after the outro.
        let third = said(
            "t03",
            0,
            "Welcome to the channel. Sessions live in the cookie store. Thanks for watching today.",
        );
        assert_eq!(
            rows(&[with(&first, None), with(&third, None)])[..3],
            [
                "Welcome to the channel.",
                "Sessions live in the cookie store.",
                "The parser reads every token."
            ]
        );
    }

    #[test]
    fn without_beats_a_second_sentence_added_after_a_row_plays_after_the_first() {
        let script = Script::parse("# A\n").unwrap();
        let first = said(
            "t01",
            0,
            "Nightly backups copy the database offsite. Caching comes last.",
        );
        let second = said(
            "t02",
            0,
            "Nightly backups copy the database offsite. Always encrypt them first. \
             Nightly backups copy the database offsite. Rotate the keys monthly. \
             Nightly backups copy the database offsite.",
        );
        let lineup = Lineup::new(
            &script,
            &Cut::default(),
            &[with(&first, None), with(&second, None)],
        );
        let rows: Vec<&str> = lineup.rows().map(|(_, r)| r.text.as_str()).collect();
        assert_eq!(
            rows[1..],
            [
                "Always encrypt them first.",
                "Rotate the keys monthly.",
                "Caching comes last."
            ]
        );
    }

    #[test]
    fn without_beats_a_sentence_added_inside_a_chapter_does_not_split_it() {
        let script = Script::parse("# A\n").unwrap();
        let first = said(
            "t01",
            0,
            "The router keeps routes next to pages. The loader runs on the server. Caching comes last.",
        );
        let second = said(
            "t02",
            0,
            "The router keeps routes next to pages. Always encrypt the cookies. \
             The loader runs on the server. One more thing about sessions.",
        );
        // The cut plays the first take only: the second is not composed into it yet.
        let cut = Cut::parse(
            "chapter \"Intro\" {\n    take t01 {\n        keep from=0 to=20_000_000\n    }\n}\n",
        )
        .unwrap();
        let lineup = Lineup::new(&script, &cut, &[with(&first, None), with(&second, None)]);
        let groups: Vec<(&str, usize)> = lineup
            .groups
            .iter()
            .map(|g| (g.title.as_str(), g.rows.len()))
            .collect();
        // The sentence added between two the chapter plays is in it, the one at the end is not.
        assert_eq!(groups, [("Intro", 4), ("Take t02", 1)]);
    }

    #[test]
    fn without_beats_an_appended_takes_own_chapter_places_none_of_its_rows() {
        let script = Script::parse("# A\n").unwrap();
        let first = said(
            "t01",
            0,
            "The router keeps routes next to pages. The loader runs on the server.",
        );
        let second = said(
            "t02",
            0,
            "The router keeps routes next to pages. Always encrypt the cookies. \
             Rotate the keys monthly. The loader runs on the server.",
        );
        let whole = TimeRange::new(Micros(0), Micros(20_000_000)).unwrap();
        let takes = [with(&first, None), with(&second, None)];
        // A recording enters the cut as a chapter of its own, and the cut is composed after.
        for (title, chapters) in [("Intro", ["Intro"]), ("Take t01", ["Take t01"])] {
            let mut cut = Cut::default()
                .append_take(id("t01"), title, whole)
                .append_take(id("t02"), "Take t02", whole);
            for _ in 0..2 {
                cut = cut
                    .compose(&Lineup::new(&script, &cut, &takes), Some(&id("t02")))
                    .unwrap();
                let titles: Vec<&str> = cut.chapters.iter().map(|c| c.title.as_str()).collect();
                assert_eq!(titles, chapters);
            }
        }
    }

    #[test]
    fn without_beats_rows_a_take_skips_between_two_it_repeats_are_worth_a_look() {
        let script = Script::parse("# A\n").unwrap();
        let (intro, outro) = ("Welcome to the channel.", "Thanks for watching today.");
        let first = said(
            "t01",
            0,
            &format!(
                "{intro} The parser reads every token. The lexer splits the input. \
                 The renderer draws every frame. {outro}"
            ),
        );
        // The rows worth a look after `later` was recorded.
        let looks = |later: &str| -> Vec<String> {
            let second = said("t02", 0, later);
            let takes = [with(&first, None), with(&second, None)];
            let lineup = Lineup::new(&script, &Cut::default(), &takes);
            let rows = lineup.rows().filter(|(_, row)| row.look);
            rows.map(|(_, row)| row.text.clone()).collect()
        };
        // One sentence in other words: it and the one it stands for both play.
        assert_eq!(
            looks(&format!(
                "{intro} Tokens come from our tokenizer. The lexer splits the input. \
                 The renderer draws every frame. {outro}"
            )),
            [
                "Tokens come from our tokenizer.",
                "The parser reads every token."
            ]
        );
        // Three in a row.
        assert_eq!(
            looks(&format!(
                "{intro} Tokens come from our tokenizer. We chop text up first. \
                 Pixels get drawn last. {outro}"
            )),
            [
                "Tokens come from our tokenizer.",
                "We chop text up first.",
                "Pixels get drawn last.",
                "The parser reads every token.",
                "The lexer splits the input.",
                "The renderer draws every frame."
            ]
        );
        // A sentence the retake left out, with nothing new in its place.
        assert_eq!(
            looks(&format!(
                "{intro} The parser reads every token. The renderer draws every frame. {outro}"
            )),
            ["The lexer splits the input."]
        );
        // A sentence added between two rows that follow each other skips nothing.
        assert_eq!(
            looks(&format!(
                "{intro} Sessions live in the cookie store. The parser reads every token. \
                 The lexer splits the input. The renderer draws every frame. {outro}"
            )),
            [""; 0]
        );
        // A take that carries on and lands between two stock lines.
        let short = said(
            "t01",
            0,
            &format!("{intro} The parser reads every token. {outro}"),
        );
        let carries_on = said(
            "t02",
            0,
            &format!("{intro} Sessions live in the cookie store. {outro}"),
        );
        let takes = [with(&short, None), with(&carries_on, None)];
        let lineup = Lineup::new(&script, &Cut::default(), &takes);
        let looks: Vec<bool> = lineup.rows().map(|(_, row)| row.look).collect();
        assert_eq!(looks, [false, true, true, false]);
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
