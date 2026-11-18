//! The detectors behind retake, misspoken, filler, idle, silence, pause and emphasis
//! elements: each turns a take's transcript, speech, screen and markers into findings with a
//! confidence. Nothing here knows the cut.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::ops::Range;

use shot_core::{ElementKind, Micros, TimeRange, Transcript, Word, cut_before};

use crate::{Speech, normalize};

/// What the detectors know about one take, all in its media time.
pub struct TakeFacts<'a> {
    pub transcript: &'a Transcript,
    pub speech: &'a Speech,
    /// Stretches where the screen does not change, empty without a screen track.
    pub still: &'a [TimeRange],
    /// Times of the `again` markers in the event log.
    pub again: &'a [Micros],
    pub duration: Micros,
}

/// One thing a detector found, with how sure it is in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    pub kind: ElementKind,
    pub range: TimeRange,
    pub confidence: u8,
}

/// Words closer than this to a speech range voice it.
const SPEECH_PAD: u64 = 200_000;
/// Repeats further apart than this are not a stumble.
const REPEAT_GAP: Micros = Micros(2_000_000);
/// A passage said again within this is a retake.
const RESTART_WITHIN: Micros = Micros(20_000_000);
/// How many words a first attempt and what follows it may run before the second one starts.
const MAX_ATTEMPT: usize = 80;
/// The most added, dropped or changed words an alignment holds, see [`allowed_changes`].
const MAX_CHANGES: usize = 3;
/// A sentence this short between two attempts goes with the first one ("It is, hmm, wait.").
const MAX_ASIDE: usize = 6;
/// Words before a repeated passage that open its sentence go with it ("Now the second change").
const MAX_LEADING: usize = 2;
/// Words after a cue that may come before the repeated passage starts ("Okay, the second").
const MAX_CUE_LEAD_IN: usize = 2;
/// Words at least this alike ([`similarity`]) count as the same word said again.
const SOUNDS_ALIKE: f64 = 0.6;
/// How far back an `again` marker looks for the start of the attempt it repeats.
const AGAIN_LOOKBACK: Micros = Micros(120_000_000);

/// Filler words and phrases with their base confidence. Single sounds are almost always noise;
/// the phrases often carry meaning, so they ask.
const FILLERS: &[(&[&str], u8)] = &[
    (&["um"], 92),
    (&["uh"], 92),
    (&["uhm"], 92),
    (&["erm"], 92),
    (&["er"], 90),
    (&["ah"], 88),
    (&["eh"], 88),
    (&["hmm"], 88),
    (&["mm"], 88),
    (&["you", "know"], 55),
    (&["i", "mean"], 50),
    (&["basically"], 50),
    (&["honestly"], 50),
    (&["literally"], 45),
    (&["kind", "of"], 40),
    (&["sort", "of"], 40),
];

/// Phrases that announce a correction of what came just before.
const CORRECTIONS: &[&[&str]] = &[
    &["sorry"],
    &["i", "mean"],
    &["i", "meant"],
    &["no", "wait"],
    &["wait", "no"],
    &["or", "rather"],
    &["let", "me", "rephrase"],
    &["correction"],
];

/// Phrases that announce the speaker starts over, wherever they are said.
const SAY_AGAIN: &[&[&str]] = &[
    &["let", "me", "say", "that", "again"],
    &["let", "me", "redo", "that"],
    &["let", "me", "try", "that", "again"],
];

/// Phrases that announce a new attempt only as a sentence of their own, next to [`CORRECTIONS`].
const ALONE: &[&[&str]] = &[&["one", "more", "time"], &["again"]];

/// Words that double in correct speech ("I know that that works").
const DOUBLES: &[&str] = &["that", "had"];

/// Words nobody doubles on purpose: articles, prepositions, pronouns, conjunctions and
/// auxiliaries. Any other doubled word may be emphasis ("very, very good"), and "so so" is one.
const FUNCTION_WORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "in", "on", "at", "by", "for", "with", "from", "into", "as", "i",
    "you", "he", "she", "it", "we", "they", "me", "him", "us", "them", "my", "your", "his", "her",
    "its", "our", "their", "this", "these", "those", "and", "or", "but", "if", "because", "than",
    "is", "are", "was", "were", "be", "been", "am", "do", "does", "did", "has", "have", "will",
    "would", "can", "could", "should", "shall", "may", "might", "must",
];

/// Fillers that belong to the speaker's voice: kept counts by phrase.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Voice {
    pub kept: HashMap<String, u32>,
}

impl Voice {
    /// A filler's confidence after the person kept it `kept` times: each Keep lowers it, and
    /// three make it part of the voice, no longer found.
    fn confidence(&self, phrase: &str, base: u8) -> Option<u8> {
        let kept = self.kept.get(phrase).copied().unwrap_or(0);
        (kept < 3).then(|| base.saturating_sub(20 * kept as u8))
    }
}

/// A word with its normalized text and its index in the transcript.
struct W<'a> {
    word: &'a Word,
    text: String,
}

impl W<'_> {
    fn from(&self) -> Micros {
        self.word.range.from()
    }

    fn to(&self) -> Micros {
        self.word.range.to()
    }

    fn ends_sentence(&self) -> bool {
        self.word.text.trim_end().ends_with(['.', '?', '!'])
    }
}

/// Runs the retake, misspoken, filler and idle detectors. A finding at or above `sure` percent
/// is one a cut would apply right away, so a stumble around a sure filler leaves the filler to
/// its own finding. Idle findings need still screen at least `idle_min` long.
pub fn find(facts: &TakeFacts, voice: &Voice, sure: u8, idle_min: Micros) -> Vec<Finding> {
    let words: Vec<W> = facts
        .transcript
        .words()
        .iter()
        .map(|word| W {
            word,
            text: normalize(&word.text),
        })
        .filter(|w| !w.text.is_empty())
        .collect();
    let gaps = Gaps::new(facts, &words);
    let fillers = fillers(&words, voice);
    let mut found = fillers.clone();
    found.extend(corrections(&words));
    // Fillers sit inside stumbles ("the, um, the settings"), so repeats are found without the
    // ones that will go.
    let fluent: Vec<&W> = words
        .iter()
        .filter(|w| {
            !fillers
                .iter()
                .any(|f| f.confidence >= sure && f.range.overlaps(w.word.range))
        })
        .collect();
    found.extend(retakes(&fluent));
    found.extend(again(&words, facts.again));
    for still in facts.still.iter().filter(|s| s.duration() >= idle_min) {
        // Dead air on a still screen is sure; a still screen under the voice only asks.
        for silent in gaps.inside(*still) {
            if silent.duration() >= idle_min {
                found.push(Finding::new(ElementKind::Idle, silent, 90));
            }
        }
        if gaps.voiced_in(*still) {
            found.push(Finding::new(ElementKind::Idle, *still, 50));
        }
    }
    found
}

/// The unvoiced stretches of a take between its words.
pub struct Gaps {
    voiced: Vec<TimeRange>,
    gaps: Vec<Gap>,
}

pub(crate) struct Gap {
    pub(crate) range: TimeRange,
    /// The last word before does not end a sentence.
    pub(crate) mid_sentence: bool,
    /// The VAD hears speech without a recognized word: maybe a missed word, maybe a breath.
    murmur: bool,
}

impl Gap {
    /// How sure it is that the gap is dead air: a silence after a sentence is, a pause mid
    /// sentence grows surer the longer it runs past `min`, and a murmur may be a missed word.
    pub(crate) fn confidence(&self, min: Micros) -> u8 {
        match (self.mid_sentence, self.murmur) {
            (false, false) => 95,
            (false, true) => 60,
            (true, false) => {
                let over = self.range.duration().0 as f64 / min.0 as f64 - 1.0;
                (60.0 + 30.0 * over).clamp(60.0, 90.0) as u8
            }
            (true, true) => 50,
        }
    }
}

impl Gaps {
    pub fn of(facts: &TakeFacts) -> Self {
        let words: Vec<W> = facts
            .transcript
            .words()
            .iter()
            .map(|word| W {
                word,
                text: String::new(),
            })
            .collect();
        Self::new(facts, &words)
    }

    fn new(facts: &TakeFacts, words: &[W]) -> Self {
        // A word voices the speech range it touches; speech without a word is not voice.
        let mut voiced: Vec<TimeRange> = words.iter().map(|w| w.word.range).collect();
        let near = |s: &TimeRange| {
            let first = words.partition_point(|w| w.to().0 + SPEECH_PAD <= s.from().0);
            words
                .get(first)
                .is_some_and(|w| w.from().0 < s.to().0 + SPEECH_PAD)
        };
        let mut murmurs = Vec::new();
        for s in &facts.speech.ranges {
            if near(s) {
                voiced.push(*s);
            } else {
                murmurs.push(*s);
            }
        }
        voiced.sort_by_key(|r| r.from());
        let mut merged: Vec<TimeRange> = Vec::with_capacity(voiced.len());
        for range in voiced {
            match merged.last_mut() {
                Some(last) if range.from() <= last.to() => {
                    *last = TimeRange::new(last.from(), last.to().max(range.to()))
                        .expect("merged ranges are not empty");
                }
                _ => merged.push(range),
            }
        }
        let end = facts
            .duration
            .max(merged.last().map_or(Micros(0), |r| r.to()));
        let mut gaps = Vec::new();
        let mut at = Micros(0);
        for (i, voice) in merged
            .iter()
            .chain([&TimeRange::new(end, Micros(end.0 + 1)).expect("one microsecond")])
            .enumerate()
        {
            let last = i == merged.len();
            if let Some(range) = TimeRange::new(at, voice.from()) {
                let before = words
                    .partition_point(|w| w.to() <= range.from())
                    .checked_sub(1)
                    .map(|i| &words[i]);
                gaps.push(Gap {
                    range,
                    mid_sentence: i > 0 && !last && before.is_some_and(|w| !w.ends_sentence()),
                    murmur: murmurs.iter().any(|m: &TimeRange| m.overlaps(range)),
                });
            }
            at = voice.to();
        }
        Self {
            voiced: merged,
            gaps,
        }
    }

    /// Every unvoiced stretch of the take, in order.
    pub(crate) fn all(&self) -> &[Gap] {
        &self.gaps
    }

    fn voiced_in(&self, range: TimeRange) -> bool {
        self.voiced.iter().any(|v| v.overlaps(range))
    }

    /// The unvoiced parts of `range`.
    fn inside(&self, range: TimeRange) -> impl Iterator<Item = TimeRange> + '_ {
        self.gaps
            .iter()
            .filter_map(move |gap| gap.range.intersect(range))
    }
}

impl Finding {
    fn new(kind: ElementKind, range: TimeRange, confidence: u8) -> Self {
        Self {
            kind,
            range,
            confidence,
        }
    }
}

/// From word `from` to word `to`: the words in between go, and the cut lands just before the
/// kept word begins, see [`cut_before`]. With no word left at `to` it ends with the last one.
fn span(words: &[&W], from: usize, to: usize) -> Option<TimeRange> {
    let before = |i: usize| i.checked_sub(1).map(|i| words[i].word.range);
    let end = match words.get(to) {
        Some(word) => cut_before(before(to), word.word.range),
        None => words.last()?.to(),
    };
    TimeRange::new(cut_before(before(from), words[from].word.range), end)
}

/// Every filler phrase said in `words` with its base confidence and where it is.
fn said_fillers(words: &[W]) -> Vec<(&'static [&'static str], u8, TimeRange)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let hit = FILLERS.iter().find(|(phrase, _)| {
            words.len() >= i + phrase.len()
                && phrase.iter().zip(&words[i..]).all(|(p, w)| *p == w.text)
        });
        let Some((phrase, base)) = hit else {
            i += 1;
            continue;
        };
        let last = &words[i + phrase.len() - 1];
        if let Some(range) = TimeRange::new(words[i].from(), last.to()) {
            found.push((*phrase, *base, range));
        }
        i += phrase.len();
    }
    found
}

fn fillers(words: &[W], voice: &Voice) -> Vec<Finding> {
    said_fillers(words)
        .into_iter()
        .filter_map(|(phrase, base, range)| {
            let confidence = voice.confidence(&phrase.join(" "), base)?;
            Some(Finding::new(ElementKind::Filler, range, confidence))
        })
        .collect()
}

/// The filler phrase of `transcript` that starts nearest to `at`, at most `within` away.
pub(crate) fn filler_at(transcript: &Transcript, at: Micros, within: Micros) -> Option<String> {
    let words: Vec<W> = transcript
        .words()
        .iter()
        .map(|word| W {
            word,
            text: normalize(&word.text),
        })
        .filter(|w| !w.text.is_empty())
        .collect();
    said_fillers(&words)
        .into_iter()
        .map(|(phrase, _, range)| (range.from().0.abs_diff(at.0), phrase))
        .filter(|(off, _)| *off <= within.0)
        .min_by_key(|(off, _)| *off)
        .map(|(_, phrase)| phrase.join(" "))
}

/// The retakes in `fluent`, the words without the fillers that will go.
fn retakes(fluent: &[&W]) -> Vec<Finding> {
    let cued = cues(fluent);
    let mut found = repeats(fluent);
    found.extend(restarts(fluent, &cued));
    found.extend(cued);
    found
}

/// One or two words said twice in a row ("the the", "the first, the first thing"): the first
/// copy goes. Longer runs are [`restarts`]. Across a sentence end the words belong to two
/// thoughts ("in the cut. Cut the silence").
fn repeats(words: &[&W]) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let hit = (1..=2).rev().find(|&k| {
            i + 2 * k <= words.len()
                && (0..k).all(|j| words[i + j].text == words[i + k + j].text)
                && !words[i..i + k].iter().any(|w| w.ends_sentence())
                && words[i + k].from().0 < words[i + k - 1].to().0 + REPEAT_GAP.0
        });
        let Some(k) = hit else {
            i += 1;
            continue;
        };
        let text = words[i].text.as_str();
        let confidence = match k {
            1 if DOUBLES.contains(&text) => 50,
            1 if FUNCTION_WORDS.contains(&text) => 85,
            1 => 60,
            _ => 92,
        };
        if let Some(range) = span(words, i, i + k) {
            found.push(Finding::new(ElementKind::Retake, range, confidence));
        }
        i += k;
    }
    found
}

/// How the words from one position say a passage again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Alignment {
    /// Words said in both, the same or sounding alike.
    same: usize,
    /// Words added, dropped or changed between them.
    changed: usize,
    /// Where the aligned part of the first passage ends.
    a_end: usize,
    /// Where the aligned part of the second passage ends.
    b_end: usize,
}

/// How many changes an alignment of `same` shared words may hold.
fn allowed_changes(same: usize) -> usize {
    match same {
        0..=4 => 0,
        5..=7 => 1,
        8..=11 => 2,
        _ => MAX_CHANGES,
    }
}

/// Lines up the passage `a` with the words from `b`, both from their first word, and returns the
/// longest alignment that ends on a shared word and holds no more changes than its length
/// allows. Words compare by sound, so an added, dropped or changed word does not hide a repeat.
fn align(words: &[&W], a: Range<usize>, b: usize) -> Option<Alignment> {
    let first = &words[a.clone()];
    let second = &words[b..words.len().min(b + first.len() + MAX_CHANGES)];
    let change = |cell: Option<(usize, usize)>| {
        cell.map(|(changed, same)| (changed + 1, same))
            .filter(|(changed, _)| *changed <= MAX_CHANGES)
    };
    // Per word of `second`: the fewest changes so far, then the most shared words.
    let mut row: Vec<Option<(usize, usize)>> = (0..=second.len())
        .map(|y| (y <= MAX_CHANGES).then_some((y, 0)))
        .collect();
    let mut best: Option<Alignment> = None;
    for (x, word) in first.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = (x < MAX_CHANGES).then_some((x + 1, 0));
        for y in 1..=second.len() {
            let above = row[y];
            let shared = diagonal
                .filter(|_| x.abs_diff(y - 1) <= MAX_CHANGES)
                .filter(|_| similarity(&word.text, &second[y - 1].text) >= SOUNDS_ALIKE)
                .map(|(changed, same)| (changed, same + 1));
            if let Some((changed, same)) = shared
                && changed <= allowed_changes(same)
                && best.is_none_or(|b| (same, Reverse(changed)) > (b.same, Reverse(b.changed)))
            {
                best = Some(Alignment {
                    same,
                    changed,
                    a_end: a.start + x + 1,
                    b_end: b + y,
                });
            }
            row[y] = [shared, change(diagonal), change(above), change(row[y - 1])]
                .into_iter()
                .flatten()
                .min_by_key(|&(changed, same)| (changed, Reverse(same)));
            diagonal = above;
        }
        if row.iter().all(Option::is_none) {
            break;
        }
    }
    best
}

/// Where a passage repeated from word `i` starts: the one or two words before it go along when
/// they open its sentence ("Now the second change"), but never reach back past `floor`.
fn passage_start(words: &[&W], i: usize, floor: usize) -> usize {
    (floor.max(i.saturating_sub(MAX_LEADING))..=i)
        .rev()
        .find(|&k| k == 0 || words[k - 1].ends_sentence())
        .unwrap_or(i)
}

/// How sure it is that the words from `b` start over what `aligned` matched, `None` when they
/// do not. The words between the two attempts go with the first: the rest of its sentence and
/// at most one short sentence. A long shared run with no or one changed word is sure, and the
/// different ending of an abandoned attempt counts as that one change.
fn restart_confidence(words: &[&W], aligned: Alignment, b: usize) -> Option<u8> {
    let Alignment {
        same,
        changed,
        a_end,
        b_end,
    } = aligned;
    let between = &words[a_end..b];
    // Whisper may break one short sentence up ("It is. Hmm. Wait."), so the aside is counted whole.
    let rest = between
        .iter()
        .position(|w| w.ends_sentence())
        .map_or(between.len(), |end| end + 1);
    if rest > MAX_ASIDE || between.len() - rest > MAX_ASIDE {
        return None;
    }
    if words[b].from().0 > words[a_end - 1].to().0 + RESTART_WITHIN.0 {
        return None;
    }
    let directly = between.is_empty();
    if same < if directly { 3 } else { 5 } {
        return None;
    }
    let changes = changed + usize::from(!directly);
    let goes_on = b_end < words.len() && !words[b_end - 1].ends_sentence();
    Some(match changes {
        0 if goes_on => 92,
        0 => 85,
        1 if same >= 6 => 85,
        2 if same >= 10 => 85,
        _ => 70,
    })
}

/// A passage said again within [`RESTART_WITHIN`]: the earlier attempt goes, the later one is
/// usually the clean version. Passages a cue already answers are left to [`cues`].
fn restarts(words: &[&W], cued: &[Finding]) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut floor = 0;
    let mut i = 0;
    while i < words.len() {
        let hit = (i + 1..words.len().min(i + MAX_ATTEMPT))
            .rev()
            .filter(|&b| words[b].text == words[i].text)
            .filter_map(|b| {
                let aligned = align(words, i..b, b)?;
                let confidence = restart_confidence(words, aligned, b)?;
                // The second attempt keeps the words that open its sentence, as the first
                // loses its own: "This parser reads. The parser reads" keeps "The".
                let kept = passage_start(words, b, aligned.a_end);
                let range = span(words, passage_start(words, i, floor), kept)?;
                let free = !cued.iter().any(|c| c.range.overlaps(range));
                free.then_some((aligned.same, b, kept, range, confidence))
            })
            .max_by_key(|(same, ..)| *same);
        match hit {
            Some((_, b, kept, range, confidence)) => {
                found.push(Finding::new(ElementKind::Retake, range, confidence));
                floor = kept;
                i = b;
            }
            None => i += 1,
        }
    }
    found
}

/// Where a cue that starts at word `c` ends: a phrase of [`SAY_AGAIN`], maybe after a correction
/// ("Sorry, let me say that again"), or a correction or [`ALONE`] phrase as its own sentence
/// after a full sentence.
fn cue_end(words: &[&W], c: usize) -> Option<usize> {
    let says = |phrase: &[&str], at: usize| {
        words.len() >= at + phrase.len()
            && phrase.iter().zip(&words[at..]).all(|(p, w)| *p == w.text)
    };
    let lead = CORRECTIONS
        .iter()
        .find(|p| says(p, c))
        .map_or(0, |p| p.len());
    if let Some(phrase) = SAY_AGAIN.iter().find(|p| says(p, c + lead)) {
        return Some(c + lead + phrase.len());
    }
    let after_sentence = c > 0 && words[c - 1].ends_sentence();
    ALONE
        .iter()
        .chain(CORRECTIONS)
        .find(|p| after_sentence && says(p, c) && words[c + p.len() - 1].ends_sentence())
        .map(|p| c + p.len())
}

/// A spoken cue to start over: the cue goes, with the passage before it that the words after it
/// say again. When nothing after the cue repeats what came before, only the cue goes, and that
/// asks.
fn cues(words: &[&W]) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut c = 0;
    while c < words.len() {
        let Some(end) = cue_end(words, c) else {
            c += 1;
            continue;
        };
        let starts: Vec<usize> = (0..c)
            .rev()
            .take_while(|&i| words[c].from().0 <= words[i].to().0 + RESTART_WITHIN.0)
            .collect();
        let repeat = (end..words.len().min(end + MAX_CUE_LEAD_IN + 1))
            .rev()
            .flat_map(|b| starts.iter().map(move |&i| (i, b)))
            .filter(|&(i, b)| words[i].text == words[b].text)
            .filter_map(|(i, b)| Some((i, b, align(words, i..c, b)?.same)))
            .filter(|&(_, _, same)| same >= 3)
            .max_by_key(|&(_, _, same)| same);
        let finding = match repeat {
            Some((i, b, _)) => {
                // The cue's end opens the restart as a sentence end does, so the words of a
                // short lead-in stay: "say that again, the parser reads" keeps "the".
                let kept = (end.max(b.saturating_sub(MAX_LEADING))..=b)
                    .rev()
                    .find(|&k| k == end || words[k - 1].ends_sentence())
                    .unwrap_or(b);
                span(words, passage_start(words, i, 0), kept).map(|r| (r, 90))
            }
            None => span(words, c, end).map(|r| (r, 70)),
        };
        if let Some((range, confidence)) = finding {
            found.push(Finding::new(ElementKind::Retake, range, confidence));
        }
        c = end;
    }
    found
}

/// An `again` marker repeats the attempt before it: from where those words were said before to
/// where they start again. Without a match, the sentence before the marker is only a guess.
fn again(words: &[W], markers: &[Micros]) -> Vec<Finding> {
    let all: Vec<&W> = words.iter().collect();
    let mut found = Vec::new();
    for &marker in markers {
        let next = all.partition_point(|w| w.from() < marker);
        if next + 3 > all.len() {
            continue;
        }
        let earliest = Micros(marker.0.saturating_sub(AGAIN_LOOKBACK.0));
        let earlier = (0..next)
            .rev()
            .take_while(|&i| all[i].from() >= earliest)
            .find(|&i| i + 3 <= next && (0..3).all(|j| all[i + j].text == all[next + j].text));
        let (from, confidence) = match earlier {
            Some(i) => (i, 95),
            None => {
                let Some(last) = next.checked_sub(1) else {
                    continue;
                };
                let start = (0..last)
                    .rev()
                    .find(|&i| all[i].ends_sentence())
                    .map_or(0, |i| i + 1);
                (start, 60)
            }
        };
        if let Some(range) = span(&all, from, next) {
            found.push(Finding::new(ElementKind::Retake, range, confidence));
        }
    }
    found
}

/// A correction phrase after words it replaces ("thirteen fifty-three, sorry, thirteen
/// thirty-five"): the replaced words and the phrase go. The replaced words are the one to four
/// words before the phrase that best match as many words after it.
fn corrections(words: &[W]) -> Vec<Finding> {
    let all: Vec<&W> = words.iter().collect();
    let mut found = Vec::new();
    for c in 0..all.len() {
        if cue_end(&all, c).is_some() {
            continue;
        }
        let Some(cue) = CORRECTIONS.iter().find(|cue| {
            all.len() > c + cue.len() && cue.iter().zip(&all[c..]).all(|(p, w)| *p == w.text)
        }) else {
            continue;
        };
        let repair = c + cue.len();
        let best = (1..=4usize)
            .filter(|&n| n <= c && repair + n <= all.len())
            .filter(|&n| !all[c - n..c - 1].iter().any(|w| w.ends_sentence()))
            .map(|n| {
                let score = (0..n)
                    .map(|j| similarity(&all[c - n + j].text, &all[repair + j].text))
                    .sum::<f64>()
                    / n as f64;
                (n, score)
            })
            .fold(None, |best: Option<(usize, f64)>, (n, score)| match best {
                Some((_, s)) if s >= score => best,
                _ => Some((n, score)),
            });
        let Some((n, score)) = best.filter(|(_, score)| *score >= 0.5) else {
            continue;
        };
        let confidence = if score >= 0.75 { 90 } else { 70 };
        if let Some(range) = span(&all, c - n, repair) {
            found.push(Finding::new(ElementKind::Misspoken, range, confidence));
        }
    }
    found
}

/// How alike two normalized words are, 0 to 1: equal, both numbers, or their shared letter
/// pairs.
fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let numeric = |s: &str| s.chars().any(|c| c.is_ascii_digit());
    let pairs = |s: &str| -> Vec<(char, char)> {
        let chars: Vec<char> = s.chars().collect();
        chars.windows(2).map(|p| (p[0], p[1])).collect()
    };
    let (pa, mut pb) = (pairs(a), pairs(b));
    let total = pa.len() + pb.len();
    let mut shared = 0;
    for pair in &pa {
        if let Some(at) = pb.iter().position(|p| p == pair) {
            pb.swap_remove(at);
            shared += 1;
        }
    }
    let dice = if total == 0 {
        0.0
    } else {
        2.0 * shared as f64 / total as f64
    };
    if numeric(a) && numeric(b) {
        dice.max(0.8)
    } else {
        dice
    }
}

/// An inline retake heard while recording: the words that go and the clean version after them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeardRetake {
    pub range: TimeRange,
    pub heard: String,
    pub kept: String,
}

/// The retakes in `words`, the words said so far, that the retake rule is at least
/// `confidence` sure of. Only those whose clean version has started are found, so the list
/// only grows as words arrive.
pub fn heard_retakes(words: &[Word], confidence: u8) -> Vec<HeardRetake> {
    let words: Vec<W> = words
        .iter()
        .map(|word| W {
            word,
            text: normalize(&word.text),
        })
        .filter(|w| !w.text.is_empty())
        .collect();
    let fluent: Vec<&W> = words.iter().collect();
    let mut found = retakes(&fluent);
    found.retain(|f| f.confidence >= confidence);
    found.sort_by_key(|f| f.range.from());
    let said = |range: TimeRange, skip: usize, take: usize| {
        words
            .iter()
            .filter(|w| w.word.range.to() > range.from())
            .skip(skip)
            .take(take)
            .map(|w| w.word.text.trim())
            .collect::<Vec<_>>()
            .join(" ")
    };
    found
        .into_iter()
        .map(|f| {
            let count = words
                .iter()
                .filter(|w| f.range.overlaps(w.word.range))
                .count();
            HeardRetake {
                range: f.range,
                heard: said(f.range, 0, count),
                kept: said(f.range, count, count),
            }
        })
        .collect()
}

/// An emphasis holds at most this long.
const MAX_EMPHASIS: Micros = Micros(3_000_000);
/// A word this much louder than the take's median word is emphasis.
pub(crate) const EMPHASIS_DB: f32 = 6.0;
const MIN_EMPHASIS_WORD: Micros = Micros(200_000);

/// A word said clearly louder than the take's typical word.
pub(crate) struct Loud {
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
    use crate::Speech;
    use shot_core::Id;

    /// Words at 0.3 s each, back to back, with `|` as a 1.5 s pause.
    fn transcript(text: &str) -> Transcript {
        let mut transcript = Transcript::new(Id::new("t01").unwrap());
        let mut at = 0;
        for token in text.split(' ') {
            if token == "|" {
                at += 1_500_000;
                continue;
            }
            transcript
                .push(Word {
                    text: token.to_owned(),
                    range: TimeRange::new(Micros(at), Micros(at + 300_000)).unwrap(),
                    confidence: 90,
                })
                .unwrap();
            at += 300_000;
        }
        transcript
    }

    fn run(transcript: &Transcript, still: &[TimeRange]) -> Vec<(ElementKind, String, u8)> {
        let speech = Speech::default();
        let facts = TakeFacts {
            transcript,
            speech: &speech,
            still,
            again: &[],
            duration: transcript.words().last().unwrap().range.to(),
        };
        let words = transcript.words();
        find(&facts, &Voice::default(), 85, Micros(3_000_000))
            .into_iter()
            .map(|f| {
                let text: Vec<&str> = words
                    .iter()
                    .filter(|w| {
                        f.range
                            .intersect(w.range)
                            .is_some_and(|r| r.duration().0 * 2 > w.range.duration().0)
                    })
                    .map(|w| w.text.as_str())
                    .collect();
                (f.kind, text.join(" "), f.confidence)
            })
            .collect()
    }

    #[test]
    fn a_restarted_sentence_cuts_the_first_attempt() {
        let t = transcript(
            "Welcome back. The first thing we do is create a new project. The first thing we do is create a new project from the tutorial preset.",
        );
        assert_eq!(
            run(&t, &[]),
            [(
                ElementKind::Retake,
                "The first thing we do is create a new project.".into(),
                92
            )]
        );
    }

    #[test]
    fn a_restart_keeps_the_word_that_opens_the_second_attempt() {
        let t = transcript(
            "This parser reads our local project files today. The parser reads our local project files today.",
        );
        assert_eq!(
            run(&t, &[]),
            [(
                ElementKind::Retake,
                "This parser reads our local project files today.".into(),
                85
            )]
        );
    }

    #[test]
    fn a_restart_that_corrects_two_words_still_cuts_the_first_attempt() {
        let t = transcript(
            "Your Stripe dashboard does. February, while the stocks were failing, was the most. February, while the stocks were falling, was the best month for growth in that data.",
        );
        assert_eq!(
            run(&t, &[]),
            [(
                ElementKind::Retake,
                "February, while the stocks were failing, was the most.".into(),
                85
            )]
        );
    }

    #[test]
    fn sentences_that_only_open_alike_are_not_a_restart() {
        let t = transcript("We need to ship this week. We need to test what we ship first.");
        assert_eq!(run(&t, &[]), []);
    }

    #[test]
    fn a_retake_is_heard_once_its_clean_version_has_started() {
        let words = |text: &str| transcript(text).words().to_vec();
        let said = "Welcome back. The first thing we do is create a new project.";
        assert!(heard_retakes(&words(said), 85).is_empty());
        let again = format!("{said} The first thing we do is create a new project from");
        assert_eq!(
            heard_retakes(&words(&again), 85)
                .into_iter()
                .map(|r| (r.heard, r.kept))
                .collect::<Vec<_>>(),
            [(
                "The first thing we do is create a new project.".to_owned(),
                "The first thing we do is create a new project".to_owned()
            )]
        );
    }

    #[test]
    fn a_self_correction_cuts_the_wrong_words_and_the_cue() {
        let t = transcript("the build ran at 13.53, sorry, 13.35, and it finished in two seconds.");
        assert_eq!(
            run(&t, &[]),
            [(ElementKind::Misspoken, "13.53, sorry,".into(), 90)]
        );
    }

    #[test]
    fn fillers_and_the_stumble_they_hide() {
        let t = transcript("we kind of check the, um, the settings.");
        assert_eq!(
            run(&t, &[]),
            [
                (ElementKind::Filler, "kind of".into(), 40),
                (ElementKind::Filler, "um,".into(), 92),
                (ElementKind::Retake, "the, um,".into(), 85),
            ]
        );
    }

    #[test]
    fn a_pause_mid_sentence_is_less_sure_than_a_silence_after_one() {
        let t = transcript("The renderer only redraws | the chapters. | That was it.");
        let speech = Speech::default();
        let facts = TakeFacts {
            transcript: &t,
            speech: &speech,
            still: &[],
            again: &[],
            duration: t.words().last().unwrap().range.to(),
        };
        let gaps: Vec<_> = Gaps::of(&facts)
            .all()
            .iter()
            .map(|g| (g.mid_sentence, g.confidence(Micros(1_000_000))))
            .collect();
        assert_eq!(gaps, [(true, 75), (false, 95)]);
    }

    #[test]
    fn kept_fillers_become_part_of_the_voice() {
        let mut voice = Voice::default();
        voice.kept.insert("basically".into(), 1);
        assert_eq!(voice.confidence("basically", 50), Some(30));
        voice.kept.insert("basically".into(), 3);
        assert_eq!(voice.confidence("basically", 50), None);
    }

    /// The review's take as Whisper wrote it.
    const ONE_TAKE: &str = "So today we look at the new router in version 12. Um, the first thing you notice is, uh, that the config file is gone. The the roots now live next to the the pages. This makes the project a lot easier to, this makes the project a lot easier to read, because every page owns its root. Now the second change is the loader, it runs on the server and it blocks. Sorry, let me say that again. The second big change is the loader. It runs on the server before the page renders. I think, you know, this is basically the best release in years. Performance is very, very good. The build ran in 13.53. Sorry, 13.35 milliseconds. That is the whole update. See you next week.";

    fn retakes_of(text: &str) -> Vec<(String, u8)> {
        run(&transcript(text), &[])
            .into_iter()
            .filter(|f| f.0 == ElementKind::Retake)
            .map(|f| (f.1, f.2))
            .collect()
    }

    #[test]
    fn a_restart_needs_no_full_stop_and_has_no_length_cap() {
        let found = retakes_of(ONE_TAKE);
        assert!(
            found.contains(&("This makes the project a lot easier to,".into(), 92)),
            "{found:?}"
        );
    }

    #[test]
    fn a_reworded_restart_is_found_by_aligning_its_words() {
        assert_eq!(
            retakes_of(
                "Now the second change is the loader, it runs on the server and it blocks. The second big change is the loader. It runs on the server before the page renders."
            ),
            [(
                "Now the second change is the loader, it runs on the server and it blocks.".into(),
                85
            )]
        );
    }

    #[test]
    fn parallel_sentences_with_two_changes_only_ask() {
        assert_eq!(
            retakes_of(
                "We look at the new router in version twelve. We look at the old router in version eleven."
            ),
            [("We look at the new router in version twelve.".into(), 70)]
        );
    }

    #[test]
    fn a_short_sentence_between_two_attempts_goes_with_the_first() {
        assert_eq!(
            retakes_of(
                "Clap gives us help text and validation for free. It is hmm wait. Clap gives us help text and validation for free."
            ),
            [(
                "Clap gives us help text and validation for free. It is hmm wait.".into(),
                85
            )]
        );
        assert_eq!(
            retakes_of(
                "Clap gives us help text and validation for free. It is. Well. Wait. Clap gives us help text and validation for free."
            ),
            [(
                "Clap gives us help text and validation for free. It is. Well. Wait.".into(),
                85
            )]
        );
    }

    #[test]
    fn a_sure_filler_does_not_hide_a_restart() {
        assert_eq!(
            retakes_of("The, um, first thing we do. The first thing we do is run it."),
            [("The, um, first thing we do.".into(), 92)]
        );
    }

    #[test]
    fn a_cue_goes_with_the_passage_the_words_after_it_repeat() {
        let found = retakes_of(ONE_TAKE);
        assert!(
            found.contains(&(
                "Now the second change is the loader, it runs on the server and it blocks. Sorry, let me say that again."
                    .into(),
                90
            )),
            "{found:?}"
        );
    }

    #[test]
    fn a_comma_after_a_cue_keeps_the_word_that_opens_the_restart() {
        assert_eq!(
            retakes_of(
                "This parser reads local project files quickly. Sorry, let me say that again, the parser reads local project files quickly."
            ),
            [(
                "This parser reads local project files quickly. Sorry, let me say that again,"
                    .into(),
                90
            )]
        );
    }

    #[test]
    fn a_cue_nothing_repeats_goes_alone_and_asks() {
        assert_eq!(
            retakes_of("We ship on Friday. Let me say that again. Tests are not optional here."),
            [("Let me say that again.".into(), 70)]
        );
        assert_eq!(
            retakes_of("We ship on Friday. Again. Tests are not optional here."),
            [("Again.".into(), 70)]
        );
    }

    #[test]
    fn cue_words_inside_a_sentence_are_no_cue() {
        assert_eq!(retakes_of("Click it one more time and run it again."), []);
    }

    #[test]
    fn only_doubled_function_words_are_sure() {
        assert_eq!(
            retakes_of("We we ship to to the server."),
            [("We".into(), 85), ("to".into(), 85)]
        );
        assert_eq!(
            retakes_of("Performance is very, very good."),
            [("very,".into(), 60)]
        );
    }

    #[test]
    fn a_repeat_across_a_sentence_end_is_no_finding() {
        assert_eq!(
            retakes_of("We keep it in the cut. Cut the silence next."),
            []
        );
    }

    #[test]
    fn the_reviewed_take_gives_each_stumble_once() {
        assert_eq!(
            run(&transcript(ONE_TAKE), &[])
                .into_iter()
                .filter(|f| f.2 >= 60 && f.0 != ElementKind::Filler)
                .map(|f| (f.0, f.1, f.2))
                .collect::<Vec<_>>(),
            [
                (ElementKind::Misspoken, "13.53. Sorry,".to_owned(), 90),
                (ElementKind::Retake, "The".to_owned(), 85),
                (ElementKind::Retake, "the".to_owned(), 85),
                (ElementKind::Retake, "very,".to_owned(), 60),
                (
                    ElementKind::Retake,
                    "This makes the project a lot easier to,".to_owned(),
                    92
                ),
                (
                    ElementKind::Retake,
                    "Now the second change is the loader, it runs on the server and it blocks. Sorry, let me say that again."
                        .to_owned(),
                    90
                ),
            ]
        );
    }

    #[test]
    fn a_silent_still_screen_is_idle() {
        let t = transcript("Let me open it. | | | | Here it is.");
        let still = [TimeRange::new(Micros(1_300_000), Micros(7_000_000)).unwrap()];
        let found = run(&t, &still);
        assert!(found.iter().any(|f| f.0 == ElementKind::Idle && f.2 == 90));
    }
}
