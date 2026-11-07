//! Runs the overlay rules over a video folder and writes their overlays into the main cut's take
//! blocks, like the auto-zoom rule writes zooms:
//!
//! - **Link cards** for every URL said in a kept part of the transcript, read with the caption
//!   fixes applied, so a fix for a misheard name also fixes its link.
//! - **B-roll slots** over idle screen of at least 8 s and at every B marker of the event log.
//! - **Highlights** where you read out text that is on screen, found with Vision.
//!
//! Each rule skips what an overlay of its template already covers, so running them again only
//! adds what is new, and a person's changes to an overlay stay. [`redact`] places a bleep or a
//! blur on words of the transcript, [`fill_b_roll`] picks the file of a slot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use shot_analyze::vision::{self, VisionError};
use shot_core::{
    CaptionFix, Cut, Id, Micros, Overlay, ParamValue, TakeEdit, TimeRange, Transcript, Word,
};

use crate::layout::Rect;
use crate::overlay::highlight::{self, ScreenLine};
use crate::overlay::{broll, link_card, overlay, redact as templates};

#[derive(Debug, thiserror::Error)]
pub enum OverlayRuleError {
    #[error("reading transcripts: {0}")]
    Transcript(shot_analyze::Error),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Vision(#[from] VisionError),
    #[error("take {0} has no transcript yet")]
    NoTranscript(Id),
    #[error("take {take} never says \"{phrase}\"")]
    NotSaid { take: Id, phrase: String },
    #[error("\"{0}\" is not on screen when it is said")]
    NotOnScreen(String),
    #[error("the cut shows no part of {take} at {at:?}")]
    NotInCut { take: Id, at: Micros },
    #[error("there is no b-roll at {at:?} of {take}")]
    NoSlot { take: Id, at: Micros },
}

/// How many overlays each rule added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Added {
    pub link_cards: usize,
    pub b_roll: usize,
    pub highlights: usize,
}

/// What a take offers the rules, read once per take.
struct TakeFacts {
    words: Vec<Word>,
    idle: Vec<TimeRange>,
    markers: Vec<Micros>,
    /// Text on screen at sample times, in screen pixels.
    screen: Vec<(Micros, Vec<ScreenLine>)>,
}

/// `cut` with link cards, b-roll slots and highlights added to every take block of the main cut.
pub fn run(video_dir: &Path, cut: &Cut) -> Result<(Cut, Added), OverlayRuleError> {
    let mut facts: HashMap<Id, TakeFacts> = HashMap::new();
    let mut added = Added::default();
    let mut cut = cut.clone();
    for chapter in &mut cut.chapters {
        for edit in &mut chapter.takes {
            if !facts.contains_key(&edit.take) {
                let read = take_facts(video_dir, &edit.take, &cut.caption_fixes)?;
                facts.insert(edit.take.clone(), read);
            }
            let take = &facts[&edit.take];
            let kept = |t: Micros| edit_keeps(edit, t);
            let words: Vec<Word> = take
                .words
                .iter()
                .filter(|w| kept(w.range.from()))
                .cloned()
                .collect();
            let cards = link_card::cards(&words, &edit.overlays);
            let slots: Vec<Overlay> = broll::slots(&take.idle, &take.markers, &edit.overlays)
                .into_iter()
                .filter(|o| kept(o.range.from()))
                .collect();
            let marks = highlight::matches(&words, &take.screen, &edit.overlays);
            added.link_cards += cards.len();
            added.b_roll += slots.len();
            added.highlights += marks.len();
            edit.overlays
                .extend(cards.into_iter().chain(slots).chain(marks));
        }
    }
    Ok((cut, added))
}

fn edit_keeps(edit: &TakeEdit, t: Micros) -> bool {
    TimeRange::new(t, Micros(t.0 + 1)).is_some_and(|r| edit.keeps(r))
}

fn take_facts(
    video_dir: &Path,
    take: &Id,
    fixes: &[CaptionFix],
) -> Result<TakeFacts, OverlayRuleError> {
    let media = video_dir.join("media").join(take.as_str());
    let words: Vec<Word> = shot_analyze::cache::read_cached(video_dir, take)
        .map_err(OverlayRuleError::Transcript)?
        .map(|t: Transcript| {
            crate::captions::apply_fixes(&t, fixes)
                .into_iter()
                .filter_map(|w| {
                    Some(Word {
                        range: TimeRange::new(w.from, w.to)?,
                        text: w.text,
                        confidence: 100,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let screen_path = media.join("screen.mp4");
    let has_screen = screen_path.exists();
    let idle = if has_screen {
        let (frames, pixels) =
            shot_analyze::idle::encoded_frames(&screen_path).map_err(|source| {
                OverlayRuleError::Io {
                    path: screen_path.clone(),
                    source,
                }
            })?;
        shot_analyze::idle::still_ranges(&frames, pixels, broll::IDLE_MIN)
    } else {
        Vec::new()
    };
    let events = media.join(crate::assets::EVENTS_FILE);
    let markers = shot_capture::events::read_markers(&events)
        .map_err(|source| OverlayRuleError::Io {
            path: events.clone(),
            source,
        })?
        .into_iter()
        .filter(|m| m.mark == shot_capture::events::Mark::BRoll)
        .map(|m| m.t)
        .collect();
    let screen = if has_screen && !words.is_empty() {
        screen_lines(video_dir, take, &sentence_starts(&words))?
    } else {
        Vec::new()
    };
    Ok(TakeFacts {
        words,
        idle,
        markers,
        screen,
    })
}

/// Where each sentence starts, snapped down to the keyframe grid so each sample decodes one
/// keyframe.
fn sentence_starts(words: &[Word]) -> Vec<Micros> {
    let step = crate::derived::FACE_STEP.0;
    let mut times: Vec<Micros> = words
        .iter()
        .enumerate()
        .filter(|(i, _)| *i == 0 || words[i - 1].text.ends_with(['.', '?', '!', ':']))
        .map(|(_, w)| Micros(w.range.from().0 / step * step))
        .collect();
    times.dedup();
    times
}

/// The text Vision reads on the screen track at `times`, in screen pixels.
fn screen_lines(
    video_dir: &Path,
    take: &Id,
    times: &[Micros],
) -> Result<Vec<(Micros, Vec<ScreenLine>)>, OverlayRuleError> {
    let sampled = vision::screen_text(video_dir, take, times)?;
    let (w, h) = (sampled.size.0 as f32, sampled.size.1 as f32);
    Ok(sampled
        .samples
        .into_iter()
        .map(|(t, lines)| {
            let lines = lines
                .into_iter()
                .map(|line| ScreenLine {
                    rect: Rect::new(
                        line.rect.x * w,
                        line.rect.y * h,
                        (line.rect.x + line.rect.w) * w,
                        (line.rect.y + line.rect.h) * h,
                    ),
                    text: line.text,
                })
                .collect();
            (t, lines)
        })
        .collect())
}

/// What [`redact`] hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redaction {
    /// The voice saying the words.
    Bleep,
    /// The words on screen, from when they are said for `hold` longer.
    Blur { hold: Micros },
}

/// `cut` with a bleep or a blur on the first time `take` says `phrase`, in the take block that
/// shows it. A blur covers where Vision finds the same words on screen.
pub fn redact(
    video_dir: &Path,
    cut: &Cut,
    take: &Id,
    phrase: &str,
    how: Redaction,
) -> Result<Cut, OverlayRuleError> {
    let transcript = shot_analyze::cache::read_cached(video_dir, take)
        .map_err(OverlayRuleError::Transcript)?
        .ok_or_else(|| OverlayRuleError::NoTranscript(take.clone()))?;
    let said = templates::phrase_range(transcript.words(), phrase).ok_or_else(|| {
        OverlayRuleError::NotSaid {
            take: take.clone(),
            phrase: phrase.to_owned(),
        }
    })?;
    let placed = match how {
        Redaction::Bleep => overlay(
            templates::BLEEP_TEMPLATE,
            said,
            Vec::<(&str, ParamValue)>::new(),
        ),
        Redaction::Blur { hold } => {
            let step = crate::derived::FACE_STEP.0;
            let at = Micros(said.from().0 / step * step);
            let lines = screen_lines(video_dir, take, &[at])?;
            let rect = highlight::find_on_screen(&lines[0].1, phrase)
                .ok_or_else(|| OverlayRuleError::NotOnScreen(phrase.to_owned()))?;
            let pad = rect.height() * 0.3;
            let range = TimeRange::new(said.from(), Micros(said.to().0 + hold.0))
                .expect("a phrase has a length");
            overlay(
                templates::BLUR_TEMPLATE,
                range,
                vec![
                    ("x", ParamValue::Integer((rect.left - pad).round() as i64)),
                    ("y", ParamValue::Integer((rect.top - pad).round() as i64)),
                    (
                        "width",
                        ParamValue::Integer((rect.width() + 2.0 * pad).round() as i64),
                    ),
                    (
                        "height",
                        ParamValue::Integer((rect.height() + 2.0 * pad).round() as i64),
                    ),
                ],
            )
        }
    };
    let mut cut = cut.clone();
    block_at(&mut cut, take, said.from())?.overlays.push(placed);
    Ok(cut)
}

/// `cut` with `file` picked for the b-roll of `take` that covers `at`.
pub fn fill_b_roll(cut: &Cut, take: &Id, at: Micros, file: &str) -> Result<Cut, OverlayRuleError> {
    let mut cut = cut.clone();
    let block = block_at(&mut cut, take, at)?;
    let slot = block
        .overlays
        .iter_mut()
        .find(|o| {
            o.template.as_str() == broll::TEMPLATE && o.range.from() <= at && at < o.range.to()
        })
        .ok_or_else(|| OverlayRuleError::NoSlot {
            take: take.clone(),
            at,
        })?;
    let file_id = Id::new("file").expect("an id");
    slot.params.retain(|(name, _)| *name != file_id);
    slot.params
        .insert(0, (file_id, ParamValue::Text(file.to_owned())));
    Ok(cut)
}

/// The take block of the main cut that keeps `take` at `at`.
fn block_at<'a>(
    cut: &'a mut Cut,
    take: &Id,
    at: Micros,
) -> Result<&'a mut TakeEdit, OverlayRuleError> {
    cut.chapters
        .iter_mut()
        .flat_map(|c| &mut c.takes)
        .find(|edit| edit.take == *take && edit_keeps(edit, at))
        .ok_or_else(|| OverlayRuleError::NotInCut {
            take: take.clone(),
            at,
        })
}
