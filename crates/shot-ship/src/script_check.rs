//! Checks on `script.md` before recording: hook length, spoken length, sources and visuals.
//!
//! Spoken time is estimated from the words a beat will say: its full script when it has one,
//! else the beat itself, at [`WORDS_PER_MINUTE`].

use shot_core::{Beat, Micros, Script};

/// A relaxed speaking pace for talking-head videos.
pub const WORDS_PER_MINUTE: u64 = 150;
/// A hook should land within this.
pub const HOOK_MAX: Micros = Micros(15_000_000);
/// Talking longer than this without a visual loses the viewer.
pub const VISUAL_GAP_MAX: Micros = Micros(90_000_000);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptReport {
    /// Estimated spoken length per chapter, in script order.
    pub chapters: Vec<Micros>,
    pub total: Micros,
    /// The first chapter's spoken length, `None` without chapters.
    pub hook: Option<Micros>,
    /// Problems only, passing checks stay quiet.
    pub findings: Vec<ScriptFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptFinding {
    /// The first chapter takes longer than [`HOOK_MAX`] to say.
    LongHook { spoken: Micros },
    /// A beat links no source.
    NoSource { chapter: String, line: usize },
    /// A run of beats from `line` talks for `spoken` without a source or b-roll note.
    NoVisual {
        chapter: String,
        line: usize,
        spoken: Micros,
    },
}

impl std::fmt::Display for ScriptFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LongHook { spoken } => write!(
                f,
                "the hook takes {} to say, aim for under {} s",
                crate::clock(*spoken),
                HOOK_MAX.0 / 1_000_000
            ),
            Self::NoSource { chapter, line } => {
                write!(f, "script.md:{line}: a beat in {chapter} has no source")
            }
            Self::NoVisual {
                chapter,
                line,
                spoken,
            } => write!(
                f,
                "script.md:{line}: {chapter} runs {} without a visual, add b-roll notes",
                crate::clock(*spoken)
            ),
        }
    }
}

/// Estimates and checks a script.
pub fn check_script(script: &Script) -> ScriptReport {
    let mut findings = Vec::new();
    let chapters: Vec<Micros> = script
        .chapters
        .iter()
        .map(|c| Micros(c.beats.iter().map(|b| spoken(b).0).sum()))
        .collect();
    let hook = chapters.first().copied();
    if let Some(spoken) = hook.filter(|h| *h > HOOK_MAX) {
        findings.push(ScriptFinding::LongHook { spoken });
    }
    for chapter in &script.chapters {
        let mut run: Option<(usize, u64)> = None;
        let flush = |run: &mut Option<(usize, u64)>, findings: &mut Vec<ScriptFinding>| {
            if let Some((line, spoken)) = run.take().filter(|(_, s)| *s > VISUAL_GAP_MAX.0) {
                findings.push(ScriptFinding::NoVisual {
                    chapter: chapter.title.clone(),
                    line,
                    spoken: Micros(spoken),
                });
            }
        };
        for beat in &chapter.beats {
            if beat.sources.is_empty() && !is_visual_note(beat) {
                findings.push(ScriptFinding::NoSource {
                    chapter: chapter.title.clone(),
                    line: beat.line,
                });
            }
            if is_visual(beat) {
                flush(&mut run, &mut findings);
            } else {
                let (_, total) = run.get_or_insert((beat.line, 0));
                *total += spoken(beat).0;
            }
        }
        flush(&mut run, &mut findings);
    }
    ScriptReport {
        total: Micros(chapters.iter().map(|c| c.0).sum()),
        chapters,
        hook,
        findings,
    }
}

/// The estimated time to say a beat.
pub fn spoken(beat: &Beat) -> Micros {
    let text = if beat.script.is_empty() {
        &beat.text
    } else {
        &beat.script
    };
    if is_visual_note(beat) && beat.script.is_empty() {
        return Micros(0);
    }
    let words = text.split_whitespace().count() as u64;
    Micros(words * 60_000_000 / WORDS_PER_MINUTE)
}

/// A beat shows something besides the face: it links a source or is a b-roll note.
fn is_visual(beat: &Beat) -> bool {
    !beat.sources.is_empty() || is_visual_note(beat)
}

/// A beat that describes a visual instead of speech, `B-roll: ...` or `Visual: ...`.
fn is_visual_note(beat: &Beat) -> bool {
    let lower = beat.text.trim_start().to_lowercase();
    ["b-roll:", "b-roll ", "visual:", "demo:"]
        .iter()
        .any(|p| lower.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_spoken_length_and_stays_quiet_on_a_good_script() {
        let script = Script::parse(
            "# A\n\n## Hook\n\n- Everyone says the IDE is dead. [post](sources/a.md)\n\n## Body\n\n- One [pr](sources/pr.md)\n- B-roll: diff scrolling\n",
        )
        .unwrap();
        let report = check_script(&script);
        assert_eq!(report.hook, Some(Micros(2_800_000)));
        assert_eq!(report.total, Micros(3_600_000));
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn flags_a_long_hook_a_beat_without_source_and_a_long_stretch_without_visual() {
        let long = "word ".repeat(250);
        let script = Script::parse(&format!(
            "# A\n\n## Hook\n\n- {long}\n\n## Body\n\n- talk\n  {long}\n- more\n  {long}\n- B-roll: the diff\n"
        ))
        .unwrap();
        let findings = check_script(&script).findings;
        assert!(matches!(findings[0], ScriptFinding::LongHook { .. }));
        assert!(
            findings
                .iter()
                .any(|f| matches!(f, ScriptFinding::NoSource { line: 9, .. }))
        );
        assert!(findings.iter().any(|f| matches!(
            f,
            ScriptFinding::NoVisual { line: 9, spoken, .. } if *spoken == Micros(200_000_000)
        )));
    }
}
