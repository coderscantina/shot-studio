//! A script proposal as the Script screen shows it: inline struck and inserted words where the
//! change applies, read from the proposal's unified diff of `script.md`. The branch behind it
//! stays out of sight.

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

    /// The new text of the change, for hook options.
    pub fn added_text(&self) -> String {
        self.pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Added(text) | Piece::Same(text) => Some(text.as_str()),
                Piece::Removed(_) => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The changes in a unified diff of `script.md`, each a removed and added block word-diffed.
pub fn changes(patch: &str) -> Vec<Change> {
    let mut out = Vec::new();
    let mut old_line = 0;
    let mut block: Option<(usize, Vec<String>, Vec<String>)> = None;
    let flush = |block: &mut Option<(usize, Vec<String>, Vec<String>)>, out: &mut Vec<Change>| {
        if let Some((line, removed, added)) = block.take() {
            out.push(Change {
                line,
                end: line + removed.len(),
                pieces: word_diff(&strip(&removed), &strip(&added)),
            });
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

/// Joins lines into one text without Markdown bullet and heading marks.
fn strip(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            let l = l.trim();
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
    let mut push = |piece: Piece| match (pieces.last_mut(), piece) {
        (Some(Piece::Same(a)), Piece::Same(b))
        | (Some(Piece::Removed(a)), Piece::Removed(b))
        | (Some(Piece::Added(a)), Piece::Added(b)) => {
            a.push(' ');
            a.push_str(&b);
        }
        (_, piece) => pieces.push(piece),
    };
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
        assert_eq!(
            found[0].added_text(),
            "Editors don't die. They become review tools"
        );
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
