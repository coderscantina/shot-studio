//! The rough cut: auto-cut rules and pacing written into `cut.kdl`.
//!
//! Rules work like a linter with auto-fix. Each finding has a confidence; one at or above the
//! format's `auto-cut confidence` becomes `remove ... by=<rule>`, the rest wait in the review
//! queue as `review ... by=<rule>` ([0013](../../../docs/decisions/0013-review-queue-in-cut-kdl.md)).
//! A finding that overlaps a `remove`, `review` or `ignore` of its rule is skipped, so a run
//! never asks twice. A rule switched off gives back what it removed and drops its open questions.
//! The pacing places the edges of silence and pause cuts next to the voice, and a pacing change
//! does nothing else, so switching back restores the cut. It also writes the punch-in zooms
//! ([0014](../../../docs/decisions/0014-auto-edits-as-written-nodes.md)).

mod edit;
pub(crate) mod rules;
pub(crate) mod zoom;

use std::collections::HashMap;
use std::path::Path;

use shot_core::{
    Cut, Format, Id, Layout, Micros, Pacing, RangeEdit, RemovedBy, RuleKind, TakeEdit, TimeRange,
    Transcript,
};

pub use rules::{HeardRetake, Voice, heard_retakes};
pub use zoom::PUNCH_IN;

use crate::{Error, Speech, again_markers, idle, normalize};

/// What the rules know about one take, all in its media time.
pub struct TakeFacts<'a> {
    pub transcript: &'a Transcript,
    pub speech: &'a Speech,
    /// Stretches where the screen does not change, empty without a screen track.
    pub still: &'a [TimeRange],
    /// Times of the `again` markers in the event log.
    pub again: &'a [Micros],
    pub duration: Micros,
}

/// The facts the rules read from a take folder, `media/<take>/`: the still screen of
/// `screen.mp4` and the `again` markers of `events.jsonl`. Either file may be missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TakeFiles {
    pub still: Vec<TimeRange>,
    pub again: Vec<Micros>,
}

impl TakeFiles {
    /// Reads them, with still stretches of at least `idle_min`.
    pub fn read(take_dir: &Path, idle_min: Micros) -> Result<Self, Error> {
        let screen = take_dir.join("screen.mp4");
        let still = if screen.exists() {
            let (frames, pixels) = idle::encoded_frames(&screen)?;
            idle::still_ranges(&frames, pixels, idle_min)
        } else {
            Vec::new()
        };
        Ok(Self {
            still,
            again: again_markers(&take_dir.join("events.jsonl"))?,
        })
    }
}

/// One thing a rule found, with how sure it is in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    pub rule: RuleKind,
    pub range: TimeRange,
    pub confidence: u8,
}

/// Runs the rules of `format` over every block of `facts.transcript.take()` in `cut` and returns
/// the new cut. `transcripts` holds the cut's other takes, whose decisions teach which fillers
/// belong to the voice.
pub fn rough_cut(
    cut: &Cut,
    format: &Format,
    facts: &TakeFacts,
    transcripts: &HashMap<Id, Transcript>,
) -> Cut {
    rerun(cut, format, facts, transcripts, Change::All)
}

/// What changed in the format since the rules last ran over a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A new take, or no telling: every rule runs.
    All,
    /// The pacing was this one before. Only the edges of gap cuts move, by the difference in
    /// breath, so switching back restores the cut.
    Pacing(Pacing),
    /// This rule was switched on, so only it runs.
    RuleOn(RuleKind),
    /// A rule was switched off, so it gives back what it cut.
    RuleOff,
}

/// [`rough_cut`] after `change`: brings the cut in line with `format` without running rules the
/// change did not touch.
pub fn rerun(
    cut: &Cut,
    format: &Format,
    facts: &TakeFacts,
    transcripts: &HashMap<Id, Transcript>,
    change: Change,
) -> Cut {
    let take = facts.transcript.take();
    let rules = &format.auto_cut;
    let breath = format.pacing.breath();
    let voice = learn_voice(cut, facts.transcript, transcripts);
    let shows_screen = |t: Micros| {
        blocks_of(cut, take).any(|edit| layout_at(edit, t, format.default_layout).shows_screen())
    };
    let findings: Vec<Finding> = rules::find(rules, breath, facts, &voice, shows_screen)
        .into_iter()
        .filter(|f| match change {
            Change::All => true,
            Change::RuleOn(rule) => f.rule == rule,
            Change::Pacing(_) | Change::RuleOff => false,
        })
        .collect();
    let gaps = rules::Gaps::of(facts);
    let on = |rule: RuleKind| match rule {
        RuleKind::Silence => rules.silence.on,
        RuleKind::Retake => rules.retake,
        RuleKind::Misspoken => rules.misspoken,
        RuleKind::Pause => rules.pause.on,
        RuleKind::Filler => rules.filler,
        RuleKind::IdleScreen => rules.idle_screen.on,
    };

    let mut cut = cut.clone();
    for chapter in &mut cut.chapters {
        for block in chapter.takes.iter_mut().filter(|b| b.take == *take) {
            for &rule in RuleKind::ALL.iter().filter(|r| !on(**r)) {
                edit::strip(block, rule);
            }
            if let Change::Pacing(before) = change {
                repace(block, &gaps, before.breath(), breath);
            }
            let Some(material) = material(block) else {
                continue;
            };
            let inside = findings
                .iter()
                .filter_map(|f| Some((f, f.range.intersect(material)?)));
            let (sure, unsure): (Vec<_>, Vec<_>) =
                inside.partition(|(f, _)| f.confidence >= rules.confidence);
            for (finding, range) in sure {
                if !edit::decided(block, range, finding.rule) {
                    edit::remove(block, range, finding.rule);
                }
            }
            for (finding, range) in unsure {
                if !edit::decided(block, range, finding.rule) {
                    edit::review(block, range, finding.rule);
                }
            }
            zoom::place(block, facts.transcript.words(), facts.speech);
        }
    }
    cut
}

/// What the rules did in a cut, per rule in contract order: removals and their length, and the
/// open review items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleCount {
    pub rule: RuleKind,
    pub removed: usize,
    pub removed_time: Micros,
    pub queued: usize,
}

pub fn rule_counts(cut: &Cut) -> Vec<RuleCount> {
    let mut counts: Vec<RuleCount> = RuleKind::ALL
        .iter()
        .map(|&rule| RuleCount {
            rule,
            removed: 0,
            removed_time: Micros(0),
            queued: 0,
        })
        .collect();
    let index = |rule: RuleKind| {
        RuleKind::ALL
            .iter()
            .position(|r| *r == rule)
            .expect("listed")
    };
    for block in cut.chapters.iter().flat_map(|c| &c.takes) {
        for piece in &block.ranges {
            if let RangeEdit::Remove {
                range,
                by: RemovedBy::Rule(rule),
            } = piece
            {
                let count = &mut counts[index(*rule)];
                count.removed += 1;
                count.removed_time.0 += range.duration().0;
            }
        }
        for review in block.reviews.iter().filter(|r| !r.kept) {
            counts[index(review.by)].queued += 1;
        }
    }
    counts
}

fn blocks_of<'a>(cut: &'a Cut, take: &'a Id) -> impl Iterator<Item = &'a TakeEdit> + 'a {
    cut.chapters
        .iter()
        .flat_map(|c| &c.takes)
        .filter(move |b| b.take == *take)
}

fn layout_at(block: &TakeEdit, t: Micros, default: Layout) -> Layout {
    block
        .layouts
        .iter()
        .find(|c| c.range.from() <= t && t < c.range.to())
        .map_or(default, |c| c.layout)
}

/// The media a block covers, from its first range to its last.
fn material(block: &TakeEdit) -> Option<TimeRange> {
    TimeRange::new(
        block.ranges.first()?.range().from(),
        block.ranges.last()?.range().to(),
    )
}

/// Moves the edges of silence and pause cuts from the breath `before` to `breath`.
fn repace(block: &mut TakeEdit, gaps: &rules::Gaps, before: Micros, breath: Micros) {
    let gap_cuts: Vec<(TimeRange, RuleKind)> = block
        .ranges
        .iter()
        .filter_map(|piece| match *piece {
            RangeEdit::Remove {
                range,
                by: RemovedBy::Rule(rule @ (RuleKind::Silence | RuleKind::Pause)),
            } => Some((range, rule)),
            _ => None,
        })
        .collect();
    for (range, rule) in gap_cuts {
        let Some(placed) = gaps.place(range, before, breath) else {
            continue;
        };
        if placed != range {
            edit::unremove(block, range, rule);
            edit::remove(block, placed, rule);
        }
    }
}

/// Fillers a person kept with K: each `ignore by=filler` counts for the phrase it covers.
fn learn_voice(cut: &Cut, this: &Transcript, others: &HashMap<Id, Transcript>) -> Voice {
    let mut voice = Voice::default();
    for block in cut.chapters.iter().flat_map(|c| &c.takes) {
        let transcript = if block.take == *this.take() {
            Some(this)
        } else {
            others.get(&block.take)
        };
        let Some(transcript) = transcript else {
            continue;
        };
        for kept in block
            .reviews
            .iter()
            .filter(|r| r.kept && r.by == RuleKind::Filler)
        {
            let phrase: Vec<String> = transcript
                .words()
                .iter()
                .filter(|w| w.range.overlaps(kept.range))
                .map(|w| normalize(&w.text))
                .collect();
            if !phrase.is_empty() {
                *voice.kept.entry(phrase.join(" ")).or_default() += 1;
            }
        }
    }
    voice
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Decision, FormatKind, TakeAt, Word};

    fn transcript(text: &str) -> Transcript {
        let mut transcript = Transcript::new(Id::new("t01").unwrap());
        let mut at = 1_000_000;
        for token in text.split(' ') {
            if token == "|" {
                at += 2_000_000;
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

    fn rough(cut: &Cut, format: &Format, transcript: &Transcript) -> Cut {
        after(cut, format, transcript, Change::All)
    }

    fn after(cut: &Cut, format: &Format, transcript: &Transcript, change: Change) -> Cut {
        let speech = Speech::default();
        let facts = TakeFacts {
            transcript,
            speech: &speech,
            still: &[],
            again: &[],
            duration: Micros(20_000_000),
        };
        rerun(cut, format, &facts, &HashMap::new(), change)
    }

    #[test]
    fn confident_cuts_apply_unsure_ones_queue_and_a_rerun_asks_nothing_twice() {
        let t = transcript("So, um, we ship it. | Basically we test it.");
        let format = FormatKind::Opinion.preset();
        let take = Id::new("t01").unwrap();
        let empty = Cut::default().append_take(
            take,
            "Take",
            TimeRange::new(Micros(0), Micros(20_000_000)).unwrap(),
        );
        let cut = rough(&empty, &format, &t);
        let counts = rule_counts(&cut);
        let count = |rule| counts.iter().find(|c| c.rule == rule).unwrap();
        assert_eq!(count(RuleKind::Filler).removed, 1);
        assert_eq!(count(RuleKind::Filler).queued, 1);
        assert_eq!(count(RuleKind::Silence).removed, 3);
        let text = cut.to_kdl();
        assert!(
            text.contains("by=filler") && text.contains("review"),
            "{text}"
        );

        let (at, review) = cut.review_queue().next().unwrap();
        assert_eq!(
            at,
            TakeAt {
                chapter: 0,
                take: 0
            }
        );
        let kept = cut.decide(at, review.range, Decision::Keep).unwrap();
        assert_eq!(rough(&kept, &format, &t), kept);
    }

    #[test]
    fn pacing_moves_the_edges_and_a_rule_switched_off_gives_back() {
        let t = transcript("We ship it. | We test it.");
        let mut format = FormatKind::Opinion.preset();
        let take = Id::new("t01").unwrap();
        let empty = Cut::default().append_take(
            take,
            "Take",
            TimeRange::new(Micros(0), Micros(20_000_000)).unwrap(),
        );
        format.pacing = Pacing::Natural;
        let natural = rough(&empty, &format, &t);
        format.pacing = Pacing::Tight;
        let tight = after(&natural, &format, &t, Change::Pacing(Pacing::Natural));
        let removed = |cut: &Cut| rule_counts(cut)[0].removed_time;
        assert!(removed(&tight) > removed(&natural));
        assert_eq!(tight, rough(&empty, &format, &t));
        format.auto_cut.silence.on = false;
        assert_eq!(after(&tight, &format, &t, Change::RuleOff), empty);
    }

    #[test]
    fn a_pacing_change_only_moves_edges_next_to_the_voice() {
        // Words end at 1.9 s and start again at 9.9 s: one 8 s silence after a sentence.
        let t = transcript("We ship it. | | | | We test it.");
        let mut format = FormatKind::Opinion.preset();
        format.pacing = Pacing::Tight;
        let take = Id::new("t01").unwrap();
        let range = |from: u64, to: u64| TimeRange::new(Micros(from), Micros(to)).unwrap();
        let mut tight = Cut::default().append_take(take, "Take", range(0, 20_000_000));
        // A silence cut a person shortened at its end, and one the rules never wrote.
        tight.chapters[0].takes[0].ranges = vec![
            RangeEdit::Keep(range(0, 1_950_000)),
            RangeEdit::Remove {
                range: range(1_950_000, 5_000_000),
                by: RemovedBy::Rule(RuleKind::Silence),
            },
            RangeEdit::Keep(range(5_000_000, 20_000_000)),
        ];
        let pace = |cut: &Cut, from: Pacing, pacing| {
            let format = Format {
                pacing,
                ..format.clone()
            };
            after(cut, &format, &t, Change::Pacing(from))
        };
        let natural = pace(&tight, Pacing::Tight, Pacing::Natural);
        assert_eq!(
            natural.chapters[0].takes[0].ranges[1],
            RangeEdit::Remove {
                range: range(2_150_000, 5_000_000),
                by: RemovedBy::Rule(RuleKind::Silence),
            }
        );
        assert_eq!(pace(&natural, Pacing::Natural, Pacing::Tight), tight);
    }
}
