//! The auto-cut rules: each turns a take's transcript, speech, screen and markers into findings
//! with a confidence. Nothing here knows the cut.

use std::collections::HashMap;

use shot_core::{AutoCut, Micros, RuleKind, TimeRange, Word};

use super::{Finding, TakeFacts};
use crate::normalize;

/// Words closer than this to a speech range voice it.
const SPEECH_PAD: u64 = 200_000;
/// Repeats further apart than this are not a stumble.
const REPEAT_GAP: Micros = Micros(2_000_000);
/// A sentence restarted within this is a retake.
const RESTART_WITHIN: Micros = Micros(20_000_000);
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

/// Words that double in correct speech ("I know that that works").
const DOUBLES: &[&str] = &["that", "had"];

/// What a person decided about fillers before: kept counts by phrase, from `ignore by=filler`.
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

/// Runs every rule that is on. `breath` is the pacing's silence kept on each side of a gap cut,
/// `shows_screen` whether the layout at a time shows the screen track.
pub fn find(
    rules: &AutoCut,
    breath: Micros,
    facts: &TakeFacts,
    voice: &Voice,
    shows_screen: impl Fn(Micros) -> bool,
) -> Vec<Finding> {
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
    let fillers = if rules.filler {
        fillers(&words, voice)
    } else {
        Vec::new()
    };
    // Fillers first: a stumble around one ("the, um, the") leaves the filler to its own rule.
    let mut found = fillers.clone();
    if rules.misspoken {
        found.extend(corrections(&words));
    }
    if rules.retake {
        // Fillers sit inside stumbles ("the, um, the settings"), so repeats are found without
        // the ones that will go.
        let fluent: Vec<&W> = words
            .iter()
            .filter(|w| {
                !fillers
                    .iter()
                    .any(|f| f.confidence >= rules.confidence && f.range.overlaps(w.word.range))
            })
            .collect();
        found.extend(repeats(&fluent));
        found.extend(restarts(&words));
        found.extend(again(&words, facts.again));
    }
    if rules.idle_screen.on {
        for still in facts
            .still
            .iter()
            .filter(|s| s.duration() >= rules.idle_screen.min)
        {
            // Dead air on a still screen goes; a still screen under the voice only asks, and
            // only where the layout shows it.
            for silent in gaps.inside(*still, breath) {
                if silent.duration() >= rules.idle_screen.min {
                    found.push(Finding::new(RuleKind::IdleScreen, silent, 90));
                }
            }
            if gaps.voiced_in(*still) && shows_screen(still.from()) {
                found.push(Finding::new(RuleKind::IdleScreen, *still, 50));
            }
        }
    }
    for gap in &gaps.gaps {
        let rule = if gap.mid_sentence {
            rules.pause
        } else {
            rules.silence
        };
        if !rule.on || gap.range.duration() < rule.min {
            continue;
        }
        let Some(range) = gap.cut(breath) else {
            continue;
        };
        let confidence = gap.confidence(rule.min);
        let kind = if gap.mid_sentence {
            RuleKind::Pause
        } else {
            RuleKind::Silence
        };
        found.push(Finding::new(kind, range, confidence));
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
    /// Voice before and after, so the cut keeps a breath on that side.
    voiced_before: bool,
    voiced_after: bool,
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

    fn cut(&self, breath: Micros) -> Option<TimeRange> {
        let from = self.range.from().0 + if self.voiced_before { breath.0 } else { 0 };
        let to = self
            .range
            .to()
            .0
            .saturating_sub(if self.voiced_after { breath.0 } else { 0 });
        TimeRange::new(Micros(from), Micros(to))
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

    /// Where the cut of a gap rule's finding goes under another pacing: the gap that holds
    /// `range`, less `breath` on its voiced sides.
    pub fn place(&self, range: TimeRange, breath: Micros) -> Option<TimeRange> {
        let at = self.gaps.partition_point(|g| g.range.to() <= range.from());
        self.gaps
            .get(at)
            .filter(|gap| gap.range.intersect(range) == Some(range))
            .and_then(|gap| gap.cut(breath))
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
                    voiced_before: i > 0,
                    voiced_after: !last,
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

    /// The unvoiced parts of `range`, less the breath next to the voice.
    fn inside(&self, range: TimeRange, breath: Micros) -> impl Iterator<Item = TimeRange> + '_ {
        self.gaps
            .iter()
            .filter_map(move |gap| gap.cut(breath)?.intersect(range))
    }
}

impl Finding {
    fn new(rule: RuleKind, range: TimeRange, confidence: u8) -> Self {
        Self {
            rule,
            range,
            confidence,
        }
    }
}

/// From the start of word `from` to the start of word `to`: the words in between go, and the
/// cut lands where the kept word begins.
fn span(words: &[&W], from: usize, to: usize) -> Option<TimeRange> {
    TimeRange::new(words[from].from(), words.get(to)?.from())
}

fn fillers(words: &[W], voice: &Voice) -> Vec<Finding> {
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
        let key = phrase.join(" ");
        if let Some(confidence) = voice.confidence(&key, *base)
            && let Some(range) = TimeRange::new(words[i].from(), last.to())
        {
            found.push(Finding::new(RuleKind::Filler, range, confidence));
        }
        i += phrase.len();
    }
    found
}

/// A run of one to six words said twice in a row ("the the", "the first thing, the first
/// thing"): the first copy goes.
fn repeats(words: &[&W]) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let hit = (1..=6).rev().find(|&k| {
            i + 2 * k <= words.len()
                && (0..k).all(|j| words[i + j].text == words[i + k + j].text)
                && words[i + k].from().0 < words[i + k - 1].to().0 + REPEAT_GAP.0
        });
        let Some(k) = hit else {
            i += 1;
            continue;
        };
        let confidence = match k {
            1 if DOUBLES.contains(&words[i].text.as_str()) => 50,
            1 => 85,
            _ => 92,
        };
        if let Some(range) = span(words, i, i + k) {
            found.push(Finding::new(RuleKind::Retake, range, confidence));
        }
        i += k;
    }
    found
}

/// A sentence said again from its start, within the next two sentences: the earlier attempt
/// goes, the later one is usually the clean version.
fn restarts(words: &[W]) -> Vec<Finding> {
    let all: Vec<&W> = words.iter().collect();
    let mut sentences = Vec::new();
    let mut start = 0;
    for (i, w) in all.iter().enumerate() {
        if w.ends_sentence() || i + 1 == all.len() {
            sentences.push(start..i + 1);
            start = i + 1;
        }
    }
    let mut found = Vec::new();
    let mut skip_to = 0;
    for (n, a) in sentences.iter().enumerate() {
        if a.start < skip_to || a.len() < 3 {
            continue;
        }
        for b in sentences.iter().skip(n + 1).take(2) {
            if all[b.start].from().0 > all[a.end - 1].to().0 + RESTART_WITHIN.0 || b.len() < a.len()
            {
                break;
            }
            let mismatches = (0..a.len())
                .filter(|&j| all[a.start + j].text != all[b.start + j].text)
                .count();
            let confidence = match mismatches {
                0 if b.len() > a.len() => 92,
                0 => 85,
                1 if a.len() >= 6 => 70,
                _ => continue,
            };
            if let Some(range) = span(&all, a.start, b.start) {
                found.push(Finding::new(RuleKind::Retake, range, confidence));
                skip_to = b.start;
            }
            break;
        }
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
            found.push(Finding::new(RuleKind::Retake, range, confidence));
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
            found.push(Finding::new(RuleKind::Misspoken, range, confidence));
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
    let mut found = repeats(&fluent);
    found.extend(restarts(&words));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Speech;
    use shot_core::{Id, TimedRule, Transcript};

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

    fn rules() -> AutoCut {
        let timed = |min| TimedRule {
            on: true,
            min: Micros(min),
        };
        AutoCut {
            confidence: 85,
            silence: timed(600_000),
            retake: true,
            misspoken: true,
            pause: timed(1_000_000),
            filler: true,
            idle_screen: timed(3_000_000),
        }
    }

    fn run(transcript: &Transcript, still: &[TimeRange]) -> Vec<(RuleKind, String, u8)> {
        let speech = Speech::default();
        let facts = TakeFacts {
            transcript,
            speech: &speech,
            still,
            again: &[],
            duration: transcript.words().last().unwrap().range.to(),
        };
        let words = transcript.words();
        find(&rules(), Micros(250_000), &facts, &Voice::default(), |_| {
            true
        })
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
            (f.rule, text.join(" "), f.confidence)
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
                RuleKind::Retake,
                "The first thing we do is create a new project.".into(),
                92
            )]
        );
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
            [(RuleKind::Misspoken, "13.53, sorry,".into(), 90)]
        );
    }

    #[test]
    fn fillers_and_the_stumble_they_hide() {
        let t = transcript("we kind of check the, um, the settings.");
        assert_eq!(
            run(&t, &[]),
            [
                (RuleKind::Filler, "kind of".into(), 40),
                (RuleKind::Filler, "um,".into(), 92),
                (RuleKind::Retake, "the, um,".into(), 85),
            ]
        );
    }

    #[test]
    fn a_pause_mid_sentence_asks_a_silence_after_one_does_not() {
        let t = transcript("The renderer only redraws | the chapters. | That was it.");
        let found = run(&t, &[]);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].0, found[0].2), (RuleKind::Pause, 75));
        assert_eq!((found[1].0, found[1].2), (RuleKind::Silence, 95));
    }

    #[test]
    fn kept_fillers_become_part_of_the_voice() {
        let mut voice = Voice::default();
        voice.kept.insert("basically".into(), 1);
        assert_eq!(voice.confidence("basically", 50), Some(30));
        voice.kept.insert("basically".into(), 3);
        assert_eq!(voice.confidence("basically", 50), None);
    }

    #[test]
    fn a_silent_still_screen_is_idle() {
        let t = transcript("Let me open it. | | | | Here it is.");
        let still = [TimeRange::new(Micros(1_300_000), Micros(7_000_000)).unwrap()];
        let found = run(&t, &still);
        assert!(
            found
                .iter()
                .any(|f| f.0 == RuleKind::IdleScreen && f.2 == 90)
        );
    }
}
