//! The Takes view of the Cut screen: every take's attempt at each beat side by side, to rate,
//! compare and compose the cut from. Rows are the beats of the script under their chapters
//! (see [`Lineup`]); each row shows the attempts with their words, length, waveform, stars, the
//! circle of the pick and whether the cut has them now.
//!
//! Arrows move between beats and attempts, Space plays the focused attempt as it would play in
//! the cut, A plays it and then the same beat in another take, 1 to 5 rate it and P circles it.
//! Shift with the arrows, or a click and a shift-click, marks words of the attempt. Enter puts
//! the attempt, or the marked words, into the cut in place of what holds the beat now, and
//! ⇧Enter assembles the whole cut from the picks. Every change is a write of `cut.kdl` through
//! `shot-core`, so ⌘Z undoes it like any edit. A video dropped on the view is imported as a
//! take, see [`import`].

pub mod import;

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use gpui::{
    App, Context, Div, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, FontWeight,
    HighlightStyle, Hsla, InteractiveText, ObjectFit, ScrollHandle, StyledText, Subscription,
    Window, actions, div, prelude::*, px, relative, surface,
};
use shot_core::{
    Attempt, Chapter, Cut, Elements, Id, Lineup, Micros, RangeEdit, Row, RowAt, TakeWords,
    TimeRange, Transcript, Word, word_diff, words_in,
};

use crate::cut::GoTo;
use crate::cut::player::Player;
use crate::folder::VideoFolder;
use crate::peaks::Peaks;
use crate::studio::Screen;
use crate::theme::*;
use crate::ui::{big_button, empty_state, hint, icon, kbd, label, mono, pane, scrolling_list};

pub const CONTEXT: &str = "TakesView";
/// Bars of an attempt's waveform.
const BARS: usize = 72;
/// Width of the preview pane on the right.
const SIDE: f32 = 400.;

actions!(
    takes,
    [
        NextBeat,
        PreviousBeat,
        NextAttempt,
        PreviousAttempt,
        Play,
        Compare,
        Rate1,
        Rate2,
        Rate3,
        Rate4,
        Rate5,
        Pick,
        UseThis,
        Assemble,
        MarkMore,
        MarkLess,
        ClearMark,
        Continue,
    ]
);

/// A take's words and elements, read once.
struct TakeData {
    transcript: Transcript,
    elements: Option<Elements>,
}

/// Words marked in the focused attempt, by their index in its words: from where the mark
/// started to where it ends now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    anchor: usize,
    end: usize,
}

impl Mark {
    fn words(self) -> RangeInclusive<usize> {
        self.anchor.min(self.end)..=self.anchor.max(self.end)
    }
}

pub struct TakesView {
    folder: Entity<VideoFolder>,
    import: Entity<import::Import>,
    takes: HashMap<Id, TakeData>,
    /// The take with the highest number in the media folder, transcribed or not.
    newest: Option<Id>,
    lineup: Lineup,
    /// Every row's place, in order.
    rows: Vec<RowAt>,
    /// The focused row, by its place in `rows`, and the focused attempt in it.
    at: (usize, usize),
    mark: Option<Mark>,
    peaks: HashMap<Id, Option<Arc<Peaks>>>,
    player: Player,
    /// What the loaded clip plays, block by block.
    playing: Vec<Attempt>,
    /// What the last action did, or what a new recording brought.
    note: Option<String>,
    error: Option<String>,
    focus: FocusHandle,
    list: ScrollHandle,
    /// The row the list last scrolled to.
    scrolled: Option<usize>,
    _blur: Option<Subscription>,
}

impl EventEmitter<GoTo> for TakesView {}

impl Focusable for TakesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// "Take 2" for `t02`, the id for a take named otherwise.
pub fn take_name(take: &Id) -> String {
    match take
        .as_str()
        .strip_prefix('t')
        .and_then(|n| n.parse::<u32>().ok())
    {
        Some(n) => format!("Take {n}"),
        None => format!("Take {take}"),
    }
}

/// How long `attempt` plays in the cut: its media without what the take's blocks remove.
fn played(cut: &Cut, attempt: &Attempt) -> Micros {
    let kept = cut
        .material(&attempt.take, attempt.range)
        .ranges
        .iter()
        .filter_map(|r| match r {
            RangeEdit::Keep(kept) => Some(kept.duration().0),
            RangeEdit::Remove(_) => None,
        })
        .sum();
    Micros(kept)
}

/// A length in seconds with one decimal.
fn seconds(t: Micros) -> String {
    format!("{:.1} s", t.0 as f64 / 1_000_000.0)
}

impl TakesView {
    pub fn new(
        folder: Entity<VideoFolder>,
        import: Entity<import::Import>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&import, |_, _, cx| cx.notify()).detach();
        cx.observe(&folder, |this, _, cx| {
            this.reload(cx);
            // The clip may have changed with the cut.
            this.player.unload();
            this.playing.clear();
            cx.notify();
        })
        .detach();
        let mut view = Self {
            folder,
            import,
            takes: HashMap::new(),
            newest: None,
            lineup: Lineup::default(),
            rows: Vec::new(),
            at: (0, 0),
            mark: None,
            peaks: HashMap::new(),
            player: Player::default(),
            playing: Vec::new(),
            note: None,
            error: None,
            focus: cx.focus_handle(),
            list: ScrollHandle::new(),
            scrolled: None,
            _blur: None,
        };
        view.reload(cx);
        view.at = view.first_focus(cx);
        view
    }

    /// Reads the words and elements of every take in the media folder, and lines them up.
    fn reload(&mut self, cx: &App) {
        let folder = self.folder.read(cx);
        let ids: Vec<Id> = std::fs::read_dir(folder.dir.join("media"))
            .map(|entries| {
                entries
                    .filter_map(|e| Id::new(e.ok()?.file_name().into_string().ok()?))
                    .collect()
            })
            .unwrap_or_default();
        self.takes.retain(|id, _| ids.contains(id));
        self.newest = ids
            .iter()
            .max_by_key(|id| (id.as_str().len(), id.as_str()))
            .cloned();
        for id in ids {
            let transcript =
                folder
                    .transcripts
                    .get(&id)
                    .cloned()
                    .or_else(|| match self.takes.get(&id) {
                        Some(known) => Some(known.transcript.clone()),
                        None => shot_analyze::cache::read_cached(&folder.dir, &id)
                            .ok()
                            .flatten(),
                    });
            let Some(transcript) = transcript else {
                self.takes.remove(&id);
                continue;
            };
            let path = folder.dir.join(format!("elements/{id}.jsonl"));
            let elements = folder.elements.get(&id).cloned().or_else(|| {
                let text = std::fs::read_to_string(path).ok()?;
                Elements::parse(id.clone(), &text).ok()
            });
            self.takes.insert(
                id,
                TakeData {
                    transcript,
                    elements,
                },
            );
        }
        let words: Vec<TakeWords<'_>> = self
            .takes
            .values()
            .map(|t| TakeWords {
                transcript: &t.transcript,
                elements: t.elements.as_ref(),
            })
            .collect();
        self.lineup = Lineup::new(&folder.project.script, &folder.project.cut, &words);
        self.rows = self.lineup.rows().map(|(at, _)| at).collect();
        let row = self.at.0.min(self.rows.len().saturating_sub(1));
        let attempts = self.row_at(row).map_or(0, |r| r.attempts.len());
        self.at = (row, self.at.1.min(attempts.saturating_sub(1)));
    }

    fn row_at(&self, index: usize) -> Option<&Row> {
        self.lineup.row(*self.rows.get(index)?)
    }

    /// The first row with attempts, on the attempt the cut keeps most of.
    fn first_focus(&self, cx: &App) -> (usize, usize) {
        let cut = &self.folder.read(cx).played;
        self.rows
            .iter()
            .enumerate()
            .find_map(|(i, at)| {
                let row = self.lineup.row(*at)?;
                (!row.attempts.is_empty()).then(|| (i, held(cut, row).unwrap_or(0)))
            })
            .unwrap_or((0, 0))
    }

    /// Whether the cut plays more than one take's attempt at some beat, which nobody has
    /// composed yet: then the Cut screen opens on this view.
    pub fn needs_composing(&self, cx: &App) -> bool {
        needs_composing(&self.lineup, &self.folder.read(cx).played)
    }

    /// After a new recording: says which beats the newest take repeats and focuses the first
    /// of them. True when it repeats any, so the Cut screen shows this view.
    pub fn recorded(&mut self, cx: &mut Context<Self>) -> bool {
        self.reload(cx);
        // Take numbers only grow, so the new recording has the highest.
        let Some(take) = self.newest.clone() else {
            return false;
        };
        let take = &take;
        let repeated = repeated_rows(&self.lineup, take);
        let Some(&first) = repeated.first() else {
            return false;
        };
        let new = self
            .row_at(first)
            .and_then(|r| r.attempts.iter().position(|a| a.take == *take))
            .unwrap_or(0);
        self.at = (first, new);
        self.mark = None;
        let beats = match repeated.len() {
            1 => "1 beat".to_owned(),
            n => format!("{n} beats"),
        };
        self.note = Some(format!(
            "{} repeats {beats} of earlier takes, and the cut plays both for now. Pick the \
             better attempt per beat, or assemble the cut from your picks.",
            take_name(take)
        ));
        cx.notify();
        true
    }

    fn focused(&self) -> Option<(RowAt, &Row, &Attempt)> {
        let at = *self.rows.get(self.at.0)?;
        let row = self.lineup.row(at)?;
        Some((at, row, row.attempts.get(self.at.1)?))
    }

    fn words_of(&self, attempt: &Attempt) -> &[Word] {
        self.takes
            .get(&attempt.take)
            .map_or(&[], |t| words_in(t.transcript.words(), attempt.range))
    }

    /// The focused attempt, or the marked part of it.
    fn chosen(&self) -> Option<Attempt> {
        let (_, _, attempt) = self.focused()?;
        let Some(mark) = self.mark else {
            return Some(attempt.clone());
        };
        let words = self.words_of(attempt);
        let marked = &words[mark.words()];
        Some(Attempt {
            take: attempt.take.clone(),
            range: TimeRange::new(marked[0].range.from(), marked[marked.len() - 1].range.to())?,
        })
    }

    /// The attempt the focused one is compared with: the one the cut keeps most of when that
    /// is another, else the next one in the row.
    fn partner(&self, cx: &App) -> Option<usize> {
        let (_, row, _) = self.focused()?;
        if row.attempts.len() < 2 {
            return None;
        }
        let cut = &self.folder.read(cx).played;
        match held(cut, row) {
            Some(held) if held != self.at.1 => Some(held),
            _ => Some((self.at.1 + 1) % row.attempts.len()),
        }
    }

    fn step_row(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let row = self.at.0.saturating_add_signed(by).min(self.rows.len() - 1);
        if row == self.at.0 {
            return;
        }
        // Stay in the same take where the next row has it.
        let take = self.focused().map(|(_, _, a)| a.take.clone());
        let attempts = self.row_at(row).map_or(&[][..], |r| &r.attempts[..]);
        let attempt = take
            .and_then(|take| attempts.iter().position(|a| a.take == take))
            .unwrap_or_else(|| self.at.1.min(attempts.len().saturating_sub(1)));
        self.focus_on(row, attempt, cx);
    }

    fn step_attempt(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(row) = self.row_at(self.at.0) else {
            return;
        };
        if row.attempts.is_empty() {
            return;
        }
        let attempt = self
            .at
            .1
            .saturating_add_signed(by)
            .min(row.attempts.len() - 1);
        self.focus_on(self.at.0, attempt, cx);
    }

    fn focus_on(&mut self, row: usize, attempt: usize, cx: &mut Context<Self>) {
        if (row, attempt) != self.at {
            self.at = (row, attempt);
            self.mark = None;
            self.player.pause();
        }
        cx.notify();
    }

    /// Grows (`more`) or shrinks the mark by a word at its moving end. Without a mark, starts one
    /// on the first word, or shrinking, on the last.
    fn step_mark(&mut self, more: bool, cx: &mut Context<Self>) {
        let Some((_, _, attempt)) = self.focused() else {
            return;
        };
        let count = self.words_of(attempt).len();
        if count == 0 {
            return;
        }
        self.mark = Some(match self.mark {
            None if more => Mark { anchor: 0, end: 0 },
            None => Mark {
                anchor: count - 1,
                end: count - 1,
            },
            Some(mark) => Mark {
                end: if more {
                    (mark.end + 1).min(count - 1)
                } else {
                    mark.end.saturating_sub(1)
                },
                ..mark
            },
        });
        self.player.pause();
        cx.notify();
    }

    /// A click on word `index` of attempt `attempt` in row `row`: marks it, or with Shift
    /// extends the mark to it.
    fn click_word(
        &mut self,
        row: usize,
        attempt: usize,
        index: usize,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        let same = self.at == (row, attempt);
        self.focus_on(row, attempt, cx);
        self.mark = Some(match self.mark {
            Some(mark) if same && shift => Mark { end: index, ..mark },
            _ => Mark {
                anchor: index,
                end: index,
            },
        });
        cx.notify();
    }

    /// Loads `attempts` back to back as they would play in the cut and plays them, or pauses
    /// when they play already.
    fn play(&mut self, attempts: Vec<Attempt>, cx: &mut Context<Self>) {
        if attempts.is_empty() {
            return;
        }
        if self.playing == attempts && self.player.loaded().is_some() {
            if self.player.is_playing() {
                self.player.pause();
            } else {
                self.player.play();
            }
            cx.notify();
            return;
        }
        let folder = self.folder.read(cx);
        let clip = clip(&folder.played, &attempts);
        let key = (attempts[0].take.clone(), attempts[0].range);
        // A key of the first attempt alone would not tell one attempt from a pair.
        self.player.unload();
        match self.player.load_clip(folder, key, clip) {
            Ok(()) => {
                self.player.play();
                self.playing = attempts;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// Shows the first frame of the focused attempt, or the marked words, while nothing plays.
    fn cue(&mut self, cx: &mut Context<Self>) {
        if self.player.is_playing() {
            return;
        }
        let Some(chosen) = self.chosen() else {
            return;
        };
        if self.playing.len() == 1 && self.playing[0] == chosen {
            return;
        }
        let folder = self.folder.read(cx);
        let clip = clip(&folder.played, std::slice::from_ref(&chosen));
        let key = (chosen.take.clone(), chosen.range);
        match self.player.load_clip(folder, key, clip) {
            Ok(()) => self.playing = vec![chosen],
            Err(error) => self.error = Some(error),
        }
    }

    fn play_focused(&mut self, cx: &mut Context<Self>) {
        if let Some(chosen) = self.chosen() {
            self.play(vec![chosen], cx);
        }
    }

    fn compare(&mut self, cx: &mut Context<Self>) {
        let (Some(chosen), Some(partner)) = (self.chosen(), self.partner(cx)) else {
            self.note = Some("This beat has one attempt, so there is nothing to compare.".into());
            cx.notify();
            return;
        };
        let other = self.row_at(self.at.0).map(|r| r.attempts[partner].clone());
        self.play(std::iter::once(chosen).chain(other).collect(), cx);
    }

    /// Writes `cut`, made from the files as they are, as one change.
    fn write(
        &mut self,
        note: Option<String>,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut VideoFolder) -> Result<(), String>,
    ) {
        let result = self.folder.update(cx, |folder, cx| {
            let result = folder.as_one_change(edit);
            cx.notify();
            result
        });
        match result {
            Ok(()) => {
                self.error = None;
                if note.is_some() {
                    self.note = note;
                }
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn rate(&mut self, stars: u8, cx: &mut Context<Self>) {
        let Some((_, _, attempt)) = self.focused() else {
            return;
        };
        let attempt = attempt.clone();
        let cut = &self.folder.read(cx).project.cut;
        // The same stars again take them away.
        let stars = (cut.rating(&attempt).and_then(|r| r.stars) != Some(stars)).then_some(stars);
        let cut = cut.rate(&attempt, stars);
        self.write(None, cx, |folder| folder.write_cut(cut));
    }

    fn pick(&mut self, cx: &mut Context<Self>) {
        let (Some((_, row, attempt)), Some(chosen)) = (self.focused(), self.chosen()) else {
            return;
        };
        let cut = &self.folder.read(cx).project.cut;
        let circled = cut
            .rating(attempt)
            .is_some_and(|r| r.pick && r.range == chosen.range);
        let cut = cut.set_pick(row, (!circled).then_some(&chosen));
        self.write(None, cx, |folder| folder.write_cut(cut));
    }

    fn use_this(&mut self, cx: &mut Context<Self>) {
        let (Some((at, _, _)), Some(chosen)) = (self.focused(), self.chosen()) else {
            return;
        };
        let folder = self.folder.read(cx);
        let cut = folder.project.cut.use_attempt(&self.lineup, at, &chosen);
        let what = if self.mark.is_some() {
            "The marked words of"
        } else {
            "The attempt of"
        };
        let note = format!(
            "{what} {} now hold this beat in the cut.",
            take_name(&chosen.take)
        );
        self.write(Some(note), cx, |folder| {
            folder.write_cut(cut.map_err(|e| e.to_string())?)
        });
        self.mark = None;
    }

    fn assemble(&mut self, cx: &mut Context<Self>) {
        let folder = self.folder.read(cx);
        let cut = &folder.project.cut;
        let mut counts: Vec<(Id, usize)> = Vec::new();
        for (_, row) in self.lineup.rows() {
            if let Some(choice) = cut.choice(row) {
                match counts.iter_mut().find(|(take, _)| *take == choice.take) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((choice.take, 1)),
                }
            }
        }
        counts.sort_by(|a, b| {
            a.0.as_str()
                .len()
                .cmp(&b.0.as_str().len())
                .then(a.0.cmp(&b.0))
        });
        let assembled = cut.assemble(&self.lineup);
        let beats: usize = counts.iter().map(|(_, n)| n).sum();
        let from = counts
            .iter()
            .map(|(take, n)| format!("{} for {n}", take_name(take)))
            .collect::<Vec<_>>()
            .join(", ");
        let note = format!("Assembled {beats} beats from your picks: {from}. ⌘Z undoes it.");
        self.write(Some(note), cx, |folder| {
            folder.write_cut(assembled.map_err(|e| e.to_string())?)
        });
        self.mark = None;
    }

    /// On to Edit, or to Record while there is nothing to cut.
    fn go_on(&mut self, cx: &mut Context<Self>) {
        self.player.pause();
        let screen = if self.folder.read(cx).project.cut.chapters.is_empty() {
            Screen::Record
        } else {
            Screen::Edit
        };
        cx.emit(GoTo(screen));
    }

    /// Reads the peaks of `take`'s mic on a background thread, once.
    fn load_peaks(&mut self, take: &Id, cx: &mut Context<Self>) {
        if self.peaks.contains_key(take) {
            return;
        }
        self.peaks.insert(take.clone(), None);
        let folder = self.folder.read(cx);
        let path = folder.media(take, "mic.mp4");
        let length = self
            .takes
            .get(take)
            .and_then(|t| t.transcript.words().last())
            .map_or(Micros(0), |w| Micros(w.range.to().0 + 1_000_000));
        if !path.exists() {
            return;
        }
        let take = take.clone();
        cx.spawn(async move |this, cx| {
            let peaks = cx
                .background_spawn(async move { Peaks::read(&path, length) })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(peaks) = peaks {
                    this.peaks.insert(take, Some(Arc::new(peaks)));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Whether the cut plays more than one take's attempt at some beat.
fn needs_composing(lineup: &Lineup, cut: &Cut) -> bool {
    lineup.rows().any(|(_, row)| in_cut(cut, row).len() > 1)
}

/// The rows, by their place in order, where `take` has an attempt and an earlier take has one
/// too.
fn repeated_rows(lineup: &Lineup, take: &Id) -> Vec<usize> {
    lineup
        .rows()
        .enumerate()
        .filter(|(_, (_, row))| {
            row.attempts.iter().any(|a| a.take == *take)
                && row.attempts.iter().any(|a| a.take != *take)
        })
        .map(|(i, _)| i)
        .collect()
}

/// A cut that plays `attempts` back to back as they would play in `cut`.
fn clip(cut: &Cut, attempts: &[Attempt]) -> Cut {
    Cut {
        music: None,
        ratings: Vec::new(),
        overrides: Vec::new(),
        derived: Vec::new(),
        chapters: vec![Chapter {
            title: String::new(),
            takes: attempts
                .iter()
                .map(|a| cut.material(&a.take, a.range))
                .collect(),
        }],
        ..cut.clone()
    }
}

/// The attempts of `row` the cut keeps media of, by index.
fn in_cut(cut: &Cut, row: &Row) -> Vec<usize> {
    row.attempts
        .iter()
        .enumerate()
        .filter(|(_, a)| cut.kept_of(&a.take, a.range).0 > 0)
        .map(|(i, _)| i)
        .collect()
}

/// The attempt of `row` the cut keeps most of.
fn held(cut: &Cut, row: &Row) -> Option<usize> {
    row.attempts
        .iter()
        .enumerate()
        .map(|(i, a)| (cut.kept_of(&a.take, a.range), i))
        .filter(|(kept, _)| kept.0 > 0)
        .max_by_key(|(kept, i)| (*kept, std::cmp::Reverse(*i)))
        .map(|(_, i)| i)
}

/// The words of an attempt as one text, the words that differ from the compared attempt
/// lit, the marked ones on the selection tint, and each word's byte range for clicks.
fn attempt_text(
    words: &[Word],
    differs: Option<&[bool]>,
    mark: Option<RangeInclusive<usize>>,
) -> (StyledText, Vec<std::ops::Range<usize>>) {
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(words.len());
    let mut highlights = Vec::new();
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(word.text.trim());
        let range = start..text.len();
        let marked = mark.as_ref().is_some_and(|m| m.contains(&i));
        let differ = differs.is_some_and(|d| d.get(i) == Some(&true));
        // The tint of a mark runs on over the space to the next marked word.
        let tinted = if marked && mark.as_ref().is_some_and(|m| m.contains(&(i + 1))) {
            range.start..range.end + 1
        } else {
            range.clone()
        };
        if marked || differ {
            highlights.push((
                tinted,
                HighlightStyle {
                    color: Some(if differ { INSERTED } else { STRONG }.into()),
                    background_color: marked.then(|| SELECTION.into()),
                    font_weight: differ.then_some(FontWeight::MEDIUM),
                    ..HighlightStyle::default()
                },
            ));
        }
        ranges.push(range);
    }
    (StyledText::new(text).with_highlights(highlights), ranges)
}

/// Five stars, the first `stars` lit.
fn stars(stars: Option<u8>, size: gpui::Pixels) -> Div {
    let lit = stars.unwrap_or(0);
    div().flex().gap(px(1.)).children((1..=5).map(|n| {
        if n <= lit {
            icon("star-on", size, ACCENT)
        } else {
            icon("star", size, FAINT)
        }
    }))
}

/// A small tag such as "In the cut".
fn tag(text: &'static str, on: bool) -> Div {
    div()
        .px(px(7.))
        .py(px(1.))
        .rounded(px(6.))
        .text_size(TEXT_XS)
        .font_weight(FontWeight::MEDIUM)
        .bg(if on {
            SELECTION.into()
        } else {
            Hsla::from(FLOAT)
        })
        .text_color(if on { ACCENT } else { MUTED })
        .child(text)
}

impl TakesView {
    /// One attempt of the row at `row`, side by side with the others.
    fn card(
        &self,
        row: usize,
        index: usize,
        diff: Option<&[bool]>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let row_data = self.row_at(row).expect("rendered rows exist");
        let attempt = &row_data.attempts[index];
        let folder = self.folder.read(cx);
        let cut = &folder.played;
        let focused = self.at == (row, index);
        let rating = cut.rating(attempt);
        let picked = rating.is_some_and(|r| r.pick);
        let kept = cut.kept_of(&attempt.take, attempt.range);
        let whole = kept.0 * 2 >= attempt.range.duration().0;
        let words = self.words_of(attempt);
        let mark = focused.then_some(self.mark).flatten().map(Mark::words);
        let (text, ranges) = attempt_text(words, diff, mark.clone());
        let peaks = self.peaks.get(&attempt.take).cloned().flatten();
        // Removed media of the take's blocks shows dim in the waveform.
        let block = cut.material(&attempt.take, attempt.range);
        let span = attempt.range.duration().0.max(1);
        let bars: Vec<(f32, bool)> = (0..BARS as u64)
            .map(|i| {
                let from = Micros(attempt.range.from().0 + i * span / BARS as u64);
                let to = Micros(attempt.range.from().0 + (i + 1) * span / BARS as u64);
                let height = peaks.as_ref().map_or(0.0, |p| p.over(from, to).clamp(0.0, 1.0));
                let middle = Micros((from.0 + to.0) / 2);
                let removed = block.ranges.iter().any(|r| {
                    matches!(r, RangeEdit::Remove(range) if range.from() <= middle && middle < range.to())
                });
                (height, removed)
            })
            .collect();
        let playhead = self.player.source().and_then(|(at, t)| {
            let playing = self.playing.get(at.take)?;
            (self.player.is_playing()
                && playing.take == attempt.take
                && playing.range.overlaps(attempt.range)
                && attempt.range.from() <= t
                && t < attempt.range.to())
            .then(|| (t.0 - attempt.range.from().0) as f32 / span as f32)
        });
        let marked_range = mark.and_then(|m| {
            let (first, last) = (words.get(*m.start())?, words.get(*m.end())?);
            let at = |t: Micros| (t.0.saturating_sub(attempt.range.from().0)) as f32 / span as f32;
            Some(at(first.range.from())..at(last.range.to()))
        });
        let wave = div()
            .relative()
            .h(px(26.))
            .flex()
            .items_center()
            .gap(px(1.))
            .children(bars.into_iter().map(|(h, removed)| {
                div()
                    .flex_1()
                    .rounded(px(1.))
                    .bg(if removed { FAINT } else { WAVE })
                    .h(px(2. + h * 24.))
            }))
            .when_some(marked_range, |this, range| {
                this.child(
                    div()
                        .absolute()
                        .top(px(-3.))
                        .bottom(px(-3.))
                        .left(relative(range.start))
                        .w(relative((range.end - range.start).max(0.005)))
                        .rounded(px(4.))
                        .bg(SELECTION),
                )
            })
            .when_some(playhead, |this, at| {
                this.child(
                    div()
                        .absolute()
                        .top(px(-4.))
                        .bottom(px(-4.))
                        .left(relative(at))
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(ACCENT),
                )
            });
        let click = cx.listener(move |this, _, _, cx| this.focus_on(row, index, cx));
        let this = cx.entity().downgrade();
        let words_view = InteractiveText::new(("attempt-words", row * 64 + index), text).on_click(
            ranges,
            move |word, window, cx| {
                let shift = window.modifiers().shift;
                this.update(cx, |this, cx| this.click_word(row, index, word, shift, cx))
                    .ok();
            },
        );
        div()
            .id(("attempt", row * 64 + index))
            .flex_1()
            .min_w(px(220.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(RADIUS_MD)
            .bg(if focused { RAISED } else { CARD })
            .border_1()
            .border_color(if focused {
                Hsla::from(ACCENT).opacity(0.7)
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .on_click(click)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if focused { STRONG } else { TEXT })
                            .child(take_name(&attempt.take)),
                    )
                    .child(
                        mono(seconds(played(cut, attempt)))
                            .text_size(TEXT_SM)
                            .text_color(MUTED),
                    )
                    .when(kept.0 > 0, |this| {
                        this.child(tag(
                            if whole {
                                "In the cut"
                            } else {
                                "Part in the cut"
                            },
                            whole,
                        ))
                    })
                    .child(div().flex_1())
                    .child(stars(rating.and_then(|r| r.stars), ICON_SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(TEXT_XS)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ACCENT)
                            .child(icon("pick", ICON, if picked { ACCENT } else { FAINT }))
                            .when(picked, |this| this.child("Picked")),
                    ),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(21.))
                    .text_color(BODY)
                    .when(!focused, |this| this.line_clamp(3).text_ellipsis())
                    .child(words_view),
            )
            .child(wave)
    }

    fn row_view(&self, row: usize, cx: &mut Context<Self>) -> Div {
        let data = self.row_at(row).expect("rendered rows exist").clone();
        let number = row + 1;
        let cut = &self.folder.read(cx).played;
        let held = in_cut(cut, &data);
        let focused_row = self.at.0 == row;
        // In the focused row the words that differ from the compared attempt are lit.
        let diffs: Option<(usize, Vec<bool>, usize, Vec<bool>)> = focused_row
            .then(|| {
                let partner = self.partner(cx)?;
                let focus = self.at.1;
                let words = |i: usize| -> Vec<&str> {
                    self.words_of(&data.attempts[i])
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect()
                };
                let (a, b) = word_diff(&words(focus), &words(partner));
                Some((focus, a, partner, b))
            })
            .flatten();
        let status = match held.len() {
            0 if data.attempts.is_empty() => None,
            0 => Some(("Not in the cut", MUTED)),
            1 => None,
            _ => Some(("In the cut more than once", WARN)),
        };
        let cards: Vec<gpui::Stateful<Div>> = (0..data.attempts.len())
            .map(|i| {
                let diff = diffs.as_ref().and_then(|(f, a, p, b)| {
                    if i == *f {
                        Some(a.as_slice())
                    } else if i == *p {
                        Some(b.as_slice())
                    } else {
                        None
                    }
                });
                self.card(row, i, diff, cx)
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(14.))
            .py(px(12.))
            .rounded(RADIUS_LG)
            .when(focused_row, |this| {
                this.bg(Hsla::from(RAISED).opacity(0.45))
            })
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        mono(format!("{number:02}"))
                            .text_size(TEXT_SM)
                            .text_color(FAINT),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(if focused_row { STRONG } else { TEXT })
                            .font_weight(FontWeight::MEDIUM)
                            .child(data.text.clone()),
                    )
                    .when_some(status, |this, (text, color)| {
                        this.child(div().text_size(TEXT_SM).text_color(color).child(text))
                    }),
            )
            .child(if cards.is_empty() {
                div()
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child("No take has this beat yet.")
            } else {
                div().flex().gap(px(8.)).children(cards)
            })
    }

    fn list_view(&mut self, cx: &mut Context<Self>) -> Div {
        let mut children: Vec<gpui::AnyElement> = Vec::new();
        let mut row = 0;
        let mut focused_child = 0;
        for (g, group) in self.lineup.groups.iter().enumerate() {
            children.push(
                div()
                    .pt(px(if g == 0 { 4. } else { 18. }))
                    .pb(px(2.))
                    .px(px(14.))
                    .child(label(group.title.clone()))
                    .into_any_element(),
            );
            for _ in &group.rows {
                if row == self.at.0 {
                    focused_child = children.len();
                }
                children.push(self.row_view(row, cx).into_any_element());
                row += 1;
            }
        }
        if self.scrolled != Some(self.at.0) {
            self.scrolled = Some(self.at.0);
            self.list.scroll_to_item(focused_child);
        }
        scrolling_list(
            "takes-rows",
            &self.list,
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .pb(px(16.))
                .children(children),
        )
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let cut = &self.folder.read(cx).played;
        let takes = match self.lineup.takes().len() {
            1 => "1 take".to_owned(),
            n => format!("{n} takes"),
        };
        let twice = self
            .lineup
            .rows()
            .filter(|(_, row)| in_cut(cut, row).len() > 1)
            .count();
        let picked = self
            .lineup
            .rows()
            .filter(|(_, row)| {
                row.attempts
                    .iter()
                    .any(|a| cut.rating(a).is_some_and(|r| r.pick))
            })
            .count();
        let summary = match twice {
            0 => format!("{takes}, {} beats, {picked} picked", self.rows.len()),
            n => format!(
                "{takes}, {} beats, {picked} picked, {n} in the cut more than once",
                self.rows.len()
            ),
        };
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Takes"),
                    )
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(summary)),
            )
            .child(
                big_button("assemble", "takes", "Assemble picks", "⇧↩", twice > 0)
                    .on_click(cx.listener(|this, _, _, cx| this.assemble(cx))),
            )
    }

    fn side(&self, cx: &mut Context<Self>) -> Div {
        let frame = self.player.frame().map(|f| f.buffer.clone());
        let playing = self.player.is_playing();
        let comparing = playing && self.playing.len() > 1;
        let focused = self
            .focused()
            .map(|(at, row, a)| (at, row.clone(), a.clone()));
        let partner = self.partner(cx);
        let marked = self.mark.and_then(|_| self.chosen());
        let has_mark = marked.is_some();
        let title = match &focused {
            Some((_, _, a)) => format!("{}, beat {}", take_name(&a.take), self.at.0 + 1),
            None => "No attempt".to_owned(),
        };
        let compare_to = partner
            .zip(focused.as_ref())
            .map(|(p, (_, row, _))| take_name(&row.attempts[p].take));
        let action =
            |id: &'static str,
             icon_name: &'static str,
             text: String,
             keys: &'static str,
             accent: bool| { big_button(id, icon_name, text, keys, accent).w_full() };
        div()
            .w(px(SIDE))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .h(px(SIDE * 9. / 16.))
                    .rounded(RADIUS_LG)
                    .overflow_hidden()
                    .bg(gpui::black())
                    .children(frame.map(|f| surface(f).size_full().object_fit(ObjectFit::Contain))),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .children(focused.as_ref().map(|(_, _, a)| {
                        mono(seconds(played(&self.folder.read(cx).played, a)))
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                    })),
            )
            .children(marked.map(|part| {
                div().text_size(TEXT_SM).text_color(BODY).child(format!(
                    "{} marked. Use this puts only them into the cut.",
                    seconds(part.range.duration())
                ))
            }))
            .child(
                action(
                    "takes-play",
                    if playing && !comparing {
                        "pause"
                    } else {
                        "play"
                    },
                    if playing && !comparing {
                        "Pause".to_owned()
                    } else if has_mark {
                        "Play the marked words".to_owned()
                    } else {
                        "Play this attempt".to_owned()
                    },
                    "Space",
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.play_focused(cx))),
            )
            .child(
                action(
                    "takes-compare",
                    if comparing { "pause" } else { "compare" },
                    match (&compare_to, comparing) {
                        (_, true) => "Pause".to_owned(),
                        (Some(other), false) => format!("Compare with {other}"),
                        (None, false) => "Compare".to_owned(),
                    },
                    "A",
                    false,
                )
                .when(compare_to.is_none(), |this| this.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| this.compare(cx))),
            )
            .child(
                action(
                    "takes-use",
                    "check",
                    if has_mark {
                        "Use the marked words".to_owned()
                    } else {
                        "Use this".to_owned()
                    },
                    "↩",
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| this.use_this(cx))),
            )
            .child(
                div()
                    .pt(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(hint("↑ ↓", "Beats"))
                    .child(hint("← →", "Attempts"))
                    .child(hint("1–5", "Rate, the same again clears"))
                    .child(hint("P", "Circle as the pick"))
                    .child(hint("⇧← ⇧→", "Mark words, or click and ⇧click"))
                    .child(hint("Esc", "Clear the mark")),
            )
    }
}

impl Render for TakesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._blur.is_none() {
            self._blur = Some(cx.on_focus_out(&self.focus, window, |this, _, _, cx| {
                this.player.pause();
                cx.notify();
            }));
        }
        self.cue(cx);
        if self.player.advance() || self.player.is_playing() {
            window.request_animation_frame();
        }
        if let Some(error) = self.player.error() {
            self.error.get_or_insert(error);
        }
        for take in self.lineup.takes() {
            self.load_peaks(&take, cx);
        }
        let root = div()
            .id("takes")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .on_action(cx.listener(|this, _: &NextBeat, _, cx| this.step_row(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousBeat, _, cx| this.step_row(-1, cx)))
            .on_action(cx.listener(|this, _: &NextAttempt, _, cx| this.step_attempt(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousAttempt, _, cx| this.step_attempt(-1, cx)))
            .on_action(cx.listener(|this, _: &Play, _, cx| this.play_focused(cx)))
            .on_action(cx.listener(|this, _: &Compare, _, cx| this.compare(cx)))
            .on_action(cx.listener(|this, _: &Rate1, _, cx| this.rate(1, cx)))
            .on_action(cx.listener(|this, _: &Rate2, _, cx| this.rate(2, cx)))
            .on_action(cx.listener(|this, _: &Rate3, _, cx| this.rate(3, cx)))
            .on_action(cx.listener(|this, _: &Rate4, _, cx| this.rate(4, cx)))
            .on_action(cx.listener(|this, _: &Rate5, _, cx| this.rate(5, cx)))
            .on_action(cx.listener(|this, _: &Pick, _, cx| this.pick(cx)))
            .on_action(cx.listener(|this, _: &UseThis, _, cx| this.use_this(cx)))
            .on_action(cx.listener(|this, _: &Assemble, _, cx| this.assemble(cx)))
            .on_action(cx.listener(|this, _: &MarkMore, _, cx| this.step_mark(true, cx)))
            .on_action(cx.listener(|this, _: &MarkLess, _, cx| this.step_mark(false, cx)))
            .on_action(cx.listener(|this, _: &ClearMark, _, cx| {
                this.mark = None;
                this.note = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Continue, _, cx| this.go_on(cx)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(SELECTION))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.import
                    .update(cx, |import, cx| import.drop_paths(paths, cx))
            }))
            .children(self.import.read(cx).row());
        if self.takes.is_empty() || self.lineup.takes().is_empty() {
            let untranscribed = std::fs::read_dir(self.folder.read(cx).dir.join("media"))
                .is_ok_and(|mut entries| entries.next().is_some());
            let (title, text) = if untranscribed {
                (
                    "No words to line up yet",
                    "The Takes view lines your takes up beat by beat by what you said. \
                     Transcribe them on Review, then come back.",
                )
            } else {
                (
                    "Nothing recorded yet",
                    "Record the script, and again for the beats you want to try once more. \
                     Every take's attempts show here side by side.",
                )
            };
            let action = if untranscribed {
                big_button(
                    "takes-transcribe",
                    "text",
                    "Transcribe on Review",
                    "⌥1",
                    true,
                )
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::cut::ShowReview), cx)
                })
            } else {
                big_button("takes-record", "record", "Record a take", "⌘↩", true)
                    .on_click(cx.listener(|this, _, _, cx| this.go_on(cx)))
            };
            return root.child(
                pane().flex_1().flex().flex_col().child(
                    empty_state("takes", title, text).child(div().pt(px(14.)).child(action)),
                ),
            );
        }
        let one_take = self.lineup.takes().len() == 1;
        let header = self.header(cx);
        let list = self.list_view(cx);
        let side = self.side(cx);
        let banner = self
            .error
            .clone()
            .map(|e| crate::ui::alert(e).into_any_element())
            .or_else(|| {
                let text = self.note.clone().or_else(|| {
                    one_take.then(|| {
                        "One take so far. Record again, and its attempts line up here next to \
                         this one, beat by beat."
                            .to_owned()
                    })
                })?;
                Some(
                    div()
                        .mx(px(14.))
                        .px(px(12.))
                        .py(px(9.))
                        .rounded(RADIUS_MD)
                        .bg(RAISED)
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(TEXT_SM)
                        .text_color(BODY)
                        .child(icon("takes", ICON_SM, MUTED))
                        .child(div().flex_1().child(text))
                        .when(self.note.is_some(), |this| this.child(kbd("Esc")))
                        .into_any_element(),
                )
            });
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap(px(8.))
                .child(
                    pane()
                        .flex_1()
                        .min_w(px(480.))
                        .pt(px(18.))
                        .px(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(header)
                        .children(banner)
                        .child(list),
                )
                .child(pane().flex_none().p(px(16.)).child(side)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::Group;

    fn attempt(take: &str, from: u64, to: u64) -> Attempt {
        Attempt {
            take: Id::new(take).unwrap(),
            range: TimeRange::new(Micros(from), Micros(to)).unwrap(),
        }
    }

    /// Beat one in both takes, beat two only in the first, beat three only in the second.
    fn lineup() -> Lineup {
        let row = |attempts| Row {
            text: String::new(),
            attempts,
        };
        Lineup {
            groups: vec![Group {
                title: "A".into(),
                rows: vec![
                    row(vec![attempt("t01", 0, 10), attempt("t02", 0, 10)]),
                    row(vec![attempt("t01", 20, 30)]),
                    row(vec![attempt("t02", 40, 50)]),
                ],
            }],
        }
    }

    #[test]
    fn a_new_take_lands_on_the_beats_it_repeats() {
        let lineup = lineup();
        assert_eq!(repeated_rows(&lineup, &Id::new("t02").unwrap()), [0]);
        assert!(repeated_rows(&lineup, &Id::new("t03").unwrap()).is_empty());
    }

    #[test]
    fn the_takes_open_while_the_cut_plays_a_beat_twice() {
        let both = Cut::parse(
            "chapter \"A\" {\n    take t01 {\n        keep from=0 to=30\n    }\n}\n\nchapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=50\n    }\n}\n",
        )
        .unwrap();
        assert!(needs_composing(&lineup(), &both));
        let composed = both
            .use_attempt(
                &lineup(),
                RowAt { group: 0, row: 0 },
                &attempt("t02", 0, 10),
            )
            .unwrap();
        assert!(!needs_composing(&lineup(), &composed));
    }
}
