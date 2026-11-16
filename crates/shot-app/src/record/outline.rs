//! What the Record screen's outline knows: the script's chapters with the takes that recorded
//! them, every take of the media folder, and the part of the script the next take records.

use std::collections::BTreeSet;

use shot_core::{Elements, Id, Lineup, Micros, Script, TakeWords, Transcript};

use crate::folder::VideoFolder;

/// The part of the script the next take records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Part {
    #[default]
    Script,
    /// A chapter, from beat `from` (counted in the chapter, from 0) to its end.
    Chapter { chapter: usize, from: usize },
}

impl Part {
    /// The beats this part records, by index across the script, `None` for a script without
    /// beats.
    pub fn beats(self, script: &Script) -> Option<std::ops::RangeInclusive<usize>> {
        let total: usize = script.chapters.iter().map(|c| c.beats.len()).sum();
        if total == 0 {
            return None;
        }
        match self {
            Self::Script => Some(0..=total - 1),
            Self::Chapter { chapter, from } => {
                let before: usize = script.chapters[..chapter.min(script.chapters.len())]
                    .iter()
                    .map(|c| c.beats.len())
                    .sum();
                let beats = script.chapters.get(chapter)?.beats.len();
                (beats > 0).then(|| before + from.min(beats - 1)..=before + beats - 1)
            }
        }
    }

    /// The part as the stage names it: "Whole script", "Hook" or "Hook, from beat 2".
    pub fn name(self, script: &Script) -> String {
        match self {
            Self::Script => "Whole script".into(),
            Self::Chapter { chapter, from } => {
                let title = script
                    .chapters
                    .get(chapter)
                    .map_or("Chapter", |c| c.title.as_str());
                if from == 0 {
                    title.to_owned()
                } else {
                    format!("{title}, from beat {}", from + 1)
                }
            }
        }
    }
}

/// A take of the media folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Take {
    pub id: Id,
    /// How long it runs, as far as its words and the cut know.
    pub length: Micros,
    /// The chapters it has attempts at, by index.
    pub chapters: BTreeSet<usize>,
    /// Whether the cut plays any of it.
    pub in_cut: bool,
    /// Whether its words are known yet; a take without the mic has none.
    pub transcribed: bool,
}

/// A chapter of the script and how far its takes got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chapter {
    /// Takes with an attempt at any of its beats, oldest first.
    pub takes: Vec<Id>,
    /// Beats that have an attempt.
    pub covered: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outline {
    pub chapters: Vec<Chapter>,
    /// Every take, oldest first.
    pub takes: Vec<Take>,
}

impl Outline {
    /// Reads every take of `folder`'s media and lines them up against the script.
    pub fn read(folder: &VideoFolder) -> Self {
        let mut ids = crate::takes::media_takes(&folder.dir);
        ids.sort_by_key(|id| (id.as_str().len(), id.to_string()));
        let words: Vec<(Id, Transcript, Option<Elements>)> = ids
            .iter()
            .filter_map(|id| {
                let (transcript, elements) = crate::takes::read_take(folder, id)?;
                Some((id.clone(), transcript, elements))
            })
            .collect();
        let lineup = Lineup::new(
            &folder.project.script,
            &folder.project.cut,
            &words
                .iter()
                .map(|(_, transcript, elements)| TakeWords {
                    transcript,
                    elements: elements.as_ref(),
                })
                .collect::<Vec<_>>(),
        );
        Self::of(folder, &ids, &words, &lineup)
    }

    fn of(
        folder: &VideoFolder,
        ids: &[Id],
        words: &[(Id, Transcript, Option<Elements>)],
        lineup: &Lineup,
    ) -> Self {
        let cut = &folder.project.cut;
        let has_beats = folder
            .project
            .script
            .chapters
            .iter()
            .any(|c| !c.beats.is_empty());
        let mut chapters = vec![Chapter::default(); folder.project.script.chapters.len()];
        if has_beats {
            for (index, group) in lineup.groups.iter().enumerate() {
                let Some(chapter) = chapters.get_mut(index) else {
                    break;
                };
                for row in &group.rows {
                    chapter.covered += usize::from(!row.attempts.is_empty());
                    for attempt in &row.attempts {
                        if !chapter.takes.contains(&attempt.take) {
                            chapter.takes.push(attempt.take.clone());
                        }
                    }
                }
                chapter
                    .takes
                    .sort_by_key(|id| ids.iter().position(|i| i == id));
            }
        }
        let takes = ids
            .iter()
            .map(|id| {
                let transcript = words.iter().find(|(i, ..)| i == id).map(|(_, t, _)| t);
                let heard = transcript
                    .and_then(|t| t.words().last())
                    .map_or(Micros(0), |w| w.range.to());
                let blocks = cut
                    .chapters
                    .iter()
                    .flat_map(|c| &c.takes)
                    .filter(|edit| edit.take == *id);
                let edited = blocks
                    .clone()
                    .flat_map(|edit| &edit.ranges)
                    .map(|r| r.range().to())
                    .max()
                    .unwrap_or(Micros(0));
                Take {
                    id: id.clone(),
                    length: heard.max(edited),
                    chapters: chapters
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.takes.contains(id))
                        .map(|(i, _)| i)
                        .collect(),
                    in_cut: blocks.count() > 0,
                    transcribed: transcript.is_some(),
                }
            })
            .collect();
        Self { chapters, takes }
    }

    /// The first chapter after `chapter` that no take recorded yet, for the next take.
    pub fn next_open(&self, after: usize) -> Option<usize> {
        (after + 1..self.chapters.len()).find(|&c| self.chapters[c].takes.is_empty())
    }
}

/// "1:42" for a take's length.
pub fn length(t: Micros) -> String {
    let seconds = t.0 / 1_000_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "# T\n\n## Hook\n\n- One\n- Two\n\n## Demo\n\n- Three\n";

    #[test]
    fn a_part_names_the_beats_it_records() {
        let script = Script::parse(SCRIPT).unwrap();
        assert_eq!(Part::Script.beats(&script), Some(0..=2));
        assert_eq!(
            Part::Chapter {
                chapter: 0,
                from: 1
            }
            .beats(&script),
            Some(1..=1)
        );
        assert_eq!(
            Part::Chapter {
                chapter: 1,
                from: 0
            }
            .beats(&script),
            Some(2..=2)
        );
        assert_eq!(
            Part::Chapter {
                chapter: 0,
                from: 1
            }
            .name(&script),
            "Hook, from beat 2"
        );
        let none = Script::parse("# T\n\n## Hook\n").unwrap();
        assert_eq!(Part::Script.beats(&none), None);
    }
}
