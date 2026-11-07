//! What the prompter shows and where the reader is.

use std::time::Duration;

use shot_analyze::normalize;
use shot_core::{PrompterMode, Script};

use crate::matcher;

/// Speaking rate for steady scrolling and time left until the reader's own rate is known.
const WORDS_PER_MINUTE: f32 = 150.0;

/// How the prompter moves on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scroll {
    /// Follows the reader's voice.
    #[default]
    Voice,
    /// Moves at a steady speaking rate.
    Steady,
    /// Moves a beat at a time, on the pedal's key or N.
    Pedal,
}

/// An inline retake the retake rule caught while recording: what was heard and the clean
/// version it keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retake {
    pub heard: String,
    pub kept: String,
}

/// One unit the prompter shows at a time: a beat's bullet, or one sentence of its full script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Words as written.
    pub words: Vec<String>,
    /// Index of the beat across the script, from 0.
    beat: usize,
    /// Index of the line's first word across the script.
    start: usize,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.join(" ")
    }
}

struct BeatInfo {
    chapter: usize,
    first_line: usize,
    /// Words the reader speaks for this beat, for time left.
    spoken: usize,
}

/// The prompter's content and the reader's position in it.
pub struct Prompter {
    chapters: Vec<String>,
    beats: Vec<BeatInfo>,
    lines: Vec<Line>,
    /// Normalized words of all lines, for matching.
    keys: Vec<String>,
    /// Words said, counted across the script.
    position: usize,
    current: usize,
    scroll: Scroll,
    retakes: u32,
    /// Fractional words a steady scroll has moved.
    steady: f32,
    caught: Option<(u32, Retake)>,
}

/// What the prompter shows now.
#[derive(Debug, Clone, PartialEq)]
pub struct View<'a> {
    pub previous: Option<&'a Line>,
    pub current: Option<&'a Line>,
    /// Words of the current line said so far.
    pub said: usize,
    pub next: Option<&'a Line>,
    pub chapter: &'a str,
    /// The current beat, numbered from 1 across the script.
    pub beat: u32,
    pub beats: u32,
    pub retakes: u32,
    pub time_left: Duration,
}

impl Prompter {
    /// The script's beats as lines: the bullets in beats mode, the sentences of each beat's
    /// full script in full-script mode (the bullet when a beat has none).
    pub fn new(script: &Script, mode: PrompterMode) -> Self {
        let mut prompter = Self {
            chapters: Vec::new(),
            beats: Vec::new(),
            lines: Vec::new(),
            keys: Vec::new(),
            position: 0,
            current: 0,
            scroll: Scroll::default(),
            retakes: 0,
            steady: 0.0,
            caught: None,
        };
        for (chapter, ch) in script.chapters.iter().enumerate() {
            prompter.chapters.push(ch.title.clone());
            for beat in &ch.beats {
                let spoken = if beat.script.is_empty() {
                    &beat.text
                } else {
                    &beat.script
                };
                let units = match mode {
                    PrompterMode::Beats => vec![beat.text.as_str()],
                    PrompterMode::FullScript => sentences(spoken),
                };
                let index = prompter.beats.len();
                prompter.beats.push(BeatInfo {
                    chapter,
                    first_line: prompter.lines.len(),
                    spoken: spoken.split_whitespace().count(),
                });
                for unit in units {
                    let words: Vec<String> = unit.split_whitespace().map(str::to_owned).collect();
                    if words.is_empty() {
                        continue;
                    }
                    prompter.keys.extend(words.iter().map(|w| normalize(w)));
                    prompter.lines.push(Line {
                        beat: index,
                        start: prompter.keys.len() - words.len(),
                        words,
                    });
                }
            }
        }
        prompter
    }

    pub fn scroll(&self) -> Scroll {
        self.scroll
    }

    pub fn set_scroll(&mut self, scroll: Scroll) {
        self.scroll = scroll;
        self.steady = 0.0;
    }

    /// Words said, counted across the script.
    pub fn position(&self) -> usize {
        self.position
    }

    /// The current beat, numbered from 1 across the script. 0 for a script without beats.
    pub fn beat(&self) -> u32 {
        self.lines
            .get(self.current)
            .map_or(0, |line| line.beat as u32 + 1)
    }

    /// A: back to the start of the current beat. Counts a retake and returns the beat.
    pub fn again(&mut self) -> u32 {
        self.retakes += 1;
        if let Some(line) = self.lines.get(self.current) {
            self.go_to_line(self.beats[line.beat].first_line);
        }
        self.beat()
    }

    /// N or the pedal: on to the start of the next beat. Returns the beat shown after it.
    pub fn next_beat(&mut self) -> u32 {
        let next = self
            .lines
            .get(self.current)
            .and_then(|line| self.beats.get(line.beat + 1));
        if let Some(beat) = next {
            self.go_to_line(beat.first_line);
        }
        self.beat()
    }

    /// Voice follow: moves to where the newest recognized words reach. Never moves back.
    pub fn hear<S: AsRef<str>>(&mut self, words: &[S]) {
        if self.scroll != Scroll::Voice {
            return;
        }
        let heard: Vec<String> = words
            .iter()
            .map(|w| normalize(w.as_ref()))
            .filter(|w| !w.is_empty())
            .collect();
        if let Some(to) = matcher::advance(&self.keys, self.position, &heard) {
            self.set_position(to);
        }
    }

    /// Steady scroll: moves on by the speaking rate over `elapsed`.
    pub fn tick(&mut self, elapsed: Duration) {
        if self.scroll != Scroll::Steady {
            return;
        }
        self.steady += elapsed.as_secs_f32() * WORDS_PER_MINUTE / 60.0;
        let words = self.steady.floor();
        if words >= 1.0 {
            self.steady -= words;
            self.set_position((self.position + words as usize).min(self.keys.len()));
        }
    }

    /// Shows an inline retake the retake rule caught, and counts it.
    pub fn caught(&mut self, retake: Retake) {
        self.retakes += 1;
        let seq = self.caught.as_ref().map_or(1, |(seq, _)| seq + 1);
        self.caught = Some((seq, retake));
    }

    /// The newest caught retake with a sequence number that changes with each one.
    pub fn last_retake(&self) -> Option<(u32, &Retake)> {
        self.caught.as_ref().map(|(seq, r)| (*seq, r))
    }

    pub fn view(&self) -> View<'_> {
        let current = self.lines.get(self.current);
        let words_left: usize = match current {
            Some(line) => {
                let beat = &self.beats[line.beat];
                let done_in_beat = self.position - self.lines[beat.first_line].start;
                beat.spoken.saturating_sub(done_in_beat)
                    + self.beats[line.beat + 1..]
                        .iter()
                        .map(|b| b.spoken)
                        .sum::<usize>()
            }
            None => 0,
        };
        View {
            previous: self.current.checked_sub(1).and_then(|i| self.lines.get(i)),
            current,
            said: current.map_or(0, |line| self.position - line.start),
            next: self.lines.get(self.current + 1),
            chapter: current.map_or("", |line| &self.chapters[self.beats[line.beat].chapter]),
            beat: self.beat(),
            beats: self.beats.len() as u32,
            retakes: self.retakes,
            time_left: Duration::from_secs_f32(words_left as f32 * 60.0 / WORDS_PER_MINUTE),
        }
    }

    fn go_to_line(&mut self, line: usize) {
        self.current = line;
        self.position = self.lines[line].start;
        self.steady = 0.0;
    }

    /// Sets the words said and makes the line holding the next word current. A finished last
    /// line stays current.
    fn set_position(&mut self, position: usize) {
        self.position = position;
        while self.current + 1 < self.lines.len() && self.lines[self.current + 1].start <= position
        {
            self.current += 1;
        }
    }
}

/// Sentences of a text, split after `.`, `?` and `!` and at line breaks.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for paragraph in text.lines() {
        let mut start = 0;
        let chars: Vec<(usize, char)> = paragraph.char_indices().collect();
        for (k, &(i, c)) in chars.iter().enumerate() {
            let ends = matches!(c, '.' | '?' | '!')
                && chars.get(k + 1).is_none_or(|&(_, n)| n.is_whitespace());
            if ends {
                out.push(paragraph[start..=i].trim());
                start = i + 1;
            }
        }
        out.push(paragraph[start..].trim());
    }
    out.retain(|s| !s.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "# Why we deploy on Fridays

## Hook

- Everyone says never deploy on a Friday. [the post](sources/never-friday.md)
  Everyone says never deploy on a Friday.
  We ship every single week, on Friday afternoon.

  And nothing burns.

## The rule

- Where the rule comes from
- Why it points at a slow rollback
";

    fn prompter(mode: PrompterMode) -> Prompter {
        Prompter::new(&Script::parse(SCRIPT).unwrap(), mode)
    }

    fn texts(view: &View) -> [Option<String>; 3] {
        [view.previous, view.current, view.next].map(|l| l.map(Line::text))
    }

    #[test]
    fn beats_mode_shows_bullets_and_full_script_mode_sentences() {
        let beats = prompter(PrompterMode::Beats);
        let view = beats.view();
        assert_eq!(
            texts(&view),
            [
                None,
                Some("Everyone says never deploy on a Friday. the post".into()),
                Some("Where the rule comes from".into()),
            ]
        );
        assert_eq!((view.chapter, view.beat, view.beats), ("Hook", 1, 3));

        let full = prompter(PrompterMode::FullScript);
        assert_eq!(
            full.lines.iter().map(Line::text).collect::<Vec<_>>(),
            [
                "Everyone says never deploy on a Friday.",
                "We ship every single week, on Friday afternoon.",
                "And nothing burns.",
                "Where the rule comes from",
                "Why it points at a slow rollback",
            ]
        );
    }

    #[test]
    fn voice_follow_marks_words_said_and_moves_to_the_next_line() {
        let mut p = prompter(PrompterMode::FullScript);
        p.hear(&["Everyone", "says", "never"]);
        assert_eq!(p.view().said, 3);
        p.hear(&["never", "deploy", "on", "a", "Friday."]);
        let view = p.view();
        assert_eq!(view.said, 0);
        assert_eq!(
            view.current.map(Line::text).as_deref(),
            Some("We ship every single week, on Friday afternoon.")
        );
        assert_eq!(view.beat, 1);
    }

    #[test]
    fn again_and_next_move_by_beat_and_count_retakes() {
        let mut p = prompter(PrompterMode::FullScript);
        p.hear(&[
            "Everyone", "says", "never", "deploy", "on", "a", "Friday", "We", "ship",
        ]);
        assert_eq!(p.next_beat(), 2);
        assert_eq!(p.view().chapter, "The rule");
        assert_eq!(p.again(), 2);
        assert_eq!(p.next_beat(), 3);
        assert_eq!(p.next_beat(), 3);
        assert_eq!(p.view().retakes, 1);
        p.caught(Retake {
            heard: "the the first".into(),
            kept: "the first".into(),
        });
        assert_eq!(p.view().retakes, 2);
        assert_eq!(p.last_retake().map(|(seq, _)| seq), Some(1));
    }

    #[test]
    fn steady_scroll_moves_at_the_speaking_rate_and_time_left_shrinks() {
        let mut p = prompter(PrompterMode::FullScript);
        let before = p.view().time_left;
        p.set_scroll(Scroll::Steady);
        p.hear(&["Everyone", "says", "never"]);
        assert_eq!(p.position(), 0);
        p.tick(Duration::from_secs(2));
        assert_eq!(p.position(), 5);
        assert_eq!(before - p.view().time_left, Duration::from_secs(2));
    }
}
