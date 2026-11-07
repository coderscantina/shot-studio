//! A line diff of two versions of a file, with a little context around each change. The Ship
//! screen shows `cut.kdl` against what the video branch holds with it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Same,
    Added,
    Removed,
    /// Unchanged lines left out between two hunks.
    Gap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub change: Change,
    pub text: String,
}

/// The changed lines of `new` against `old`, each hunk with `context` unchanged lines around
/// it. Empty when the texts are the same.
pub fn diff(old: &str, new: &str, context: usize) -> Vec<Line> {
    let full = lines(old, new);
    let near_change = |i: usize| {
        let from = i.saturating_sub(context);
        let to = (i + context + 1).min(full.len());
        full[from..to].iter().any(|l| l.change != Change::Same)
    };
    let mut out = Vec::new();
    let mut skipped = false;
    for (i, line) in full.iter().enumerate() {
        if near_change(i) {
            if skipped && !out.is_empty() {
                out.push(Line {
                    change: Change::Gap,
                    text: String::new(),
                });
            }
            skipped = false;
            out.push(line.clone());
        } else {
            skipped = true;
        }
    }
    out
}

/// How many lines were added and removed.
pub fn stat(lines: &[Line]) -> (usize, usize) {
    let count = |c| lines.iter().filter(|l| l.change == c).count();
    (count(Change::Added), count(Change::Removed))
}

/// Every line of both texts in order, by the longest common subsequence of the lines between
/// their common head and tail.
fn lines(old: &str, new: &str) -> Vec<Line> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    // lcs[i][j]: the common subsequence length of am[i..] and bm[j..].
    let width = bm.len() + 1;
    let mut lcs = vec![0u32; (am.len() + 1) * width];
    for i in (0..am.len()).rev() {
        for j in (0..bm.len()).rev() {
            lcs[i * width + j] = if am[i] == bm[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let line = |change, text: &str| Line {
        change,
        text: text.to_owned(),
    };
    let mut out: Vec<Line> = a[..head].iter().map(|t| line(Change::Same, t)).collect();
    let (mut i, mut j) = (0, 0);
    while i < am.len() || j < bm.len() {
        if i < am.len() && j < bm.len() && am[i] == bm[j] {
            out.push(line(Change::Same, am[i]));
            i += 1;
            j += 1;
        } else if i < am.len()
            && (j == bm.len() || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
        {
            out.push(line(Change::Removed, am[i]));
            i += 1;
        } else {
            out.push(line(Change::Added, bm[j]));
            j += 1;
        }
    }
    out.extend(a[a.len() - tail..].iter().map(|t| line(Change::Same, t)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_changed_lines_with_context_and_gaps() {
        let old = "a\nb\nc\nd\ne\nf\ng\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\n";
        let lines = diff(old, new, 1);
        let found: Vec<(Change, &str)> =
            lines.iter().map(|l| (l.change, l.text.as_str())).collect();
        assert_eq!(
            found,
            [
                (Change::Same, "a"),
                (Change::Removed, "b"),
                (Change::Added, "B"),
                (Change::Same, "c"),
                (Change::Gap, ""),
                (Change::Same, "g"),
                (Change::Added, "h"),
            ]
        );
        assert_eq!(stat(&lines), (2, 1));
        assert!(diff(old, old, 3).is_empty());
    }
}
