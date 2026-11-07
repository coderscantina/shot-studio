//! The `code-diff` overlay template: a diff snapshot from `sources/` typed out over the overlay's
//! range, with one line highlighted from the moment the voice reaches it.
//!
//! Parameters, all owned by this template:
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `diff` | the snapshot, a unified diff relative to the video folder, usually `sources/<name>.diff` |
//! | `highlight-line` | optional, the line of the snapshot file to highlight, counted from 1 |
//! | `highlight-at` | optional, the media time the highlight starts, by default the overlay's start |
//!
//! [`snapshot`] writes the snapshot of a commit range, [`phrase_at`] finds the moment a phrase is
//! spoken in a transcript.

use std::path::Path;

use shot_core::{Id, Micros, Overlay, ParamValue, TimeRange, Word};

use crate::drawing::{Font, Rgba, Shape};
use crate::layout::{Canvas, Rect};

/// The template name in `cut.kdl`.
pub const TEMPLATE: &str = "code-diff";

/// The share of the overlay's range spent typing; the rest holds the full diff.
const TYPING_SHARE: f32 = 0.6;
/// The panel fades in and out over this long.
const FADE: f32 = 250_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    File,
    Hunk,
    Context,
    Added,
    Removed,
}

/// A line of the snapshot the panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// The line of the snapshot file, from 1.
    pub number: usize,
    pub kind: LineKind,
    pub text: String,
}

/// A parsed snapshot: the lines worth showing, without git's headers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodeDiff {
    pub lines: Vec<DiffLine>,
}

impl CodeDiff {
    pub fn parse(text: &str) -> Self {
        let mut lines = Vec::new();
        let mut removed_file = None;
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let (kind, text) = if let Some(path) = line.strip_prefix("--- ") {
                removed_file = path.strip_prefix("a/").map(str::to_owned);
                continue;
            } else if let Some(path) = line.strip_prefix("+++ ") {
                let path = path
                    .strip_prefix("b/")
                    .map(str::to_owned)
                    .or(removed_file.take())
                    .unwrap_or_else(|| path.to_owned());
                (LineKind::File, path)
            } else if line.starts_with("@@") {
                (LineKind::Hunk, line.to_owned())
            } else if line.starts_with('+') {
                (LineKind::Added, line.to_owned())
            } else if line.starts_with('-') {
                (LineKind::Removed, line.to_owned())
            } else if line.starts_with(' ') {
                (LineKind::Context, line.to_owned())
            } else {
                continue;
            };
            lines.push(DiffLine { number, kind, text });
        }
        Self { lines }
    }
}

/// The unified diff of the commit range `from..to` of the repository at `repo`, as a snapshot.
pub fn snapshot(repo: &Path, range: &str) -> Result<String, git2::Error> {
    let repo = git2::Repository::open(repo)?;
    let (from, to) = range
        .split_once("..")
        .ok_or_else(|| git2::Error::from_str("a commit range reads `from..to`"))?;
    let tree = |rev: &str| repo.revparse_single(rev).and_then(|o| o.peel_to_tree());
    let diff = repo.diff_tree_to_tree(Some(&tree(from)?), Some(&tree(to)?), None)?;
    let mut out = String::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            out.push(line.origin());
        }
        out.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    Ok(out)
}

/// When the first run of transcript words reading `phrase` starts, ignoring case and punctuation.
pub fn phrase_at(words: &[Word], phrase: &str) -> Option<Micros> {
    let key = |text: &str| -> String {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let wanted: Vec<String> = phrase
        .split_whitespace()
        .map(key)
        .filter(|w| !w.is_empty())
        .collect();
    if wanted.is_empty() {
        return None;
    }
    words
        .windows(wanted.len())
        .find(|run| run.iter().zip(&wanted).all(|(w, k)| key(&w.text) == *k))
        .map(|run| run[0].range.from())
}

/// A `code-diff` overlay's parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeDiffOverlay {
    pub range: TimeRange,
    pub diff: String,
    pub highlight_line: Option<usize>,
    pub highlight_at: Option<Micros>,
}

impl CodeDiffOverlay {
    /// The overlay node for `cut.kdl`.
    pub fn to_overlay(&self) -> Overlay {
        let id = |name| Id::new(name).expect("parameter names are ids");
        let mut params = vec![(id("diff"), ParamValue::Text(self.diff.clone()))];
        if let Some(line) = self.highlight_line {
            params.push((id("highlight-line"), ParamValue::Integer(line as i64)));
        }
        if let Some(at) = self.highlight_at {
            params.push((id("highlight-at"), ParamValue::Integer(at.0 as i64)));
        }
        Overlay {
            template: id(TEMPLATE),
            range: self.range,
            params,
        }
    }

    /// The overlay's parameters, `None` for another template or a `code-diff` without `diff`.
    pub fn of(overlay: &Overlay) -> Option<Self> {
        if overlay.template.as_str() != TEMPLATE {
            return None;
        }
        let param = |name: &str| {
            overlay
                .params
                .iter()
                .find(|(n, _)| n.as_str() == name)
                .map(|(_, v)| v)
        };
        let int = |name| match param(name) {
            Some(ParamValue::Integer(n)) => u64::try_from(*n).ok(),
            _ => None,
        };
        let diff = match param("diff") {
            Some(ParamValue::Text(path)) => path.clone(),
            _ => return None,
        };
        Some(Self {
            range: overlay.range,
            diff,
            highlight_line: int("highlight-line").map(|n| n as usize),
            highlight_at: int("highlight-at").map(Micros),
        })
    }

    /// The panel at media time `t`: the lines typed so far and the highlight once it is due.
    pub fn draw(&self, diff: &CodeDiff, t: Micros, canvas: Canvas) -> Vec<Shape> {
        if t < self.range.from() || t >= self.range.to() {
            return Vec::new();
        }
        let s = canvas.width as f32 / 3840.0;
        let from = self.range.from().0 as f32;
        let duration = self.range.duration().0 as f32;
        let elapsed = t.0 as f32 - from;
        let fade = (elapsed / FADE)
            .min((self.range.to().0 as f32 - t.0 as f32) / FADE)
            .clamp(0.0, 1.0);

        let panel = Rect::new(
            canvas.width as f32 * 0.17,
            canvas.height as f32 * 0.15,
            canvas.width as f32 * 0.83,
            canvas.height as f32 * 0.85,
        );
        let size = 44.0 * s;
        let line_height = size * 1.55;
        let pad = 48.0 * s;
        let rows = ((panel.height() - 2.0 * pad) / line_height)
            .floor()
            .max(1.0) as usize;

        let total: usize = diff.lines.iter().map(|l| l.text.chars().count() + 1).sum();
        let typed_share = (elapsed / (duration * TYPING_SHARE)).clamp(0.0, 1.0);
        let mut budget = (total as f32 * typed_share).round() as usize;
        let mut shown: Vec<(&DiffLine, String)> = Vec::new();
        for line in &diff.lines {
            if budget == 0 {
                break;
            }
            let count = line.text.chars().count();
            let take = count.min(budget);
            shown.push((line, line.text.chars().take(take).collect()));
            budget = budget.saturating_sub(count + 1);
        }
        let typing = typed_share < 1.0;
        let highlight = self
            .highlight_line
            .filter(|_| t >= self.highlight_at.unwrap_or(self.range.from()))
            .and_then(|number| shown.iter().position(|(l, _)| l.number == number));
        // Keep the caret in view while typing, then the highlight.
        let focus = highlight.unwrap_or(shown.len().saturating_sub(1));
        let first = if highlight.is_some() {
            focus.saturating_sub(rows / 2)
        } else {
            (focus + 1).saturating_sub(rows)
        }
        .min(shown.len().saturating_sub(rows));

        let mut inner = Vec::new();
        for (row, (index, (line, text))) in
            shown.iter().enumerate().skip(first).take(rows).enumerate()
        {
            let top = panel.top + pad + row as f32 * line_height;
            let band = Rect::new(panel.left, top, panel.right, top + line_height);
            let tint = match line.kind {
                LineKind::Added => Some(Rgba::hex(0xC8F25A, 0.07)),
                LineKind::Removed => Some(Rgba::hex(0xFF5A52, 0.08)),
                _ => None,
            };
            let tint = if highlight == Some(index) {
                Some(Rgba::hex(0xC8F25A, 0.2))
            } else {
                tint
            };
            if let Some(color) = tint {
                inner.push(Shape::Rect {
                    rect: band,
                    radius: 0.0,
                    color: color.faded(fade),
                });
            }
            let color = match line.kind {
                LineKind::File | LineKind::Hunk => Rgba::hex(0x5E6570, 1.0),
                LineKind::Context => Rgba::hex(0xC3C9D3, 1.0),
                LineKind::Added => Rgba::hex(0xC8F25A, 1.0),
                LineKind::Removed => Rgba::hex(0xFF8A80, 1.0),
            };
            let origin = [panel.left + pad, top + (line_height - size * 1.2) / 2.0];
            inner.push(Shape::Text {
                origin,
                text: text.clone(),
                size,
                font: Font::Mono,
                color: color.faded(fade),
            });
            let last = index + 1 == shown.len();
            if typing && last {
                let x = origin[0] + text.chars().count() as f32 * size * 0.6;
                inner.push(Shape::Rect {
                    rect: Rect::new(
                        x,
                        top + line_height * 0.18,
                        x + size * 0.5,
                        top + line_height * 0.82,
                    ),
                    radius: 2.0 * s,
                    color: Rgba::hex(0xC8F25A, fade),
                });
            }
        }
        vec![
            Shape::Shadow {
                rect: panel,
                radius: 28.0 * s,
                blur: 40.0 * s,
                color: Rgba::hex(0x000000, 0.45 * fade),
            },
            Shape::Rect {
                rect: panel,
                radius: 28.0 * s,
                color: Rgba::hex(0x0A0B0D, fade),
            },
            Shape::Clip {
                rect: panel,
                radius: 28.0 * s,
                shapes: inner,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: &str = "diff --git a/src/query.rs b/src/query.rs
index 1111111..2222222 100644
--- a/src/query.rs
+++ b/src/query.rs
@@ -1,3 +1,3 @@
 pub fn parse(input: &str) -> Query {
-    Query::text(input)
+    Query::tokens(input)
 }
";

    #[test]
    fn a_snapshot_shows_file_hunks_and_lines_by_their_file_line() {
        let diff = CodeDiff::parse(SNAPSHOT);
        let kinds: Vec<_> = diff.lines.iter().map(|l| (l.number, l.kind)).collect();
        assert_eq!(
            kinds,
            [
                (4, LineKind::File),
                (5, LineKind::Hunk),
                (6, LineKind::Context),
                (7, LineKind::Removed),
                (8, LineKind::Added),
                (9, LineKind::Context),
            ]
        );
        assert_eq!(diff.lines[0].text, "src/query.rs");
    }

    #[test]
    fn snapshot_writes_the_diff_of_a_commit_range() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let signature = git2::Signature::now("t", "t@example.com").unwrap();
        let mut parent: Option<git2::Oid> = None;
        for body in ["one\ntwo\n", "one\n2\n"] {
            std::fs::write(dir.path().join("a.txt"), body).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("a.txt")).unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit> = parent
                .iter()
                .map(|p| repo.find_commit(*p).unwrap())
                .collect();
            let parents: Vec<&git2::Commit> = parents.iter().collect();
            parent = Some(
                repo.commit(Some("HEAD"), &signature, &signature, "c", &tree, &parents)
                    .unwrap(),
            );
        }
        let text = snapshot(dir.path(), "HEAD~1..HEAD").unwrap();
        let lines: Vec<_> = CodeDiff::parse(&text)
            .lines
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(lines, ["a.txt", "@@ -1,2 +1,2 @@", " one", "-two", "+2"]);
    }

    #[test]
    fn an_overlay_reads_back_its_own_parameters() {
        let overlay = CodeDiffOverlay {
            range: TimeRange::new(Micros(12_000_000), Micros(26_000_000)).unwrap(),
            diff: "sources/query-parse.diff".into(),
            highlight_line: Some(4),
            highlight_at: Some(Micros(21_300_000)),
        };
        assert_eq!(CodeDiffOverlay::of(&overlay.to_overlay()), Some(overlay));
    }

    #[test]
    fn a_phrase_starts_at_its_first_word() {
        let word = |text: &str, from: u64| Word {
            text: text.into(),
            range: TimeRange::new(Micros(from), Micros(from + 100)).unwrap(),
            confidence: 90,
        };
        let words = [
            word("Now", 0),
            word("the", 200),
            word("Parser,", 400),
            word("splits", 600),
        ];
        assert_eq!(phrase_at(&words, "the parser splits"), Some(Micros(200)));
        assert_eq!(phrase_at(&words, "the lexer"), None);
    }

    #[test]
    fn the_panel_types_out_then_highlights_its_line() {
        let diff = CodeDiff::parse(SNAPSHOT);
        let overlay = CodeDiffOverlay {
            range: TimeRange::new(Micros(0), Micros(10_000_000)).unwrap(),
            diff: "sources/q.diff".into(),
            highlight_line: Some(8),
            highlight_at: Some(Micros(8_000_000)),
        };
        let texts = |t| -> Vec<String> {
            let shapes = overlay.draw(&diff, Micros(t), Canvas::UHD);
            let Some(Shape::Clip { shapes, .. }) = shapes.last() else {
                return Vec::new();
            };
            shapes
                .iter()
                .filter_map(|s| match s {
                    Shape::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        assert!(texts(0).is_empty());
        let halfway = texts(3_000_000);
        assert!(halfway.len() < diff.lines.len());
        assert_eq!(texts(7_000_000).len(), diff.lines.len());
        let highlighted = |t| {
            overlay.draw(&diff, Micros(t), Canvas::UHD).iter().any(|s| {
                matches!(s, Shape::Clip { shapes, .. } if shapes.iter().any(|s| matches!(s,
                    Shape::Rect { color, .. } if (color.0[3] - 0.2).abs() < 1e-3)))
            })
        };
        assert!(!highlighted(7_900_000));
        assert!(highlighted(8_000_000));
    }
}
