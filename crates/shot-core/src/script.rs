use crate::ParseError;

/// `script.md`: the spec. `#` is the title, `##` a chapter, a bullet a beat, and paragraphs
/// after a beat are its full script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub title: String,
    pub chapters: Vec<ScriptChapter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptChapter {
    pub title: String,
    /// Text between the heading and the first beat. Lines join with spaces, paragraphs with a
    /// newline.
    pub note: String,
    pub beats: Vec<Beat>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beat {
    /// 1-based line of the bullet.
    pub line: usize,
    /// The bullet text with links replaced by their labels, as the prompter shows it.
    pub text: String,
    /// Link targets in the bullet, usually files in `sources/`.
    pub sources: Vec<String>,
    /// The spoken text for full-script mode. Lines join with spaces, paragraphs with a newline.
    pub script: String,
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let error = |line: usize, message: &str| ParseError {
            file: "script.md",
            line,
            column: 1,
            message: message.to_owned(),
        };
        let mut title = None;
        let mut chapters: Vec<ScriptChapter> = Vec::new();
        let mut paragraph_break = false;
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                paragraph_break = true;
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix('#') {
                let (level, heading) = match rest.strip_prefix('#') {
                    Some(rest) => (2, rest),
                    None => (1, rest),
                };
                let Some(heading) = heading
                    .strip_prefix(' ')
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                else {
                    return Err(error(line, "headings are `# title` or `## chapter`"));
                };
                match (level, &title) {
                    (1, None) => title = Some(heading.to_owned()),
                    (1, Some(_)) => return Err(error(line, "the title (`#`) appears once")),
                    (_, None) => {
                        return Err(error(
                            line,
                            "the title (`#`) comes before the first chapter",
                        ));
                    }
                    _ => chapters.push(ScriptChapter {
                        title: heading.to_owned(),
                        note: String::new(),
                        beats: Vec::new(),
                    }),
                }
            } else if let Some(bullet) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                let chapter = chapters
                    .last_mut()
                    .ok_or_else(|| error(line, "a beat belongs to a chapter (`##`)"))?;
                let (text, sources) = split_links(bullet.trim());
                chapter.beats.push(Beat {
                    line,
                    text,
                    sources,
                    script: String::new(),
                });
            } else {
                let chapter = chapters
                    .last_mut()
                    .ok_or_else(|| error(line, "text belongs to a chapter (`##`)"))?;
                let text = match chapter.beats.last_mut() {
                    Some(beat) => &mut beat.script,
                    None => &mut chapter.note,
                };
                if !text.is_empty() {
                    text.push(if paragraph_break { '\n' } else { ' ' });
                }
                text.push_str(trimmed);
            }
            paragraph_break = false;
        }
        let title = title.ok_or_else(|| error(1, "missing the title (`# ...`)"))?;
        Ok(Self { title, chapters })
    }
}

/// Replaces `[label](target)` links by their label and collects the targets.
fn split_links(text: &str) -> (String, Vec<String>) {
    let mut plain = String::with_capacity(text.len());
    let mut sources = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let link = rest[open + 1..]
            .split_once("](")
            .and_then(|(label, after)| {
                let (target, tail) = after.split_once(')')?;
                (!label.contains('[')).then_some((label, target, tail))
            });
        let Some((label, target, tail)) = link else {
            break;
        };
        plain.push_str(&rest[..open]);
        plain.push_str(label);
        sources.push(target.to_owned());
        rest = tail;
    }
    plain.push_str(rest);
    (plain, sources)
}
