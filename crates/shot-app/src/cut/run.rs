//! Running the auto-cut rules from the app: after a take is appended, and again whenever a rule
//! switch or the pacing changes. Reads the cached transcript and speech of each take and the
//! take folder's screen and markers, and writes `format.kdl` and `cut.kdl` through
//! [`VideoFolder`]. Only for a video before migration: a migrated video's rough cut is its lock,
//! built from the elements by the style rules.

use std::fs;

use shot_analyze::autocut::{self, TakeFacts, TakeFiles};
use shot_analyze::{Speech, cache};
use shot_core::{Cut, Format, Id, Micros, Pacing, RuleKind};

use crate::folder::VideoFolder;

/// Runs the rules over every block of `take` and writes the rough cut.
pub fn rough_cut(folder: &mut VideoFolder, take: &Id) -> Result<(), String> {
    if folder.is_migrated() {
        return Ok(());
    }
    let cut = rough_cut_of(folder, &folder.project.cut, take)?;
    folder.write_cut(cut)
}

/// Switches `rule` on or off in `format.kdl` and runs the rules again.
pub fn set_rule(folder: &mut VideoFolder, rule: RuleKind, on: bool) -> Result<(), String> {
    let mut format = folder.project.format.clone();
    let rules = &mut format.auto_cut;
    match rule {
        RuleKind::Silence => rules.silence.on = on,
        RuleKind::Retake => rules.retake = on,
        RuleKind::Misspoken => rules.misspoken = on,
        RuleKind::Pause => rules.pause.on = on,
        RuleKind::Filler => rules.filler = on,
        RuleKind::IdleScreen => rules.idle_screen.on = on,
    }
    rerun(folder, format)
}

/// Sets the pacing in `format.kdl` and moves the cut edges with it.
pub fn set_pacing(folder: &mut VideoFolder, pacing: Pacing) -> Result<(), String> {
    let mut format = folder.project.format.clone();
    format.pacing = pacing;
    rerun(folder, format)
}

pub fn rule_on(format: &Format, rule: RuleKind) -> bool {
    let rules = &format.auto_cut;
    match rule {
        RuleKind::Silence => rules.silence.on,
        RuleKind::Retake => rules.retake,
        RuleKind::Misspoken => rules.misspoken,
        RuleKind::Pause => rules.pause.on,
        RuleKind::Filler => rules.filler,
        RuleKind::IdleScreen => rules.idle_screen.on,
    }
}

fn rerun(folder: &mut VideoFolder, format: Format) -> Result<(), String> {
    if folder.is_migrated() {
        return Err("this video's cuts come from the style rules of its format".into());
    }
    let path = folder.dir.join("format.kdl");
    fs::write(&path, format.to_kdl()).map_err(|e| format!("{}: {e}", path.display()))?;
    folder.project.format = format;
    let mut takes: Vec<Id> = Vec::new();
    for block in folder.project.cut.chapters.iter().flat_map(|c| &c.takes) {
        if !takes.contains(&block.take) {
            takes.push(block.take.clone());
        }
    }
    let mut cut = folder.project.cut.clone();
    for take in &takes {
        cut = rough_cut_of(folder, &cut, take)?;
    }
    folder.write_cut(cut)
}

fn rough_cut_of(folder: &VideoFolder, cut: &Cut, take: &Id) -> Result<Cut, String> {
    let Some(transcript) = folder.transcripts.get(take) else {
        // Without words the rules cannot tell a silence from a sentence end.
        return Ok(cut.clone());
    };
    let speech = cache::read_speech(&folder.dir, take)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let format = &folder.project.format;
    let files = TakeFiles::read(
        &folder.dir.join("media").join(take.as_str()),
        format.auto_cut.idle_screen.min,
    )
    .map_err(|e| e.to_string())?;
    let facts = TakeFacts {
        transcript,
        speech: &speech,
        still: &files.still,
        again: &files.again,
        duration: take_end(cut, take, &speech),
    };
    Ok(autocut::rough_cut(cut, format, &facts, &folder.transcripts))
}

/// Where the take ends: the furthest range of its blocks, or its speech levels.
fn take_end(cut: &Cut, take: &Id, speech: &Speech) -> Micros {
    let blocks = cut
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .filter(|b| b.take == *take)
        .filter_map(|b| b.ranges.last().map(|r| r.range().to()));
    let levels =
        Micros(speech.levels_from.0 + speech.levels.len() as u64 * shot_analyze::LEVEL_STEP.0);
    blocks.fold(levels, Micros::max)
}
