//! Finds how far into the script the heard words reach.
//!
//! The heard words are the recognizer's newest hypothesis: a few seconds of speech, with old
//! words in front and often a cut-off or wrong word at the end. A local alignment against the
//! script around the current position finds the longest run that agrees, and its end is the new
//! position. A run needs two matching words, or one distinctive word right at the position, so a
//! stray "the" never jumps ahead.

/// Script words before the position the heard words may still align with.
const BACK: usize = 8;
/// Script words after the position the heard words may reach.
const AHEAD: usize = 14;
/// Heard words used, from the newest.
const HEARD: usize = 12;
const MATCH: i32 = 3;
const MISMATCH: i32 = -2;
const GAP: i32 = -1;

/// The script position after the heard words, as a count of words said, when it moves past
/// `from`. `script` and `heard` are normalized words.
pub fn advance(script: &[String], from: usize, heard: &[String]) -> Option<usize> {
    let heard = &heard[heard.len().saturating_sub(HEARD)..];
    let lo = from.saturating_sub(BACK).min(script.len());
    let hi = (from + AHEAD).min(script.len());
    let window = &script[lo..hi];
    if heard.is_empty() || window.is_empty() {
        return None;
    }
    let cols = window.len() + 1;
    // Score and matched words of the best alignment ending at each cell.
    let mut score = vec![(0i32, 0u32); (heard.len() + 1) * cols];
    let mut best: Option<(i32, u32, usize)> = None;
    for i in 1..=heard.len() {
        for j in 1..=window.len() {
            let hit = same(&heard[i - 1], &window[j - 1]);
            let (diag, diag_hits) = score[(i - 1) * cols + j - 1];
            let candidates = [
                (
                    diag + if hit { MATCH } else { MISMATCH },
                    diag_hits + u32::from(hit),
                ),
                (
                    score[(i - 1) * cols + j].0 + GAP,
                    score[(i - 1) * cols + j].1,
                ),
                (score[i * cols + j - 1].0 + GAP, score[i * cols + j - 1].1),
            ];
            let cell = candidates
                .into_iter()
                .max_by_key(|c| c.0)
                .filter(|c| c.0 > 0)
                .unwrap_or((0, 0));
            score[i * cols + j] = cell;
            // On a tie the nearer run wins, so a repeated phrase is not skipped.
            if hit && best.is_none_or(|(s, _, _)| cell.0 > s) {
                best = Some((cell.0, cell.1, j));
            }
        }
    }
    let (_, hits, end) = best?;
    let to = lo + end;
    let next_word = to == from + 1 && script[from].chars().count() >= 4;
    (to > from && (hits >= 2 || next_word)).then_some(to)
}

/// Whether a heard word is the script word: equal, cut off at the end of the audio (at least
/// three letters of it), or one edit apart when both are long.
fn same(heard: &str, script: &str) -> bool {
    if heard == script {
        return true;
    }
    let (h, s) = (heard.chars().count(), script.chars().count());
    (h >= 3 && script.starts_with(heard)) || (h.min(s) >= 4 && one_edit(heard, script))
}

fn one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    if long.len() - short.len() > 1 {
        return false;
    }
    let prefix = short
        .iter()
        .zip(long.iter())
        .take_while(|(x, y)| x == y)
        .count();
    if short.len() == long.len() {
        short[prefix + 1..] == long[prefix + 1..]
    } else {
        short[prefix..] == long[prefix + 1..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn follows_the_newest_run_of_agreeing_words() {
        let script = words("agents don't need editors they need a goal a sandbox and a reviewer");
        assert_eq!(advance(&script, 0, &words("agents don't need")), Some(3));
        // Old words in front, a cut-off word at the end.
        assert_eq!(
            advance(&script, 3, &words("need editors they need a go")),
            Some(7)
        );
        assert_eq!(advance(&script, 7, &words("need editors they")), None);
    }

    #[test]
    fn tolerates_misheard_and_skipped_words() {
        let script = words("the renderer only redraws the chapters that changed");
        assert_eq!(
            advance(&script, 0, &words("the render only redraw chapters")),
            Some(6)
        );
    }

    #[test]
    fn a_lone_common_word_never_jumps_ahead() {
        let script = words("we open the file and we check the settings");
        assert_eq!(advance(&script, 2, &words("um the")), None);
        assert_eq!(advance(&script, 3, &words("file")), Some(4));
        assert_eq!(advance(&script, 3, &words("settings")), None);
    }
}
