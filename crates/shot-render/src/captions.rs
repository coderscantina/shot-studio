//! Captions from word timestamps: the transcript's words placed in output time, with the caption
//! fixes applied ([0015](../../../docs/decisions/0015-caption-fixes-as-a-dictionary.md)) and
//! grouped into cues. The main cut ships them as a WebVTT sidecar next to the export; burned-in
//! captions draw the same cues in a [`Design`]: the preset with the brand's and the video's look,
//! and the Edit preview draws them with the same [`shape`]. Only [`write_sidecar`] does IO.

use std::collections::HashMap;
use std::fmt::Write;
use std::io;
use std::path::{Path, PathBuf};

use shot_core::{
    CaptionBack, CaptionBreak, CaptionCase, CaptionDesign, CaptionFix, CaptionFont, CaptionLook,
    CaptionPosition, CaptionStyle, Emphasis, Id, Micros, Output, Transcript,
};

use crate::drawing::{Caption, Font, Rgba, Shape};
use crate::kinetic;
use crate::layout::Canvas;
use crate::plan::{ExportPlan, frame_time};
use crate::theme::Theme;

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

/// A word of a take with where it starts in the take, before it moves to output time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceWord {
    pub take: Id,
    /// The take's media time the word starts at, which a line break names.
    pub at: Micros,
    pub word: CaptionWord,
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

/// The cues of a planned output, grouped as the sidecar groups them.
pub fn cues(
    plan: &ExportPlan,
    transcripts: &HashMap<Id, Transcript>,
    fixes: &[CaptionFix],
) -> Vec<Cue> {
    group(
        words(plan, transcripts, fixes)
            .into_iter()
            .map(|w| w.word)
            .collect(),
    )
}

/// The words of a planned output in output time, each with where it starts in its take. A word
/// shows where its middle lands in a segment, so a word cut at an edge shows once or not at all.
/// Takes without a transcript have no captions.
pub fn words(
    plan: &ExportPlan,
    transcripts: &HashMap<Id, Transcript>,
    fixes: &[CaptionFix],
) -> Vec<SourceWord> {
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
            words.push(SourceWord {
                take: planned.segment.take.clone(),
                at: word.from,
                word: CaptionWord {
                    text: word.text.clone(),
                    from: shift(word.from),
                    to: shift(word.to),
                },
            });
        }
    }
    words
}

/// The burned-in cues of a planned output: at most `most` words each, with a person's line
/// breaks.
pub fn burned_cues(
    plan: &ExportPlan,
    transcripts: &HashMap<Id, Transcript>,
    fixes: &[CaptionFix],
    breaks: &[CaptionBreak],
    most: usize,
) -> Vec<Cue> {
    burned_lines(plan, transcripts, fixes, breaks, most)
        .into_iter()
        .map(|line| Cue {
            words: line.into_iter().map(|w| w.word).collect(),
        })
        .collect()
}

/// [`burned_cues`] with where each word starts in its take, for editing them.
pub fn burned_lines(
    plan: &ExportPlan,
    transcripts: &HashMap<Id, Transcript>,
    fixes: &[CaptionFix],
    breaks: &[CaptionBreak],
    most: usize,
) -> Vec<Vec<SourceWord>> {
    let mut words = words(plan, transcripts, fixes).into_iter();
    let forced: Vec<Option<bool>> = words
        .as_slice()
        .iter()
        .map(|w| {
            breaks
                .iter()
                .find(|b| b.take == w.take && b.at == w.at)
                .map(|b| b.split)
        })
        .collect();
    let cues = group_with(
        words.as_slice().iter().map(|w| w.word.clone()).collect(),
        most,
        &forced,
    );
    cues.iter()
        .map(|cue| words.by_ref().take(cue.words.len()).collect())
        .collect()
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

/// The cues as a WebVTT file, or `None` for [`CaptionStyle::Off`]. Every preset but Clean marks
/// each word's start with a cue timestamp, so players that support it light words up as they
/// are said.
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
                if style != CaptionStyle::Clean {
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
/// punctuation, becomes the words of `said` with the run's timing spread over them by length, and
/// the last word's trailing punctuation. Where several fixes match, the longest wins.
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
        let hit = heard
            .iter()
            .filter(|(tokens, _)| {
                keys.len() >= i + tokens.len() && tokens.iter().zip(&keys[i..]).all(|(a, b)| a == b)
            })
            .max_by_key(|(tokens, _)| tokens.len());
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
                let (from, to) = (words[i].range.from().0, last.range.to().0);
                let parts: Vec<&str> = said.split_whitespace().collect();
                let total: u64 = parts.iter().map(|p| p.chars().count() as u64 + 1).sum();
                let mut start = from;
                let mut done = 0;
                for (n, part) in parts.iter().enumerate() {
                    done += part.chars().count() as u64 + 1;
                    let end = if n + 1 == parts.len() {
                        to
                    } else {
                        from + (to - from) * done / total.max(1)
                    };
                    let text = if n + 1 == parts.len() {
                        format!("{part}{trailing}")
                    } else {
                        (*part).to_owned()
                    };
                    out.push(CaptionWord {
                        text,
                        from: Micros(start),
                        to: Micros(end),
                    });
                    start = end;
                }
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
pub(crate) fn group(words: Vec<CaptionWord>) -> Vec<Cue> {
    group_with(words, CUE_WORDS, &[])
}

/// [`group`] with at most `most` words a cue, and `forced` per word: `Some(true)` starts a cue
/// there, `Some(false)` keeps the word on the cue before.
fn group_with(words: Vec<CaptionWord>, most: usize, forced: &[Option<bool>]) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();
    let mut current: Vec<CaptionWord> = Vec::new();
    for (i, word) in words.into_iter().enumerate() {
        if let Some(last) = current.last() {
            let chars: usize =
                current.iter().map(|w| w.text.len() + 1).sum::<usize>() + word.text.len();
            let ends = last.text.ends_with(['.', '?', '!']);
            let automatic = ends
                || current.len() >= most.max(1)
                || chars > CUE_CHARS
                || word.from.0 >= last.to.0 + CUE_GAP.0;
            if forced.get(i).copied().flatten().unwrap_or(automatic) {
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

/// A caption on screen: the cue's index, the word being said when the design marks it, and how
/// long since that word started, up to [`kinetic::word_pop_settle`], for its pop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub cue: usize,
    pub word: Option<usize>,
    pub pop: Micros,
}

/// A caption design ready to draw: the preset with the brand's look and the video's look over
/// it, in the brand's fonts and accent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Design {
    pub design: CaptionDesign,
    pub font: Font,
    pub color: Rgba,
    pub accent: Rgba,
}

/// The box behind the words, and the outline around them.
const BACKGROUND: u32 = 0x0A0B0D;

fn rgba(color: shot_core::Color) -> Rgba {
    let [r, g, b] = color.0;
    Rgba::hex(u32::from_be_bytes([0, r, g, b]), 1.0)
}

impl Design {
    /// The design of `style` with the brand's look in `theme` and the video's `look` over it.
    pub fn new(style: CaptionStyle, look: &CaptionLook, theme: &Theme) -> Self {
        let design = theme.captions.merge(look).design(style);
        let face = match design.font {
            CaptionFont::Heading => theme.fonts.heading,
            CaptionFont::Body => theme.fonts.body,
            CaptionFont::Mono => theme.fonts.mono,
        };
        Self {
            design,
            font: Font::Face(face.at(design.weight)),
            color: rgba(design.color),
            accent: design.highlight.map_or(theme.palette.accent, rgba),
        }
    }

    /// The design of an output: its style and its look over the brand's.
    pub fn of(output: &Output, theme: &Theme) -> Self {
        Self::new(output.captions, &output.caption_look, theme)
    }

    pub fn style(&self) -> CaptionStyle {
        self.design.style
    }

    /// Everything the design draws with, for chunk keys.
    pub fn key(&self) -> Vec<u8> {
        let mut key = format!("{:?}", self.design).into_bytes();
        if let Font::Face(face) = self.font {
            key.extend_from_slice(&face.hash());
            key.extend_from_slice(&face.weight.to_le_bytes());
        }
        for channel in self.color.0.iter().chain(&self.accent.0) {
            key.extend_from_slice(&channel.to_le_bytes());
        }
        key
    }

    /// The burned-in cues of a planned output in this design.
    pub fn cues(
        &self,
        plan: &ExportPlan,
        transcripts: &HashMap<Id, Transcript>,
        output: &Output,
    ) -> Vec<Cue> {
        burned_cues(
            plan,
            transcripts,
            &output.caption_fixes,
            &output.caption_breaks,
            usize::from(self.design.words),
        )
    }

    /// What shows at output time `t`: the cue and the word being said, or nothing with captions
    /// off or between cues. `cues` are in order.
    pub fn shown(&self, cues: &[Cue], t: Micros) -> Option<Shown> {
        if self.design.style == CaptionStyle::Off {
            return None;
        }
        let (_, word) = shown_at(cues, t)?;
        let cue = cues.partition_point(|c| c.from() <= t) - 1;
        let since = Micros(t.0.saturating_sub(cues[cue].words[word].from.0));
        let settled = kinetic::word_pop_settle();
        Some(match self.design.emphasis {
            Emphasis::None => Shown {
                cue,
                word: None,
                pop: settled,
            },
            Emphasis::Fill => Shown {
                cue,
                word: Some(word),
                pop: settled,
            },
            Emphasis::Pill | Emphasis::Color => Shown {
                cue,
                word: Some(word),
                pop: since.min(settled),
            },
        })
    }

    /// The caption `shown` of `cue` on a canvas.
    pub fn shape(&self, cue: &Cue, shown: Shown, canvas: Canvas) -> Shape {
        let d = &self.design;
        let tall = canvas.height > canvas.width;
        let (w, h) = (canvas.width as f32, canvas.height as f32);
        let size = w * if tall { 0.062 } else { 0.05 } * f32::from(d.size) / 100.0;
        let at = match (d.position, tall) {
            (CaptionPosition::Bottom, true) => 0.72,
            (CaptionPosition::Bottom, false) => 0.83,
            (CaptionPosition::Middle, _) => 0.5,
            (CaptionPosition::Top, true) => 0.2,
            (CaptionPosition::Top, false) => 0.15,
        } + f32::from(d.nudge) / 100.0;
        let words = cue
            .words
            .iter()
            .map(|word| match d.case {
                CaptionCase::AsSaid => word.text.clone(),
                CaptionCase::Upper => word.text.to_uppercase(),
            })
            .collect();
        Shape::Caption(Box::new(Caption {
            center: [w / 2.0, h * at],
            width: w * 0.84,
            words,
            highlight: shown.word,
            size,
            font: self.font,
            color: self.color,
            accent: self.accent,
            background: Rgba::hex(
                BACKGROUND,
                if d.back == CaptionBack::Box {
                    0.78
                } else {
                    1.0
                },
            ),
            back: d.back,
            emphasis: d.emphasis,
            pop: kinetic::word_pop(shown.pop),
        }))
    }
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

    #[test]
    fn the_longest_fix_wins_and_spreads_its_timing_over_the_said_words() {
        let words = transcript(&[
            ("we", 0, 100_000),
            ("love", 100_000, 300_000),
            ("lara", 300_000, 500_000),
            ("vel.", 500_000, 700_000),
        ]);
        let fixes = [
            CaptionFix {
                heard: "lara vel".into(),
                said: "Laravel".into(),
            },
            CaptionFix {
                heard: "love lara vel".into(),
                said: "really like Laravel".into(),
            },
        ];
        let fixed = apply_fixes(&words, &fixes);
        let texts: Vec<&str> = fixed.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["we", "really", "like", "Laravel."]);
        assert_eq!(fixed[1].from, Micros(100_000));
        assert_eq!(fixed[3].to, Micros(700_000));
        assert!(fixed[1].to < fixed[2].to && fixed[2].to < fixed[3].to);
    }

    #[test]
    fn a_split_starts_a_line_and_a_join_keeps_a_word_on_the_line_before() {
        let word = |text: &str, from: u64| CaptionWord {
            text: text.into(),
            from: Micros(from),
            to: Micros(from + 200_000),
        };
        let words = vec![
            word("Ship", 0),
            word("it", 200_000),
            word("now.", 400_000),
            word("Then", 600_000),
            word("rest", 800_000),
        ];
        let texts = |forced: &[Option<bool>], most| -> Vec<String> {
            group_with(words.clone(), most, forced)
                .iter()
                .map(Cue::text)
                .collect()
        };
        assert_eq!(texts(&[], 7), ["Ship it now.", "Then rest"]);
        assert_eq!(texts(&[], 2), ["Ship it", "now.", "Then rest"]);
        let forced = [None, Some(true), None, Some(false), None];
        assert_eq!(texts(&forced, 7), ["Ship", "it now. Then rest"]);
    }

    #[test]
    fn each_preset_marks_the_word_being_said_its_own_way() {
        let theme = Theme::default();
        let word = |text: &str, from: u64| CaptionWord {
            text: text.into(),
            from: Micros(from),
            to: Micros(from + 200_000),
        };
        let cues = [Cue {
            words: vec![word("Ship", 0), word("it.", 200_000)],
        }];
        let at = Micros(250_000);
        let shown = |style| Design::new(style, &CaptionLook::default(), &theme).shown(&cues, at);
        assert_eq!(shown(CaptionStyle::Off), None);
        assert_eq!(shown(CaptionStyle::Clean).unwrap().word, None);
        let pop = shown(CaptionStyle::WordPop).unwrap();
        assert_eq!((pop.word, pop.pop), (Some(1), Micros(50_000)));
        let fill = shown(CaptionStyle::Karaoke).unwrap();
        assert_eq!((fill.word, fill.pop), (Some(1), kinetic::word_pop_settle()));
        let bold = Design::new(CaptionStyle::Bold, &CaptionLook::default(), &theme);
        let Shape::Caption(caption) =
            bold.shape(&cues[0], shown(CaptionStyle::Bold).unwrap(), Canvas::UHD)
        else {
            panic!("a caption");
        };
        assert_eq!(caption.words, ["SHIP", "IT."], "bold is uppercase");
        assert_eq!(caption.back, CaptionBack::Outline);
        let bigger = CaptionLook {
            size: Some(150),
            ..CaptionLook::default()
        };
        assert_ne!(
            Design::new(CaptionStyle::Bold, &bigger, &theme).key(),
            bold.key(),
            "the look is part of the key"
        );
    }
}
