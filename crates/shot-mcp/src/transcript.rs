//! A video's transcript as text an agent can cut from, and cut proposals made of whole
//! recorded words. A proposal can only remove or restore words that were recorded, so an agent
//! never puts words in the speaker's mouth.

use std::collections::HashMap;
use std::fmt::Write;

use rmcp::schemars;
use shot_core::{Cut, Id, Micros, RangeEdit, RemovedBy, TakeAt, TimeRange, Transcript, Word};

/// Words per line of the transcript text, so word numbers stay easy to count.
const LINE_WORDS: usize = 8;

/// A run of words of one take, by their numbers in the take's transcript, both included.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
pub struct WordSpan {
    /// The take id, for example `t01`.
    pub take: String,
    /// The number of the first word, as `read_transcript` shows it after `w`.
    pub from_word: usize,
    /// The number of the last word, included.
    pub to_word: usize,
}

/// One take block of the cut with the words it covers.
struct Block<'a> {
    at: TakeAt,
    chapter: &'a str,
    take: &'a Id,
    ranges: &'a [RangeEdit],
}

fn blocks(cut: &Cut) -> Vec<Block<'_>> {
    cut.chapters
        .iter()
        .enumerate()
        .flat_map(|(chapter, c)| {
            c.takes.iter().enumerate().map(move |(take, edit)| Block {
                at: TakeAt { chapter, take },
                chapter: &c.title,
                take: &edit.take,
                ranges: &edit.ranges,
            })
        })
        .collect()
}

fn middle(word: &Word) -> u64 {
    (word.range.from().0 + word.range.to().0) / 2
}

/// The transcript of every take block in cut order. Each line starts with the edited time of
/// its first kept word and the number of its first word (`w12`). Removed words are in brackets
/// with what removed them: `[um so](filler)`.
pub fn render(cut: &Cut, transcripts: &HashMap<Id, Transcript>) -> String {
    let mut out = String::new();
    let mut offset = 0u64;
    for block in blocks(cut) {
        let _ = writeln!(
            out,
            "## chapter {} \"{}\", take {}",
            block.at.chapter + 1,
            block.chapter,
            block.take
        );
        let Some(transcript) = transcripts.get(block.take) else {
            out.push_str("(no transcript yet: run `shot transcribe`)\n\n");
            offset += kept(block.ranges);
            continue;
        };
        let mut lines: Vec<Line> = Vec::new();
        let mut kept_before = offset;
        for range in block.ranges {
            let r = range.range();
            for (index, word) in transcript.words().iter().enumerate() {
                let m = middle(word);
                if m < r.from().0 || m >= r.to().0 {
                    continue;
                }
                let (removed, at) = match range {
                    RangeEdit::Keep(_) => (
                        None,
                        Some(kept_before + word.range.from().0.saturating_sub(r.from().0)),
                    ),
                    RangeEdit::Remove { by, .. } => (Some(*by), None),
                };
                match lines.last_mut() {
                    Some(line) if line.words.len() < LINE_WORDS => {
                        line.at = line.at.or(at);
                        line.words.push((word, removed));
                    }
                    _ => lines.push(Line {
                        at,
                        first: index,
                        words: vec![(word, removed)],
                    }),
                }
            }
            if let RangeEdit::Keep(_) = range {
                kept_before += r.duration().0;
            }
        }
        for Line { at, first, words } in lines {
            let time = at.map_or_else(|| "  cut  ".to_owned(), |t| timecode(Micros(t)));
            let _ = write!(out, "{time} w{first} ");
            let mut run: Option<RemovedBy> = None;
            let mut pieces = Vec::new();
            for (word, removed) in words {
                match (run, removed) {
                    (Some(by), Some(now)) if by == now => {
                        pieces.push(word.text.clone());
                        continue;
                    }
                    _ => {}
                }
                if let Some(by) = run.take() {
                    close(&mut pieces, by);
                }
                if let Some(by) = removed {
                    pieces.push(format!("[{}", word.text));
                    run = Some(by);
                } else {
                    pieces.push(word.text.clone());
                }
            }
            if let Some(by) = run {
                close(&mut pieces, by);
            }
            let _ = writeln!(out, "{}", pieces.join(" "));
        }
        out.push('\n');
        offset = kept_before;
    }
    if out.is_empty() {
        out.push_str("the cut has no chapters yet\n");
    }
    out
}

/// Up to [`LINE_WORDS`] words: the edited time of the first kept one and the first's number.
struct Line<'a> {
    at: Option<u64>,
    first: usize,
    words: Vec<(&'a Word, Option<RemovedBy>)>,
}

fn close(pieces: &mut [String], by: RemovedBy) {
    if let Some(last) = pieces.last_mut() {
        let _ = write!(last, "]({})", removed_by(by));
    }
}

fn removed_by(by: RemovedBy) -> &'static str {
    match by {
        RemovedBy::Manual => "manual",
        RemovedBy::Rule(rule) => rule.as_str(),
    }
}

fn kept(ranges: &[RangeEdit]) -> u64 {
    ranges
        .iter()
        .filter_map(|r| match r {
            RangeEdit::Keep(range) => Some(range.duration().0),
            RangeEdit::Remove { .. } => None,
        })
        .sum()
}

/// `m:ss.d` of edited time.
pub fn timecode(t: Micros) -> String {
    let tenths = t.0 / 100_000;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

/// What a proposed cut change does, for the agent's confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub removes: bool,
    pub take: String,
    pub from_word: usize,
    pub to_word: usize,
    pub text: String,
    pub length: Micros,
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} w{}-w{} \"{}\" ({:.1} s)",
            if self.removes { "removes" } else { "restores" },
            self.take,
            self.from_word,
            self.to_word,
            self.text,
            self.length.0 as f64 / 1e6
        )
    }
}

/// Applies word removals, then restores, to `cut`. Each span must lie in one take block.
pub fn apply(
    cut: &Cut,
    transcripts: &HashMap<Id, Transcript>,
    remove: &[WordSpan],
    restore: &[WordSpan],
) -> Result<(Cut, Vec<Change>), String> {
    if remove.is_empty() && restore.is_empty() {
        return Err("name at least one span of words to remove or restore".to_owned());
    }
    let mut cut = cut.clone();
    let mut changes = Vec::new();
    for (span, removes) in remove
        .iter()
        .map(|s| (s, true))
        .chain(restore.iter().map(|s| (s, false)))
    {
        let take = Id::new(span.take.as_str())
            .ok_or_else(|| format!("`{}` is not a take id", span.take))?;
        let transcript = transcripts
            .get(&take)
            .ok_or_else(|| format!("take {take} has no transcript"))?;
        let words = transcript.words();
        if span.from_word > span.to_word || span.to_word >= words.len() {
            return Err(format!(
                "w{}-w{} is not a span of take {take}, which has words w0-w{}",
                span.from_word,
                span.to_word,
                words.len().saturating_sub(1)
            ));
        }
        let (first, last) = (&words[span.from_word], &words[span.to_word]);
        let block_of = |word: &Word| {
            blocks(&cut).into_iter().find_map(|b| {
                (b.take == &take
                    && b.ranges.iter().any(|r| {
                        let r = r.range();
                        r.from().0 <= middle(word) && middle(word) < r.to().0
                    }))
                .then_some(b.at)
            })
        };
        let at = block_of(first)
            .ok_or_else(|| format!("{take} w{} is not in the cut", span.from_word))?;
        if block_of(last) != Some(at) {
            return Err(format!(
                "{take} w{}-w{} crosses a chapter or take block, propose one span per block",
                span.from_word, span.to_word
            ));
        }
        let range = TimeRange::new(first.range.from(), last.range.to())
            .expect("words are ordered and not empty");
        cut = if removes {
            cut.delete_words(at, transcript, range)
        } else {
            cut.restore(at, range)
        }
        .map_err(|e| e.to_string())?;
        changes.push(Change {
            removes,
            take: take.as_str().to_owned(),
            from_word: span.from_word,
            to_word: span.to_word,
            text: words[span.from_word..=span.to_word]
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            length: range.duration(),
        });
    }
    Ok((cut, changes))
}

/// The words a time range of a take covers, for showing review items.
pub fn words_in(transcript: Option<&Transcript>, range: TimeRange) -> String {
    let Some(transcript) = transcript else {
        return String::new();
    };
    transcript
        .words()
        .iter()
        .filter(|w| w.range.overlaps(range))
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcripts() -> HashMap<Id, Transcript> {
        let id = Id::new("t01").unwrap();
        let mut t = Transcript::new(id.clone());
        for (i, text) in ["so", "um", "we", "ship", "on", "fridays"]
            .iter()
            .enumerate()
        {
            let from = i as u64 * 1_000_000;
            t.push(Word {
                text: (*text).to_owned(),
                range: TimeRange::new(Micros(from), Micros(from + 800_000)).unwrap(),
                confidence: 90,
            })
            .unwrap();
        }
        HashMap::from([(id, t)])
    }

    const CUT: &str = "chapter \"Hook\" {\n    take t01 {\n        keep from=0 to=1_000_000\n        remove from=1_000_000 to=2_000_000 by=filler\n        keep from=2_000_000 to=6_000_000\n    }\n}\n";

    #[test]
    fn renders_numbered_words_in_edited_time_with_removed_runs_marked() {
        let cut = Cut::parse(CUT).unwrap();
        assert_eq!(
            render(&cut, &transcripts()),
            "## chapter 1 \"Hook\", take t01\n0:00.0 w0 so [um](filler) we ship on fridays\n\n"
        );
    }

    #[test]
    fn proposes_cuts_and_restores_of_whole_recorded_words() {
        let cut = Cut::parse(CUT).unwrap();
        let span = |from, to| WordSpan {
            take: "t01".to_owned(),
            from_word: from,
            to_word: to,
        };
        let (cut, changes) = apply(&cut, &transcripts(), &[span(4, 5)], &[span(1, 1)]).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "removes t01 w4-w5 \"on fridays\" (1.8 s)"
        );
        let text = render(&cut, &transcripts());
        assert!(
            text.contains("so um we ship [on fridays](manual)"),
            "{text}"
        );
        assert!(apply(&cut, &transcripts(), &[span(5, 9)], &[]).is_err());
    }
}
