//! What the viewer hears: the transcript's words placed in an output's time, and the chapter
//! marks they give.

use std::collections::HashMap;
use std::fmt::Write;

use shot_core::{Id, Micros, Output, Transcript};

/// A word as the output plays it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenWord {
    pub text: String,
    /// Start and end in output time.
    pub at: Micros,
    pub to: Micros,
    /// The output chapter the word plays in.
    pub chapter: usize,
    pub confidence: u8,
}

/// A chapter of the output and the moment its first word is heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterMark {
    pub title: String,
    pub at: Micros,
}

/// The words `output` keeps, in output order. A word plays where its middle lands in a segment,
/// like captions, so a word cut at an edge counts once or not at all. Takes without a
/// transcript are silent here.
pub fn spoken_words(output: &Output, transcripts: &HashMap<Id, Transcript>) -> Vec<SpokenWord> {
    let mut words = Vec::new();
    let mut offset = 0;
    for (chapter, group) in output.chapters.iter().enumerate() {
        for segment in &group.segments {
            let source = segment.source;
            if let Some(transcript) = transcripts.get(&segment.take) {
                for word in transcript.words() {
                    let middle = (word.range.from().0 + word.range.to().0) / 2;
                    if middle < source.from().0 || middle >= source.to().0 {
                        continue;
                    }
                    let place = |t: Micros| {
                        Micros(offset + t.0.clamp(source.from().0, source.to().0) - source.from().0)
                    };
                    words.push(SpokenWord {
                        text: word.text.clone(),
                        at: place(word.range.from()),
                        to: place(word.range.to()),
                        chapter,
                        confidence: word.confidence,
                    });
                }
            }
            offset += source.duration().0;
        }
    }
    words
}

/// The output's length.
pub fn duration(output: &Output) -> Micros {
    Micros(
        output
            .chapters
            .iter()
            .flat_map(|c| &c.segments)
            .map(|s| s.source.duration().0)
            .sum(),
    )
}

/// One mark per chapter that plays: the first at 0:00, the others where their first word is
/// heard, or where the chapter starts when nobody speaks in it.
pub fn chapter_marks(output: &Output, words: &[SpokenWord]) -> Vec<ChapterMark> {
    let mut marks = Vec::new();
    let mut start = 0;
    for (index, group) in output.chapters.iter().enumerate() {
        let length: u64 = group.segments.iter().map(|s| s.source.duration().0).sum();
        if length == 0 {
            continue;
        }
        let at = if marks.is_empty() {
            0
        } else {
            words
                .iter()
                .find(|w| w.chapter == index)
                .map_or(start, |w| w.at.0)
        };
        marks.push(ChapterMark {
            title: group.title.clone(),
            at: Micros(at),
        });
        start += length;
    }
    marks
}

/// `m:ss`, or `h:mm:ss` from an hour on, rounded down to the second as YouTube reads chapters.
pub fn clock(t: Micros) -> String {
    let seconds = t.0 / 1_000_000;
    let mut out = String::new();
    if seconds >= 3_600 {
        let _ = write!(
            out,
            "{}:{:02}:{:02}",
            seconds / 3_600,
            seconds % 3_600 / 60,
            seconds % 60
        );
    } else {
        let _ = write!(out, "{}:{:02}", seconds / 60, seconds % 60);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use shot_core::{Cut, FormatKind, Project, Script, TimeRange, Timeline, Video, Word};

    pub(crate) fn project(cut: &str) -> Project {
        Project {
            video: Video::parse("title \"A\"\nstatus editing\nformat opinion\n").unwrap(),
            script: Script::parse("# A\n").unwrap(),
            format: FormatKind::Opinion.preset(),
            cut: Cut::parse(cut).unwrap(),
        }
    }

    pub(crate) fn transcript(words: &[(&str, u64, u64)]) -> HashMap<Id, Transcript> {
        let id = Id::new("t01").unwrap();
        let mut transcript = Transcript::new(id.clone());
        for &(text, from, to) in words {
            transcript
                .push(Word {
                    text: text.to_owned(),
                    range: TimeRange::new(Micros(from), Micros(to)).unwrap(),
                    confidence: 90,
                })
                .unwrap();
        }
        HashMap::from([(id, transcript)])
    }

    const CUT: &str = "chapter \"Hook\" {\n    take t01 {\n        keep from=0 to=2_000_000\n        remove from=2_000_000 to=5_000_000 by=manual\n        keep from=5_000_000 to=8_000_000\n    }\n}\n\nchapter \"Body\" {\n    take t01 {\n        keep from=8_000_000 to=20_000_000\n    }\n}\n";

    #[test]
    fn places_kept_words_in_output_time_and_marks_chapters_at_their_first_word() {
        let project = project(CUT);
        let main = &Timeline::evaluate(&project).outputs[0];
        let transcripts = transcript(&[
            ("ship", 500_000, 900_000),
            ("cut", 3_000_000, 3_400_000),
            ("it", 5_500_000, 5_800_000),
            ("then", 9_000_000, 9_400_000),
        ]);
        let words = spoken_words(main, &transcripts);
        let placed: Vec<_> = words.iter().map(|w| (w.text.as_str(), w.at.0)).collect();
        assert_eq!(
            placed,
            [("ship", 500_000), ("it", 2_500_000), ("then", 6_000_000)]
        );
        let marks = chapter_marks(main, &words);
        assert_eq!(marks[0].at, Micros(0));
        assert_eq!(marks[1].at, Micros(6_000_000));
        assert_eq!(duration(main), Micros(17_000_000));
    }

    #[test]
    fn clocks_read_like_youtube_chapters() {
        assert_eq!(clock(Micros(65_900_000)), "1:05");
        assert_eq!(clock(Micros(3_725_000_000)), "1:02:05");
    }
}
