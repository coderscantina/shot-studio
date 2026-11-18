//! Overlays a person places from the transcript: [`redact`] places a bleep or a blur on words,
//! [`highlight_said`] a highlight on text read out on screen, and [`fill_b_roll`] picks the file
//! of a b-roll slot. Link cards, b-roll slots and highlights found by themselves are style rules.

use std::path::{Path, PathBuf};

use shot_analyze::vision::{self, VisionError};
use shot_core::{Cut, Id, Micros, Overlay, ParamValue, TakeAt, TakeEdit, TimeRange};

use crate::layout::Rect;
use crate::overlay::highlight::{self, ScreenLine};
use crate::overlay::{broll, overlay, redact as templates};

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

fn edit_keeps(edit: &TakeEdit, t: Micros) -> bool {
    TimeRange::new(t, Micros(t.0 + 1)).is_some_and(|r| edit.keeps(r))
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
    // Through the core, so every block of the take that plays part of the range is covered.
    cut.place_overlay(take, placed)
        .map_err(|_| OverlayRuleError::NotInCut {
            take: take.clone(),
            at: said.from(),
        })
}

/// A `highlight` around where `phrase`, said over `said` in `take`, shows on the screen when it
/// is said, held until [`highlight::AFTER`] past it. Vision reads the screen once, on the
/// keyframe at the start.
pub fn highlight_said(
    video_dir: &Path,
    take: &Id,
    said: TimeRange,
    phrase: &str,
) -> Result<Overlay, OverlayRuleError> {
    let step = crate::derived::FACE_STEP.0;
    let at = Micros(said.from().0 / step * step);
    let lines = screen_lines(video_dir, take, &[at])?;
    let rect = lines
        .first()
        .and_then(|(_, lines)| highlight::find_on_screen(lines, phrase))
        .ok_or_else(|| OverlayRuleError::NotOnScreen(phrase.to_owned()))?;
    let range = TimeRange::new(said.from(), Micros(said.to().0 + highlight::AFTER.0))
        .expect("a phrase has a length");
    let px = |v: f32| ParamValue::Integer(v.round() as i64);
    Ok(overlay(
        highlight::TEMPLATE,
        range,
        vec![
            ("x", px(rect.left)),
            ("y", px(rect.top)),
            ("width", px(rect.width())),
            ("height", px(rect.height())),
            ("text", ParamValue::Text(phrase.to_owned())),
        ],
    ))
}

/// `cut` with `file` picked for the b-roll of `take` that covers `at`.
pub fn fill_b_roll(cut: &Cut, take: &Id, at: Micros, file: &str) -> Result<Cut, OverlayRuleError> {
    let not_in_cut = || OverlayRuleError::NotInCut {
        take: take.clone(),
        at,
    };
    let (block, edit) = cut
        .chapters
        .iter()
        .enumerate()
        .flat_map(|(chapter, c)| {
            let blocks = c.takes.iter().enumerate();
            blocks.map(move |(take, edit)| (TakeAt { chapter, take }, edit))
        })
        .find(|(_, edit)| edit.take == *take && edit_keeps(edit, at))
        .ok_or_else(not_in_cut)?;
    let index = edit
        .overlays
        .iter()
        .position(|o| {
            o.template.as_str() == broll::TEMPLATE && o.range.from() <= at && at < o.range.to()
        })
        .ok_or_else(|| OverlayRuleError::NoSlot {
            take: take.clone(),
            at,
        })?;
    let mut slot = edit.overlays[index].clone();
    let file_id = Id::new("file").expect("an id");
    slot.params.retain(|(name, _)| *name != file_id);
    slot.params
        .insert(0, (file_id, ParamValue::Text(file.to_owned())));
    // Through the core, so the slot's copies across a splice get the file too.
    Ok(cut.set_overlay(block, index, Some(slot)))
}
