//! Captions from word timestamps: the transcript's words placed in output time, with the caption
//! fixes applied ([0015](../../../docs/decisions/0015-caption-fixes-as-a-dictionary.md)) and
//! grouped into cues. The main cut ships them as a WebVTT sidecar next to the export; burned-in
//! captions draw the same cues. Only [`write_sidecar`] does IO.

use std::collections::HashMap;
use std::fmt::Write;
use std::io;
use std::path::{Path, PathBuf};

use shot_core::{CaptionFix, CaptionStyle, Id, Micros, Output, Transcript};

use crate::plan::{ExportPlan, frame_time};

/// A cue holds at most this many words.
const CUE_WORDS: usize = 7;
/// A cue's text is at most this long, the width of one caption line.
const CUE_CHARS: usize = 42;
/// A pause this long starts a new cue.
const CUE_GAP: Micros = Micros(600_000);

/// One word as it shows, in output time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionWord {
    pub text: String,
    pub from: Micros,
    pub to: Micros,
}

/// Words shown together, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub words: Vec<CaptionWord>,
}

impl Cue {
    pub fn from(&self) -> Micros {
        self.words.first().map_or(Micros(0), |w| w.from)
    }

    pub fn to(&self) -> Micros {
        self.words.last().map_or(Micros(0), |w| w.to)
    }

    pub fn text(&self) -> String {
        self.words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The cues of a planned output. A word shows where its middle lands in a segment, so a word cut
/// at an edge shows once or not at all. Takes without a transcript have no captions.
pub fn cues(
    plan: &ExportPlan,
    transcripts: &HashMap<Id, Transcript>,
    fixes: &[CaptionFix],
) -> Vec<Cue> {
    let fixed: HashMap<&Id, Vec<CaptionWord>> = transcripts
        .iter()
        .map(|(take, transcript)| (take, apply_fixes(transcript, fixes)))
        .collect();
    let mut words = Vec::new();
    for planned in &plan.segments {
        let Some(take_words) = fixed.get(&planned.segment.take) else {
            continue;
        };
        let source_from = frame_time(planned.source_frames.start, plan.fps);
        let source_to = frame_time(planned.source_frames.end, plan.fps);
        let output_from = frame_time(planned.output_frame, plan.fps);
        let shift = |t: Micros| {
            Micros(output_from.0 + t.0.clamp(source_from.0, source_to.0) - source_from.0)
        };
        let first = take_words.partition_point(|w| w.to <= source_from);
        for word in take_words[first..]
            .iter()
            .take_while(|w| w.from < source_to)
        {
            let middle = Micros((word.from.0 + word.to.0) / 2);
            if middle < source_from || middle >= source_to {
                continue;
            }
            words.push(CaptionWord {
                text: word.text.clone(),
                from: shift(word.from),
                to: shift(word.to),
            });
        }
    }
    group(words)
}

/// A cue stays up this long after its last word, unless the next one starts.
const CUE_HOLD: Micros = Micros(300_000);

/// What a burned-in caption shows at output time `t`: the cue on screen, and for Word pop the
/// index of the word being said, which draws highlighted. `cues` are in order.
pub fn shown_at(cues: &[Cue], t: Micros) -> Option<(&Cue, usize)> {
    let index = cues.partition_point(|c| c.from() <= t).checked_sub(1)?;
    let cue = &cues[index];
    let until = cues
        .get(index + 1)
        .map_or(Micros(cue.to().0 + CUE_HOLD.0), |next| {
            next.from().min(Micros(cue.to().0 + CUE_HOLD.0))
        });
    if t >= until {
        return None;
    }
    let word = cue.words.partition_point(|w| w.from <= t).saturating_sub(1);
    Some((cue, word))
}

/// The cues as a WebVTT file, or `None` for [`CaptionStyle::Off`]. Word pop marks every word's
/// start with a cue timestamp, so players that support it light words up as they are said.
pub fn webvtt(cues: &[Cue], style: CaptionStyle) -> Option<String> {
    if style == CaptionStyle::Off {
        return None;
    }
    let mut out = String::from("WEBVTT\n");
    for (i, cue) in cues.iter().enumerate() {
        let _ = write!(
            out,
            "\n{}\n{} --> {}\n",
            i + 1,
            timestamp(cue.from()),
            timestamp(cue.to())
        );
        for (j, word) in cue.words.iter().enumerate() {
            if j > 0 {
                out.push(' ');
                if style == CaptionStyle::WordPop {
                    let _ = write!(out, "<{}>", timestamp(word.from));
                }
            }
            out.push_str(&escape(&word.text));
        }
        out.push('\n');
    }
    Some(out)
}

/// Writes the main cut's captions next to `out` as `<out>.vtt`, from the cached transcripts in
/// `video_dir`, or removes a stale sidecar when the style is `off`. Returns the path written.
pub fn write_sidecar(
    video_dir: &Path,
    plan: &ExportPlan,
    output: &Output,
    out: &Path,
) -> io::Result<Option<PathBuf>> {
    let path = out.with_extension("vtt");
    let mut transcripts = HashMap::new();
    for planned in &plan.segments {
        let take = &planned.segment.take;
        if transcripts.contains_key(take) {
            continue;
        }
        if let Some(transcript) = shot_analyze::cache::read_cached(video_dir, take)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
        {
            transcripts.insert(take.clone(), transcript);
        }
    }
    let cues = cues(plan, &transcripts, &output.caption_fixes);
    match webvtt(&cues, output.captions) {
        Some(text) => {
            std::fs::write(&path, text)?;
            Ok(Some(path))
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(None),
        },
    }
}

/// The take's words with every fix applied: a run of words reading `heard`, ignoring case and
/// punctuation, becomes one word `said` with the run's timing and the last word's trailing
/// punctuation.
pub(crate) fn apply_fixes(transcript: &Transcript, fixes: &[CaptionFix]) -> Vec<CaptionWord> {
    let key = |text: &str| -> String {
        text.trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
    };
    let heard: Vec<(Vec<String>, &str)> = fixes
        .iter()
        .map(|fix| {
            (
                fix.heard.split_whitespace().map(key).collect(),
                fix.said.as_str(),
            )
        })
        .filter(|(tokens, _): &(Vec<String>, &str)| !tokens.is_empty())
        .collect();
    let words = transcript.words();
    let keys: Vec<String> = words.iter().map(|w| key(&w.text)).collect();
    let mut out = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let hit = heard.iter().find(|(tokens, _)| {
            keys.len() >= i + tokens.len() && tokens.iter().zip(&keys[i..]).all(|(a, b)| a == b)
        });
        match hit {
            Some((tokens, said)) => {
                let last = &words[i + tokens.len() - 1];
                let trailing: String = last
                    .text
                    .chars()
                    .rev()
                    .take_while(|c| !c.is_alphanumeric())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                out.push(CaptionWord {
                    text: format!("{said}{trailing}"),
                    from: words[i].range.from(),
                    to: last.range.to(),
                });
                i += tokens.len();
            }
            None => {
                out.push(CaptionWord {
                    text: words[i].text.clone(),
                    from: words[i].range.from(),
                    to: words[i].range.to(),
                });
                i += 1;
            }
        }
    }
    out
}

/// Breaks words into cues at sentence ends, pauses and the cue limits.
fn group(words: Vec<CaptionWord>) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();
    let mut current: Vec<CaptionWord> = Vec::new();
    for word in words {
        if let Some(last) = current.last() {
            let chars: usize =
                current.iter().map(|w| w.text.len() + 1).sum::<usize>() + word.text.len();
            let ends = last.text.ends_with(['.', '?', '!']);
            if ends
                || current.len() >= CUE_WORDS
                || chars > CUE_CHARS
                || word.from.0 >= last.to.0 + CUE_GAP.0
            {
                cues.push(Cue {
                    words: std::mem::take(&mut current),
                });
            }
        }
        current.push(word);
    }
    if !current.is_empty() {
        cues.push(Cue { words: current });
    }
    cues
}

fn timestamp(t: Micros) -> String {
    let ms = t.0 / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{
        Chapter, Cut, Format, FormatKind, MediaId, Project, RangeEdit, RemovedBy, RuleKind, Script,
        TakeEdit, TimeRange, Timeline, Video, Word,
    };

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    fn transcript(words: &[(&str, u64, u64)]) -> Transcript {
        let mut transcript = Transcript::new(Id::new("t01").unwrap());
        for &(text, from, to) in words {
            transcript
                .push(Word {
                    text: text.into(),
                    range: range(from, to),
                    confidence: 90,
                })
                .unwrap();
        }
        transcript
    }

    fn plan(ranges: Vec<RangeEdit>) -> ExportPlan {
        let format: Format = FormatKind::Opinion.preset();
        let project = Project {
            video: Video::parse("title \"A\"\nstatus editing\nformat opinion\n").unwrap(),
            script: Script::parse("# A\n").unwrap(),
            format,
            cut: Cut {
                chapters: vec![Chapter {
                    title: "A".into(),
                    takes: vec![TakeEdit {
                        take: Id::new("t01").unwrap(),
                        ranges,
                        layouts: Vec::new(),
                        reviews: Vec::new(),
                        zooms: Vec::new(),
                        overlays: Vec::new(),
                    }],
                }],
                ..Cut::default()
            },
        };
        let timeline = Timeline::evaluate(&project);
        ExportPlan::new(timeline.main(), 60, |_| MediaId::of(b"t01"))
    }

    #[test]
    fn words_move_to_output_time_and_fixes_take_the_matched_timing() {
        let transcripts = HashMap::from([(
            Id::new("t01").unwrap(),
            transcript(&[
                ("We", 0, 200_000),
                ("um", 300_000, 600_000),
                ("love", 1_000_000, 1_200_000),
                ("lara", 1_200_000, 1_400_000),
                ("vel.", 1_400_000, 1_700_000),
            ]),
        )]);
        let plan = plan(vec![
            RangeEdit::Keep(range(0, 300_000)),
            RangeEdit::Remove {
                range: range(300_000, 1_000_000),
                by: RemovedBy::Rule(RuleKind::Filler),
            },
            RangeEdit::Keep(range(1_000_000, 2_000_000)),
        ]);
        let fixes = [CaptionFix {
            heard: "Lara Vel".into(),
            said: "Laravel".into(),
        }];
        let cues = cues(&plan, &transcripts, &fixes);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text(), "We love Laravel.");
        assert_eq!(cues[0].words[2].from, Micros(500_000));
        assert_eq!(cues[0].to(), Micros(1_000_000));
        assert_eq!(
            webvtt(&cues, CaptionStyle::WordPop).unwrap(),
            "WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\nWe <00:00:00.300>love <00:00:00.500>Laravel.\n"
        );
        assert_eq!(webvtt(&cues, CaptionStyle::Off), None);
    }

    #[test]
    fn a_cue_shows_its_current_word_and_holds_briefly() {
        let word = |text: &str, from: u64| CaptionWord {
            text: text.into(),
            from: Micros(from),
            to: Micros(from + 200_000),
        };
        let cues = [
            Cue {
                words: vec![word("Ship", 0), word("it.", 200_000)],
            },
            Cue {
                words: vec![word("Now", 2_000_000)],
            },
        ];
        assert_eq!(
            shown_at(&cues, Micros(250_000)).map(|(c, w)| (c.text(), w)),
            Some(("Ship it.".into(), 1))
        );
        assert!(shown_at(&cues, Micros(650_000)).is_some());
        assert!(shown_at(&cues, Micros(800_000)).is_none());
        assert_eq!(shown_at(&cues, Micros(2_000_000)).map(|(_, w)| w), Some(0));
    }

    #[test]
    fn cues_break_at_sentences_pauses_and_length() {
        let word = |text: &str, from: u64| CaptionWord {
            text: text.into(),
            from: Micros(from),
            to: Micros(from + 200_000),
        };
        let words = vec![
            word("Ship.", 0),
            word("It", 200_000),
            word("works", 400_000),
            word("later", 2_000_000),
        ];
        let cues = group(words);
        let texts: Vec<String> = cues.iter().map(Cue::text).collect();
        assert_eq!(texts, ["Ship.", "It works", "later"]);
    }
}
