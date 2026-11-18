//! The takes of a video folder read from its files and lined up, for composing the cut outside
//! the app ([0040]).
//!
//! [0040]: ../../../docs/decisions/0040-the-cut-proposes-itself.md

use std::path::Path;

use shot_core::{Cut, Elements, Id, Lineup, Project, TakeWords, Transcript};

/// The takes in the video's media folder, also the ones the cut no longer uses.
pub fn media_takes(dir: &Path) -> Vec<Id> {
    std::fs::read_dir(dir.join("media"))
        .map(|entries| {
            entries
                .filter_map(|e| Id::new(e.ok()?.file_name().into_string().ok()?))
                .collect()
        })
        .unwrap_or_default()
}

/// A take's words from the transcript cache and its elements file. `None` while the take has
/// no transcript.
pub fn read_take(dir: &Path, id: &Id) -> Option<(Transcript, Option<Elements>)> {
    let transcript = shot_analyze::cache::read_cached(dir, id).ok().flatten()?;
    let elements = std::fs::read_to_string(dir.join(format!("elements/{id}.jsonl")))
        .ok()
        .and_then(|text| Elements::parse(id.clone(), &text).ok());
    Some((transcript, elements))
}

/// Every take of the video at `dir` lined up against its script.
pub fn lineup(dir: &Path, project: &Project) -> Lineup {
    let takes: Vec<_> = media_takes(dir)
        .iter()
        .filter_map(|id| read_take(dir, id))
        .collect();
    let words: Vec<TakeWords<'_>> = takes
        .iter()
        .map(|(transcript, elements)| TakeWords {
            transcript,
            elements: elements.as_ref(),
        })
        .collect();
    Lineup::new(&project.script, &project.cut, &words)
}

/// The cut of the video at `dir` composed from its takes, see [`Cut::compose`]. `new` is a take
/// just appended whole.
pub fn compose(dir: &Path, project: &Project, new: Option<&Id>) -> Result<Cut, String> {
    project
        .cut
        .compose(&lineup(dir, project), new)
        .map_err(|e| e.to_string())
}
