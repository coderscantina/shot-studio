//! A draft title and description for the upload: the hook as the opening line, the chapters at
//! the moment their first word is heard, and the links the video shows or the script cites.
//! A person edits it before shipping, Claude may rewrite it as text.

use std::collections::HashMap;
use std::fmt::Write;

use shot_core::{Id, Output, ParamValue, Project, Transcript};

use crate::spoken::{chapter_marks, clock, spoken_words};

/// The template whose `url` parameter is a link card.
pub const LINK_CARD: &str = "link-card";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    pub title: String,
    pub text: String,
}

/// Drafts the description of `main`, the main output as export renders it (see
/// [`crate::lock::timeline`]).
pub fn describe(
    project: &Project,
    main: &Output,
    transcripts: &HashMap<Id, Transcript>,
) -> Description {
    let words = spoken_words(main, transcripts);
    let mut text = String::new();
    if let Some(hook) = project
        .script
        .chapters
        .first()
        .and_then(|c| c.beats.first())
    {
        let _ = writeln!(text, "{}\n", hook.text.trim());
    }
    let marks = chapter_marks(main, &words);
    if !marks.is_empty() {
        text.push_str("Chapters\n");
        for mark in &marks {
            let _ = writeln!(text, "{} {}", clock(mark.at), mark.title);
        }
    }
    let links = links(project, main);
    if !links.is_empty() {
        text.push_str("\nLinks\n");
        for link in links {
            let _ = writeln!(text, "{link}");
        }
    }
    Description {
        title: project.video.title.clone(),
        text,
    }
}

/// Web links from link cards in the output, then from the script's beats, each once.
fn links(project: &Project, main: &Output) -> Vec<String> {
    let cards = main
        .chapters
        .iter()
        .flat_map(|c| &c.segments)
        .flat_map(|s| &s.overlays)
        .filter(|o| o.template.as_str() == LINK_CARD)
        .filter_map(
            |o| match o.params.iter().find(|(name, _)| name.as_str() == "url") {
                Some((_, ParamValue::Text(url))) => Some(url.clone()),
                _ => None,
            },
        );
    let cited = project
        .script
        .chapters
        .iter()
        .flat_map(|c| &c.beats)
        .flat_map(|b| b.sources.iter().cloned())
        .filter(|s| s.starts_with("https://") || s.starts_with("http://"));
    let mut seen = Vec::new();
    for link in cards.chain(cited) {
        if !seen.contains(&link) {
            seen.push(link);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spoken::tests::{project, transcript};
    use shot_core::{Script, Timeline};

    #[test]
    fn drafts_hook_chapters_and_links() {
        let mut project = project(
            "chapter \"Hook\" {\n    take t01 {\n        keep from=0 to=4_000_000\n    }\n}\n\nchapter \"Why\" {\n    take t01 {\n        keep from=4_000_000 to=70_000_000\n        overlay link-card from=5_000_000 to=9_000_000 {\n            url \"https://github.com/a/b/pull/1\"\n        }\n    }\n}\n",
        );
        project.script = Script::parse(
            "# A\n\n## Hook\n\n- Never on a Friday? [post](https://example.com/post)\n",
        )
        .unwrap();
        let transcripts =
            transcript(&[("never", 300_000, 600_000), ("so", 65_200_000, 65_500_000)]);
        let description = describe(&project, Timeline::evaluate(&project).main(), &transcripts);
        assert_eq!(
            description.text,
            "Never on a Friday? post\n\nChapters\n0:00 Hook\n1:05 Why\n\nLinks\nhttps://github.com/a/b/pull/1\nhttps://example.com/post\n"
        );
    }
}
