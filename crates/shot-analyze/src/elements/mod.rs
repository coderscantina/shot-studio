//! Elements: the facts of a take, written to `elements/<take>.jsonl` in the video folder
//! ([0019]). Builds read the file and never analyse; this module is the analysis.
//!
//! - While recording, [`Log`] appends each word the transcriber settles, so a crash keeps them.
//! - At stop, [`analyze`] with [`Depth::Quick`] maps what milestones 03 to 07 detect to element
//!   kinds: words and sentences, silences and pauses, fillers, retakes and misspoken words,
//!   emphasis, URLs said, idle screen, clicks, typing, record markers, and the script's beats,
//!   chapters and `{name}` tags at the prompter's beat presses. [`write`] replaces the log.
//! - After the rough cut, [`Depth::Full`] adds what Vision reads: text read off the screen and
//!   faces. Claude's reading comes from [`claude`] through the bridge and is kept across
//!   re-analysis with [`add`].
//!
//! The same inputs give the same file, byte for byte.
//!
//! [0019]: ../../../docs/decisions/0019-semantic-video-elements-and-style.md

pub mod claude;
mod mentions;
mod screen;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use shot_capture::events::{Mark, Marker, read_markers};
use shot_core::{
    Attr, AutoCut, Element, ElementKind, Elements, Id, Micros, RuleKind, Script, TimeRange,
    TimedRule, Transcript, Word,
};

pub use mentions::{SaidUrl, urls};
pub use screen::{ReadOut, read_outs};

use crate::autocut::rules::{self, Gaps, Voice};
use crate::autocut::{TakeFacts, zoom};
use crate::events::{self, Input};
use crate::{Error, Speech, idle, normalize, vision};

/// The `by` of each source, with the version of its detector.
pub const BY_WHISPER: &str = "whisper@large-v3-turbo";
pub const BY_VAD: &str = "vad@silero-6.2";
pub const BY_PROSODY: &str = "prosody@1";
pub const BY_TRANSCRIPT: &str = "transcript";
pub const BY_SCREEN: &str = "screen@1";
pub const BY_EVENTS: &str = "events";
pub const BY_SCRIPT: &str = "script";
pub const BY_VISION: &str = "vision@1";
pub const BY_CLAUDE: &str = "claude";

/// Gaps shorter than this are not a silence or pause; a rule's `min` picks among the rest.
pub const GAP_MIN: Micros = Micros(300_000);
/// A pause's confidence grows as it runs past this, the pause rule of every preset.
const PAUSE_REFERENCE: Micros = Micros(1_000_000);
/// The shortest still screen that is idle, the lowest `idle-screen min` of the presets.
pub const IDLE_MIN: Micros = Micros(3_000_000);
/// Findings at or above this are sure, as the presets' `auto-cut confidence`.
const SURE: u8 = 85;
/// Key presses closer than this are one stretch of typing, of at least [`TYPING_KEYS`].
const TYPING_GAP: Micros = Micros(1_000_000);
const TYPING_KEYS: usize = 3;
/// Vision samples on the 0.5 s keyframe grid, the grid the face and text caches share.
const SAMPLE_STEP: Micros = Micros(500_000);

/// How much [`analyze`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// The transcript, speech, event log, script and the encoded screen's stillness. Fast
    /// enough to run between stop and the rough cut.
    Quick,
    /// Also text read off the screen and faces, which decode frames for Vision.
    Full,
}

/// `elements/<take>.jsonl` of the video folder.
pub fn path(video: &Path, take: &Id) -> PathBuf {
    video
        .join("elements")
        .join(format!("{}.jsonl", take.as_str()))
}

/// The elements of a take, `None` without a file. A last line without its newline is what a
/// crash leaves mid-write, so it is dropped.
pub fn read(video: &Path, take: &Id) -> Result<Option<Elements>, Error> {
    let text = match fs::read_to_string(path(video, take)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let whole = text.rfind('\n').map_or("", |end| &text[..=end]);
    Ok(Some(Elements::parse(take.clone(), whole)?))
}

/// Writes the canonical file atomically, replacing what was there.
pub fn write(video: &Path, elements: &Elements) -> Result<PathBuf, Error> {
    let path = path(video, elements.take());
    fs::create_dir_all(path.parent().expect("elements/ is a parent"))?;
    let tmp = path.with_extension("jsonl.tmp");
    fs::write(&tmp, elements.to_jsonl())?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Adds `extra` to `elements`, leaving out any whose identity (kind and `t`) is taken.
pub fn add(elements: &mut Elements, extra: impl IntoIterator<Item = Element>) {
    for element in extra {
        let _ = elements.push(element);
    }
}

/// A take's elements file while it records: each settled word appended as its own line.
pub struct Log {
    take: Id,
    file: Mutex<fs::File>,
}

impl Log {
    /// Starts an empty file for `take`, replacing one left by an earlier recording.
    pub fn create(video: &Path, take: &Id) -> Result<Self, Error> {
        let path = path(video, take);
        fs::create_dir_all(path.parent().expect("elements/ is a parent"))?;
        Ok(Self {
            take: take.clone(),
            file: Mutex::new(fs::File::create(path)?),
        })
    }

    /// Appends `word`. Words arrive in order, as the transcriber settles them.
    pub fn word(&self, word: &Word) -> io::Result<()> {
        let mut line = Elements::new(self.take.clone());
        line.push(word_element(word))
            .expect("one element has no duplicate");
        self.file
            .lock()
            .expect("element log")
            .write_all(line.to_jsonl().as_bytes())
    }
}

/// The elements of `transcript`'s take from the files of the video folder `video`: its event
/// log, its screen track, `script.md` and, at [`Depth::Full`], Vision on its video tracks. The
/// take lasts as long as its screen track or its speech, whichever is longer.
pub fn analyze(
    video: &Path,
    transcript: &Transcript,
    speech: &Speech,
    depth: Depth,
) -> Result<Elements, Error> {
    let take = transcript.take();
    let media = video.join("media").join(take.as_str());
    let events = media.join("events.jsonl");
    let screen = media.join("screen.mp4");
    let mut duration = speech.end().max(
        transcript
            .words()
            .last()
            .map_or(Micros(0), |w| w.range.to()),
    );
    let still = if screen.exists() {
        let (frames, pixels) = idle::encoded_frames(&screen)?;
        if let Some(last) = frames.last() {
            duration = duration.max(last.t);
        }
        idle::still_ranges(&frames, pixels, IDLE_MIN)
    } else {
        Vec::new()
    };
    let script = match fs::read_to_string(video.join("script.md")) {
        Ok(text) => Some(Script::parse(&text)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let facts = Facts {
        transcript,
        speech,
        duration,
        still: &still,
        markers: &read_markers(&events)?,
        input: &events::input(&events)?,
        script: script.as_ref(),
    };
    let mut elements = local(&facts);
    if depth == Depth::Full {
        let words = transcript.words();
        if screen.exists() && !words.is_empty() {
            let times = on_grid(sentences(words).iter().map(|s| words[s.start].range.from()));
            let text = vision::screen_text(video, take, &times)?;
            add(&mut elements, read_out_elements(words, &text));
        }
        if media.join("camera.mp4").exists() {
            let times: Vec<Micros> = (0..duration.0.div_ceil(SAMPLE_STEP.0))
                .map(|i| Micros(i * SAMPLE_STEP.0))
                .collect();
            let faces = vision::face_track(video, take, &times)?;
            add(&mut elements, face_elements(&faces));
        }
    }
    Ok(elements)
}

/// What [`local`] reads, all in the take's media time.
struct Facts<'a> {
    transcript: &'a Transcript,
    speech: &'a Speech,
    duration: Micros,
    /// Stretches of at least [`IDLE_MIN`] where the screen does not change.
    still: &'a [TimeRange],
    markers: &'a [Marker],
    input: &'a Input,
    script: Option<&'a Script>,
}

/// Every element that needs no frame decoded.
fn local(facts: &Facts) -> Elements {
    let transcript = facts.transcript;
    let words = transcript.words();
    let mut out = Elements::new(transcript.take().clone());
    add(&mut out, words.iter().map(word_element));
    add(
        &mut out,
        sentences(words).into_iter().filter_map(|s| {
            let range = TimeRange::new(words[s.start].range.from(), words[s.end - 1].range.to())?;
            Some(Element::over(ElementKind::Sentence, range, BY_WHISPER))
        }),
    );

    let again: Vec<Micros> = facts
        .markers
        .iter()
        .filter(|m| matches!(m.mark, Mark::Again { .. }))
        .map(|m| m.t)
        .collect();
    let take = TakeFacts {
        transcript,
        speech: facts.speech,
        still: facts.still,
        again: &again,
        duration: facts.duration,
    };
    // The rules with every finding on, the gaps left to the loop below, which keeps them whole.
    let reference = AutoCut {
        confidence: SURE,
        silence: TimedRule {
            on: false,
            min: GAP_MIN,
        },
        retake: true,
        misspoken: true,
        pause: TimedRule {
            on: false,
            min: GAP_MIN,
        },
        filler: true,
        idle_screen: TimedRule {
            on: true,
            min: IDLE_MIN,
        },
    };
    for finding in rules::find(&reference, Micros(0), &take, &Voice::default(), |_| true) {
        let (kind, by) = match finding.rule {
            RuleKind::Filler => (ElementKind::Filler, BY_WHISPER),
            RuleKind::Retake => (ElementKind::Retake, BY_WHISPER),
            RuleKind::Misspoken => (ElementKind::Misspoken, BY_WHISPER),
            RuleKind::IdleScreen => (ElementKind::Idle, BY_SCREEN),
            RuleKind::Silence | RuleKind::Pause => unreachable!("gap rules are off"),
        };
        let element = Element::over(kind, finding.range, by).with_confidence(finding.confidence);
        let element = match kind {
            ElementKind::Filler => {
                let phrase: Vec<String> = words
                    .iter()
                    .filter(|w| finding.range.intersect(w.range) == Some(w.range))
                    .map(|w| normalize(&w.text))
                    .collect();
                element.with_text(Attr::Text, phrase.join(" "))
            }
            _ => element,
        };
        add(&mut out, [element]);
    }
    for gap in Gaps::of(&take)
        .all()
        .iter()
        .filter(|g| g.range.duration() >= GAP_MIN)
    {
        let kind = if gap.mid_sentence {
            ElementKind::Pause
        } else {
            ElementKind::Silence
        };
        let element =
            Element::over(kind, gap.range, BY_VAD).with_confidence(gap.confidence(PAUSE_REFERENCE));
        add(&mut out, [element]);
    }

    let mut emphasis_end = Micros(0);
    for loud in zoom::loud_words(words, facts.speech) {
        if loud.range.from() < emphasis_end {
            continue;
        }
        emphasis_end = loud.range.to();
        let confidence = (60.0 + 4.0 * (loud.over - zoom::EMPHASIS_DB)).clamp(60.0, 95.0) as u8;
        add(
            &mut out,
            [Element::over(ElementKind::Emphasis, loud.range, BY_PROSODY)
                .with_confidence(confidence)],
        );
    }

    add(
        &mut out,
        urls(words).into_iter().map(|said| {
            Element::over(ElementKind::Url, said.words, BY_TRANSCRIPT)
                .with_text(Attr::Url, said.url)
                .with_text(Attr::Host, said.host)
        }),
    );

    add(
        &mut out,
        facts.input.clicks.iter().map(|&(t, x, y)| {
            Element::at(ElementKind::Click, t, BY_EVENTS)
                .with_integer(Attr::X, x.into())
                .with_integer(Attr::Y, y.into())
        }),
    );
    add(&mut out, typing(&facts.input.keys));
    add(
        &mut out,
        facts.markers.iter().filter_map(|m| {
            let label = match m.mark {
                Mark::Marker => "M",
                Mark::Zoom => "Z",
                Mark::BRoll => "B",
                Mark::Again { .. } | Mark::Next { .. } | Mark::Layout { .. } => return None,
            };
            Some(Element::at(ElementKind::Marker, m.t, BY_EVENTS).with_text(Attr::Label, label))
        }),
    );
    if let Some(script) = facts.script {
        add(
            &mut out,
            script_elements(script, facts.markers, facts.duration),
        );
    }
    out
}

fn word_element(word: &Word) -> Element {
    Element::over(ElementKind::Word, word.range, BY_WHISPER)
        .with_confidence(word.confidence)
        .with_text(Attr::Text, word.text.clone())
}

/// The words of each sentence, as index ranges: up to a word that ends with `.`, `?` or `!`.
pub(crate) fn sentences(words: &[Word]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, word) in words.iter().enumerate() {
        if word.text.trim_end().ends_with(['.', '?', '!']) || i + 1 == words.len() {
            out.push(start..i + 1);
            start = i + 1;
        }
    }
    out
}

/// Runs of at least [`TYPING_KEYS`] key presses less than [`TYPING_GAP`] apart, from the first
/// press to the last. What was typed stays out: it may be a password.
fn typing(keys: &[Micros]) -> Vec<Element> {
    let mut out = Vec::new();
    let mut start = 0;
    for end in 1..=keys.len() {
        if end < keys.len() && keys[end].0 - keys[end - 1].0 < TYPING_GAP.0 {
            continue;
        }
        let run = &keys[start..end];
        start = end;
        if run.len() >= TYPING_KEYS
            && let Some(range) = TimeRange::new(run[0], run[run.len() - 1])
        {
            out.push(Element::over(ElementKind::Typing, range, BY_EVENTS));
        }
    }
    out
}

/// Beats, chapters and tags where the prompter showed them: beat 1 from the start and each beat
/// an `again` or `next` press moved to. A take without those presses was recorded without the
/// prompter and has none. A beat's tags span until the next beat shows.
fn script_elements(script: &Script, markers: &[Marker], duration: Micros) -> Vec<Element> {
    let beats: Vec<(usize, usize)> = script
        .chapters
        .iter()
        .enumerate()
        .flat_map(|(c, chapter)| (0..chapter.beats.len()).map(move |b| (c, b)))
        .collect();
    let presses: Vec<(Micros, u32)> = markers
        .iter()
        .filter_map(|m| match m.mark {
            Mark::Again { beat } | Mark::Next { beat } => Some((m.t, beat)),
            _ => None,
        })
        .collect();
    if presses.is_empty() {
        return Vec::new();
    }
    let shown: Vec<(Micros, u32)> = std::iter::once((Micros(0), 1)).chain(presses).collect();
    let mut out = Vec::new();
    for (i, &(t, index)) in shown.iter().enumerate() {
        let Some(&(c, b)) = (index as usize).checked_sub(1).and_then(|i| beats.get(i)) else {
            continue;
        };
        let chapter = &script.chapters[c];
        out.push(
            Element::at(ElementKind::Beat, t, BY_SCRIPT).with_integer(Attr::Index, index.into()),
        );
        if b == 0 {
            out.push(
                Element::at(ElementKind::Chapter, t, BY_SCRIPT)
                    .with_text(Attr::Text, &chapter.title),
            );
        }
        let until = shown.get(i + 1).map_or(duration, |(next, _)| *next);
        if let Some(range) = TimeRange::new(t, until) {
            out.extend(tags(&chapter.beats[b].text).into_iter().map(|name| {
                Element::over(ElementKind::Tag, range, BY_SCRIPT).with_text(Attr::Name, name)
            }));
        }
    }
    out
}

/// The `{name}` tags of a beat, in order. A name is letters, digits, `-` and `_`.
fn tags(text: &str) -> Vec<&str> {
    text.split('{')
        .skip(1)
        .filter_map(|rest| {
            let name = rest.split_once('}')?.0;
            let valid = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
            valid.then_some(name)
        })
        .collect()
}

/// `times` snapped down to the sample grid, sorted, each once.
fn on_grid(times: impl Iterator<Item = Micros>) -> Vec<Micros> {
    let mut out: Vec<Micros> = times
        .map(|t| Micros(t.0 / SAMPLE_STEP.0 * SAMPLE_STEP.0))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// `ocr` elements: phrases said while they show on screen, the box in screen pixels.
fn read_out_elements(
    words: &[Word],
    text: &vision::Sampled<Vec<vision::TextLine>>,
) -> Vec<Element> {
    read_outs(words, &text.samples)
        .into_iter()
        .map(|read| {
            let element = Element::over(ElementKind::Ocr, read.said, BY_VISION)
                .with_text(Attr::Text, read.text);
            with_box(element, read.rect, text.size)
        })
        .collect()
}

/// `face` elements: the largest face of each sample, the box in camera pixels.
fn face_elements(faces: &vision::Sampled<Option<vision::NormRect>>) -> Vec<Element> {
    faces
        .samples
        .iter()
        .filter_map(|(t, face)| {
            let range = TimeRange::new(*t, Micros(t.0 + SAMPLE_STEP.0))?;
            Some(with_box(
                Element::over(ElementKind::Face, range, BY_VISION),
                (*face)?,
                faces.size,
            ))
        })
        .collect()
}

fn with_box(element: Element, rect: vision::NormRect, (w, h): (u32, u32)) -> Element {
    let px = |fraction: f32, size: u32| (fraction * size as f32).round() as i64;
    element
        .with_integer(Attr::X, px(rect.x, w))
        .with_integer(Attr::Y, px(rect.y, h))
        .with_integer(Attr::Width, px(rect.w, w))
        .with_integer(Attr::Height, px(rect.h, h))
}

#[cfg(test)]
mod tests;
