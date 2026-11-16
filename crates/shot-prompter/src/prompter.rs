//! What the prompter shows and where the reader is.

use std::ops::{Range, RangeInclusive};
use std::time::Duration;

use shot_analyze::normalize;
use shot_core::{PrompterMode, Script};

use crate::matcher;

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

/// How the prompter moves on, how fast, and how long it waits at the start of a take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pace {
    pub scroll: Scroll,
    /// Speaking rate for steady scrolling and time left.
    pub words_per_minute: f32,
    /// The countdown the prompter shows before it moves on, from [`Prompter::start`].
    pub countdown: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Self {
            scroll: Scroll::default(),
            words_per_minute: 150.0,
            countdown: Duration::ZERO,
        }
    }
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
    /// The chapter's title when this line opens a chapter.
    pub section: Option<String>,
    /// Whether this line opens a beat, so the output sets it apart from the line before.
    pub opens_beat: bool,
    /// Index of the beat across the script, from 0.
    beat: usize,
    /// The script words voice follow matches for this line. In beats mode that is the beat's
    /// spoken script, which the bullet only sums up.
    keys: Range<usize>,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.join(" ")
    }
}

struct BeatInfo {
    chapter: usize,
    first_line: usize,
    /// The beat's words in the matched script words.
    keys: Range<usize>,
}

/// How voice follow moved the prompter, for the marker it writes into the take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    /// The reader went on to this beat, numbered from 1 across the script, `words` into it.
    Next { beat: u32, words: usize },
    /// The reader started this beat over.
    Again { beat: u32 },
}

/// The prompter's content and the reader's position in it.
pub struct Prompter {
    chapters: Vec<String>,
    beats: Vec<BeatInfo>,
    lines: Vec<Line>,
    /// Normalized words the reader speaks, for matching and time left.
    keys: Vec<String>,
    /// Words said, counted across the script.
    position: usize,
    current: usize,
    /// The beats this take records, by index: the whole script unless a part is chosen.
    part: RangeInclusive<usize>,
    pace: Pace,
    /// What is left of the countdown, while it runs.
    countdown: Option<Duration>,
    retakes: u32,
    /// Fractional words a steady scroll has moved.
    steady: f32,
    caught: Option<(u32, Retake)>,
    /// A restart heard once, which the next hypothesis has to confirm.
    rewind: Option<usize>,
}

/// What the prompter shows now.
#[derive(Debug, Clone, PartialEq)]
pub struct View<'a> {
    pub previous: Option<&'a Line>,
    pub current: Option<&'a Line>,
    /// Index of the current line in [`Prompter::lines`].
    pub index: usize,
    /// Words of the current line said so far.
    pub said: usize,
    /// The lines after the current one, up to the end of the part.
    pub ahead: &'a [Line],
    pub chapter: &'a str,
    /// The current beat, numbered from 1 across the script.
    pub beat: u32,
    pub beats: u32,
    /// The current beat counted in the part, from 1, and the part's beats.
    pub part_beat: u32,
    pub part_beats: u32,
    pub retakes: u32,
    pub time_left: Duration,
    /// Whole seconds left of the countdown, while it runs.
    pub countdown: Option<u32>,
}

impl Prompter {
    /// The script's beats as lines: the bullets in beats mode, the sentences of each beat's
    /// full script in full-script mode (the bullet when a beat has none). Voice follow always
    /// matches what the reader speaks: the full script where a beat has one.
    pub fn new(script: &Script, mode: PrompterMode) -> Self {
        let mut prompter = Self {
            chapters: Vec::new(),
            beats: Vec::new(),
            lines: Vec::new(),
            keys: Vec::new(),
            position: 0,
            current: 0,
            part: 0..=0,
            pace: Pace::default(),
            countdown: None,
            retakes: 0,
            steady: 0.0,
            caught: None,
            rewind: None,
        };
        for (chapter, ch) in script.chapters.iter().enumerate() {
            prompter.chapters.push(ch.title.clone());
            let mut section = Some(ch.title.clone());
            for beat in &ch.beats {
                let spoken = if beat.script.is_empty() {
                    &beat.text
                } else {
                    &beat.script
                };
                let index = prompter.beats.len();
                let from = prompter.keys.len();
                let first_line = prompter.lines.len();
                let mut line = |words: Vec<String>, keys: Range<usize>, lines: &mut Vec<Line>| {
                    if !words.is_empty() {
                        lines.push(Line {
                            words,
                            section: section.take(),
                            opens_beat: lines.len() == first_line,
                            beat: index,
                            keys,
                        });
                    }
                };
                match mode {
                    PrompterMode::Beats => {
                        prompter
                            .keys
                            .extend(spoken.split_whitespace().map(normalize));
                        let words = beat.text.split_whitespace().map(str::to_owned).collect();
                        line(words, from..prompter.keys.len(), &mut prompter.lines);
                    }
                    PrompterMode::FullScript => {
                        for unit in sentences(spoken) {
                            let words: Vec<String> =
                                unit.split_whitespace().map(str::to_owned).collect();
                            let at = prompter.keys.len();
                            prompter.keys.extend(words.iter().map(|w| normalize(w)));
                            line(words, at..prompter.keys.len(), &mut prompter.lines);
                        }
                    }
                }
                prompter.beats.push(BeatInfo {
                    chapter,
                    first_line,
                    keys: from..prompter.keys.len(),
                });
            }
        }
        prompter.part = 0..=prompter.beats.len().saturating_sub(1);
        prompter
    }

    /// Every line, in order.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// Records only `beats` (by index across the script, from 0): starts at the first and ends
    /// after the last. Out of range beats clamp to the script.
    pub fn select(&mut self, beats: RangeInclusive<usize>) {
        let last = self.beats.len().saturating_sub(1);
        let (from, to) = ((*beats.start()).min(last), (*beats.end()).min(last));
        self.part = from..=to.max(from);
        if let Some(line) = self.beats.get(from).map(|b| b.first_line)
            && line < self.lines.len()
        {
            self.go_to_line(line);
        }
    }

    pub fn pace(&self) -> Pace {
        self.pace
    }

    pub fn set_pace(&mut self, pace: Pace) {
        self.pace = pace;
        self.steady = 0.0;
    }

    /// The take starts: runs the pace's countdown, if it has one, before steady scroll moves.
    pub fn start(&mut self) {
        self.countdown = (!self.pace.countdown.is_zero()).then_some(self.pace.countdown);
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

    /// N or the pedal: on to the start of the next beat of the part. Returns the beat shown
    /// after it.
    pub fn next_beat(&mut self) -> u32 {
        let next = self
            .lines
            .get(self.current)
            .map(|line| line.beat + 1)
            .filter(|beat| beat <= self.part.end())
            .and_then(|beat| self.beats.get(beat))
            .map(|beat| beat.first_line)
            .filter(|&line| line < self.lines.len());
        if let Some(line) = next {
            self.go_to_line(line);
        }
        self.beat()
    }

    /// Voice follow: moves to where the newest recognized words reach, across lines and beats.
    /// When the reader starts a sentence of this beat or the one before over, it jumps back
    /// there and counts a retake. Says when it moved to another beat or started one over.
    pub fn hear<S: AsRef<str>>(&mut self, words: &[S]) -> Option<Moved> {
        if self.pace.scroll != Scroll::Voice || self.lines.is_empty() {
            return None;
        }
        let heard: Vec<String> = words
            .iter()
            .map(|w| normalize(w.as_ref()))
            .filter(|w| !w.is_empty())
            .collect();
        let before = self.lines[self.current].beat;
        let end = self.beats[*self.part.end()].keys.end;
        if let Some(to) = matcher::advance(&self.keys[..end], self.position, &heard) {
            self.rewind = None;
            // Reading before the countdown ends means the reader is ready.
            self.countdown = None;
            self.set_position(to);
            let now = self.lines[self.current].beat;
            return (now != before).then(|| Moved::Next {
                beat: now as u32 + 1,
                words: to - self.beats[now].keys.start,
            });
        }
        let floor_beat = before.saturating_sub(1).max(*self.part.start());
        let floor = self.beats[floor_beat].keys.start;
        let heard_at = matcher::rewind(&self.keys[..end], self.position, floor, &heard);
        // One hypothesis can be wrong; two in a row that agree are the reader starting over.
        let confirmed = match (self.rewind, heard_at) {
            (Some(before), Some(now)) => before.abs_diff(now) <= REWIND_AGREE,
            _ => false,
        };
        self.rewind = heard_at;
        let to = heard_at.filter(|_| confirmed)?;
        self.rewind = None;
        self.retakes += 1;
        self.countdown = None;
        self.set_position(to);
        let now = self.lines[self.current].beat;
        let from_top = to - self.beats[now].keys.start <= RESTART_WORDS;
        (now != before || from_top).then(|| Moved::Again {
            beat: now as u32 + 1,
        })
    }

    /// Runs the countdown, then steady scroll moves on by the speaking rate over `elapsed`.
    pub fn tick(&mut self, elapsed: Duration) {
        if let Some(left) = self.countdown {
            let rest = left.saturating_sub(elapsed);
            self.countdown = (!rest.is_zero()).then_some(rest);
            return;
        }
        if self.pace.scroll != Scroll::Steady || self.lines.is_empty() {
            return;
        }
        self.steady += elapsed.as_secs_f32() * self.pace.words_per_minute / 60.0;
        let words = self.steady.floor();
        if words >= 1.0 {
            self.steady -= words;
            let end = self.beats[*self.part.end()].keys.end;
            self.set_position((self.position + words as usize).min(end));
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
        let words_left = current.map_or(0, |line| {
            let end = self.beats[*self.part.end()].keys.end;
            end.saturating_sub(self.position.max(self.beats[line.beat].keys.start))
        });
        let last_line = self
            .beats
            .get(*self.part.end() + 1)
            .map_or(self.lines.len(), |b| b.first_line);
        let ahead = self
            .lines
            .get(self.current + 1..last_line.max(self.current + 1))
            .unwrap_or(&[]);
        let beat = self.beat();
        View {
            previous: self.current.checked_sub(1).and_then(|i| self.lines.get(i)),
            current,
            index: self.current,
            said: current.map_or(0, |line| said_words(line, self.position)),
            ahead,
            chapter: current.map_or("", |line| &self.chapters[self.beats[line.beat].chapter]),
            beat,
            beats: self.beats.len() as u32,
            part_beat: beat.saturating_sub(*self.part.start() as u32),
            part_beats: (self.part.end() + 1 - self.part.start()) as u32,
            retakes: self.retakes,
            time_left: Duration::from_secs_f32(
                words_left as f32 * 60.0 / self.pace.words_per_minute.max(1.0),
            ),
            countdown: self.countdown.map(|left| left.as_secs_f32().ceil() as u32),
        }
    }

    fn go_to_line(&mut self, line: usize) {
        self.current = line;
        self.position = self.lines[line].keys.start;
        self.steady = 0.0;
        self.countdown = None;
    }

    /// Sets the words said and makes the line holding the next word current, forward or back.
    /// A finished last line stays current.
    fn set_position(&mut self, position: usize) {
        self.position = position;
        while self.current > 0 && self.lines[self.current].keys.start > position {
            self.current -= 1;
        }
        while self.current + 1 < self.lines.len()
            && self.lines[self.current + 1].keys.start <= position
        {
            self.current += 1;
        }
    }
}

/// How far apart, in words, two hypotheses may place a restart and still agree.
const REWIND_AGREE: usize = 2;

/// A restart this close to the top of a beat starts the beat over: a new attempt at it.
const RESTART_WORDS: usize = 6;

/// The words of `line` said at `position`. In beats mode the bullet stands for the spoken
/// script, so the share said lights the same share of the bullet.
fn said_words(line: &Line, position: usize) -> usize {
    let done = position
        .saturating_sub(line.keys.start)
        .min(line.keys.len());
    if line.keys.len() == line.words.len() {
        return done;
    }
    (done * line.words.len())
        .checked_div(line.keys.len())
        .unwrap_or(0)
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
        [view.previous, view.current, view.ahead.first()].map(|l| l.map(Line::text))
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
    fn beats_mode_follows_the_spoken_script_into_the_next_beat() {
        let mut p = prompter(PrompterMode::Beats);
        assert_eq!(p.hear(&["Everyone", "says", "never", "deploy"]), None);
        assert_eq!(
            p.view().said,
            2,
            "half the spoken words light half the bullet"
        );
        p.hear(&[
            "deploy", "on", "a", "Friday", "We", "ship", "every", "single", "week",
        ]);
        // The beat's last word said, the next beat shows.
        assert_eq!(
            p.hear(&["on", "Friday", "afternoon", "And", "nothing", "burns"]),
            Some(Moved::Next { beat: 2, words: 0 })
        );
        assert_eq!(p.view().chapter, "The rule");
    }

    #[test]
    fn starting_over_jumps_back_and_counts_a_retake() {
        let mut p = prompter(PrompterMode::FullScript);
        p.hear(&["Everyone", "says", "never", "deploy", "on", "a", "Friday"]);
        p.hear(&["Friday", "We", "ship", "every", "single", "week"]);
        assert_eq!(
            p.hear(&["single", "week", "Everyone", "says", "never", "deploy"]),
            None,
            "one hypothesis is not enough"
        );
        assert_eq!(
            p.hear(&["single", "week", "Everyone", "says", "never", "deploy"]),
            Some(Moved::Again { beat: 1 })
        );
        let view = p.view();
        assert_eq!((view.index, view.said, view.retakes), (0, 4, 1));
    }

    #[test]
    fn a_chosen_part_starts_at_its_first_beat_and_ends_with_its_last() {
        let mut p = prompter(PrompterMode::Beats);
        p.select(1..=1);
        let view = p.view();
        assert_eq!((view.beat, view.part_beat, view.part_beats), (2, 1, 1));
        assert!(view.ahead.is_empty());
        assert_eq!(p.next_beat(), 2, "next stays inside the part");
        assert_eq!(
            p.lines()[1].section.as_deref(),
            Some("The rule"),
            "the first line of a chapter names it"
        );
    }

    #[test]
    fn steady_scroll_moves_at_the_speaking_rate_and_time_left_shrinks() {
        let mut p = prompter(PrompterMode::FullScript);
        let before = p.view().time_left;
        p.set_pace(Pace {
            scroll: Scroll::Steady,
            ..Pace::default()
        });
        p.hear(&["Everyone", "says", "never"]);
        assert_eq!(p.position(), 0);
        p.tick(Duration::from_secs(2));
        assert_eq!(p.position(), 5);
        assert_eq!(before - p.view().time_left, Duration::from_secs(2));
    }

    #[test]
    fn the_countdown_holds_steady_scroll_and_reading_ends_it() {
        let mut p = prompter(PrompterMode::FullScript);
        p.set_pace(Pace {
            scroll: Scroll::Steady,
            words_per_minute: 300.0,
            countdown: Duration::from_secs(3),
        });
        assert_eq!(p.view().countdown, None);
        p.start();
        assert_eq!(p.view().countdown, Some(3));
        p.tick(Duration::from_millis(2500));
        assert_eq!((p.view().countdown, p.position()), (Some(1), 0));
        p.tick(Duration::from_millis(500));
        assert_eq!(p.view().countdown, None);
        p.tick(Duration::from_secs(1));
        assert_eq!(p.position(), 5);

        let mut p = prompter(PrompterMode::FullScript);
        p.set_pace(Pace {
            countdown: Duration::from_secs(5),
            ..Pace::default()
        });
        p.start();
        p.hear(&["Everyone", "says"]);
        assert_eq!((p.view().countdown, p.position()), (None, 2));
    }
}
