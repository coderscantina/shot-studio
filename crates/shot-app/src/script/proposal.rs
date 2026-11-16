//! A script proposal as the Script screen shows it: inline struck and inserted words where the
//! change applies, read from the proposal's unified diff of `script.md`, one change per chapter
//! it touches. The branch behind it stays out of sight.

/// One run of words in an inline proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Same(String),
    Removed(String),
    Added(String),
}

/// One changed stretch of `script.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The 1-based line in the script the change starts at, on the script's side.
    pub line: usize,
    /// The line after the last one it replaces, `line` when it only inserts.
    pub end: usize,
    /// The chapter heading the change starts with, if it does.
    pub chapter: Option<String>,
    pub pieces: Vec<Piece>,
}

impl Change {
    /// Words removed minus words added, the spoken words the change saves. B-roll notes are
    /// not spoken.
    pub fn words_saved(&self) -> isize {
        let spoken = |text: &str| {
            if text.to_lowercase().starts_with("b-roll") {
                0
            } else {
                text.split_whitespace().count() as isize
            }
        };
        self.pieces
            .iter()
            .map(|p| match p {
                Piece::Removed(text) => spoken(text),
                Piece::Added(text) => -spoken(text),
                Piece::Same(_) => 0,
            })
            .sum()
    }
}

/// The changes in a unified diff of `script.md`, each a removed and added block word-diffed.
pub fn changes(patch: &str) -> Vec<Change> {
    let mut out = Vec::new();
    let mut old_line = 0;
    let mut block: Option<(usize, Vec<String>, Vec<String>)> = None;
    let flush = |block: &mut Option<(usize, Vec<String>, Vec<String>)>, out: &mut Vec<Change>| {
        if let Some((line, removed, added)) = block.take() {
            out.extend(by_chapter(line, &removed, &added));
        }
    };
    for text in patch.lines() {
        if let Some(header) = text.strip_prefix("@@ -") {
            flush(&mut block, &mut out);
            old_line = header
                .split([',', ' '])
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
        } else if text.starts_with("---") || text.starts_with("+++") || text.starts_with("diff ") {
        } else if let Some(removed) = text.strip_prefix('-') {
            block
                .get_or_insert_with(|| (old_line, Vec::new(), Vec::new()))
                .1
                .push(removed.to_owned());
            old_line += 1;
        } else if let Some(added) = text.strip_prefix('+') {
            block
                .get_or_insert_with(|| (old_line, Vec::new(), Vec::new()))
                .2
                .push(added.to_owned());
        } else if text.starts_with(' ') || text.is_empty() {
            flush(&mut block, &mut out);
            old_line += 1;
        }
    }
    flush(&mut block, &mut out);
    out.retain(|c| !c.pieces.is_empty());
    out
}

/// One change per chapter of a changed block, so a rewrite of the whole script reads chapter by
/// chapter. The removed and the added lines are cut before each `## ` heading and paired in order.
fn by_chapter(line: usize, removed: &[String], added: &[String]) -> Vec<Change> {
    let (old, new) = (chapters(removed), chapters(added));
    (0..old.len().max(new.len()))
        .map(|i| {
            let (start, gone) = old.get(i).copied().unwrap_or((removed.len(), &[]));
            let came = new.get(i).map_or(&[][..], |(_, lines)| lines);
            Change {
                line: line + start,
                end: line + start + gone.len(),
                chapter: [came, gone]
                    .iter()
                    .find_map(|lines| heading(lines.first()?))
                    .filter(|title| !title.is_empty())
                    .map(str::to_owned),
                pieces: word_diff(&strip(gone), &strip(came)),
            }
        })
        .collect()
}

/// The runs of `lines` that start at each `## ` heading, with their offsets, after the run of
/// lines before the first heading, which may be empty.
fn chapters(lines: &[String]) -> Vec<(usize, &[String])> {
    let mut starts = vec![0];
    starts.extend((0..lines.len()).filter(|&i| heading(&lines[i]).is_some()));
    let ends = starts.iter().skip(1).copied().chain([lines.len()]);
    starts
        .iter()
        .zip(ends)
        .map(|(&start, end)| (start, &lines[start..end]))
        .collect()
}

/// The title of a `## ` chapter heading, empty for a chapter without one yet.
pub fn heading(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("##")?;
    (rest.is_empty() || rest.starts_with(' ')).then(|| rest.trim())
}

/// Joins lines into one text without Markdown bullet and heading marks.
fn strip(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            let l = l.trim_start();
            l.strip_prefix("- ")
                .or_else(|| l.strip_prefix("* "))
                .or_else(|| l.strip_prefix("## "))
                .or_else(|| l.strip_prefix("# "))
                .unwrap_or(l)
        })
        .flat_map(|l| l.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .collect()
}

/// A word diff by longest common subsequence, merged into runs. When less than half of the shorter
/// text stays, the old text is struck whole and the new one follows: easier to read than a patchwork.
fn word_diff(old: &[String], new: &[String]) -> Vec<Piece> {
    let pieces = lcs_diff(old, new);
    let kept: usize = pieces
        .iter()
        .map(|p| match p {
            Piece::Same(text) => text.split_whitespace().count(),
            _ => 0,
        })
        .sum();
    if kept * 2 >= old.len().min(new.len()).max(1) {
        return pieces;
    }
    [
        (!old.is_empty()).then(|| Piece::Removed(old.join(" "))),
        (!new.is_empty()).then(|| Piece::Added(new.join(" "))),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Appends `piece`, joining it to the last run when they are the same kind.
pub fn push(pieces: &mut Vec<Piece>, piece: Piece) {
    match (pieces.last_mut(), piece) {
        (Some(Piece::Same(a)), Piece::Same(b))
        | (Some(Piece::Removed(a)), Piece::Removed(b))
        | (Some(Piece::Added(a)), Piece::Added(b)) => {
            a.push(' ');
            a.push_str(&b);
        }
        (_, piece) => pieces.push(piece),
    }
}

fn lcs_diff(old: &[String], new: &[String]) -> Vec<Piece> {
    let (n, m) = (old.len(), new.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut push = |piece: Piece| push(&mut pieces, piece);
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            push(Piece::Same(old[i].clone()));
            (i, j) = (i + 1, j + 1);
        } else if j < m && (i == n || lcs[i][j + 1] > lcs[i + 1][j]) {
            push(Piece::Added(new[j].clone()));
            j += 1;
        } else {
            push(Piece::Removed(old[i].clone()));
            i += 1;
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_changed_beat_as_struck_and_inserted_words_at_its_line() {
        let patch = "--- a/script.md\n+++ b/script.md\n@@ -9,3 +9,3 @@ \n - Demo: reviewing PR 218\n-- Editors turn into review tools\n+- Editors don't die. They become review tools\n - The diff is the new file tree\n";
        let found = changes(patch);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].line, found[0].end), (10, 11));
        assert_eq!(
            found[0].pieces,
            [
                Piece::Same("Editors".into()),
                Piece::Removed("turn into".into()),
                Piece::Added("don't die. They become".into()),
                Piece::Same("review tools".into()),
            ]
        );
        assert_eq!(found[0].words_saved(), -2);
    }

    #[test]
    fn a_whole_script_draft_reads_as_one_change_per_chapter() {
        let patch = "@@ -1,3 +1,9 @@\n # Title\n \n-## \n+## Hook\n+\n+- Most editing is boring\n+\n+## Body\n+\n+- One\n";
        let found = changes(patch);
        let chapters: Vec<_> = found.iter().map(|c| c.chapter.as_deref()).collect();
        assert_eq!(chapters, [Some("Hook"), Some("Body")]);
        assert_eq!((found[0].line, found[0].end), (3, 4));
        assert_eq!((found[1].line, found[1].end), (4, 4));
        assert_eq!(found[1].pieces, [Piece::Added("Body One".into())]);
    }

    #[test]
    fn a_rewrite_strikes_the_old_text_whole() {
        let patch = "@@ -5,1 +5,1 @@\n-- Welcome, and what this video covers\n+- Most editing is boring work\n";
        assert_eq!(
            changes(patch)[0].pieces,
            [
                Piece::Removed("Welcome, and what this video covers".into()),
                Piece::Added("Most editing is boring work".into()),
            ]
        );
    }
}
