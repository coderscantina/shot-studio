//! The Cut screen, where the auto-cut rules' rough cut is reviewed right after recording. The
//! middle shows one review item at a time: what the rule found and why it asks, the words around
//! it, its picture and waveform. Space plays it with a little of what plays before and after,
//! ⇧Space plays it as it would sound cut. K keeps it, C cuts it, and the next item plays. The
//! rules with their switches and the pacing sit on the left, the queue on the right. Once
//! nothing waits, the cuts the rules applied on their own can be stepped through the same way:
//! K restores the words, C leaves them cut.
//!
//! The queue is the `review` nodes of `cut.kdl`, or a migrated video's `review` instances of
//! its lock, read through [`Cut::review_queue`] of the shown cut; every answer goes through
//! [`VideoFolder::decide`], which writes `ignore` or a rule removal before migration and an
//! `override { keep }` or `{ cut }` after. The screen keeps only which item is on show and the
//! answers given since it opened, so the list can show them dimmed. An answer undone with ⌘Z
//! brings its item back on show.

pub mod player;
pub mod run;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle,
    Hsla, KeyBinding, ObjectFit, ScrollHandle, SharedString, StrikethroughStyle, StyledText,
    Subscription, Window, actions, div, prelude::*, px, relative, surface,
};
use shot_analyze::autocut::{self, RuleCount};
use shot_analyze::{Speech, cache};
use shot_core::{
    Cut, Decision, Id, Micros, Pacing, RangeEdit, RemovedBy, Review, RuleKind, TakeAt, TimeRange,
    Word,
};

use crate::folder::VideoFolder;
use crate::model::{self, FPS};
use crate::peaks::Peaks;
use crate::studio::Screen;
use crate::theme::*;
use crate::transcribe::{Transcribe, Transcribed};
use crate::ui::{Segment, icon, kbd, label, mono, pane, segmented};
use player::{Hear, ItemKey, Player};

const CONTEXT: &str = "CutScreen";
/// Words of context shown on each side of an item.
const CONTEXT_WORDS: usize = 8;
/// Bars of the waveform under an item.
const BARS: usize = 120;
/// Line height of the words around a review item.
const CONTEXT_LINE: f32 = 30.;

actions!(
    cut,
    [
        KeepItem,
        CutItem,
        NextItem,
        PreviousItem,
        PlayAround,
        PlayCut,
        Continue,
        ReviewApplied
    ]
);

pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("k", KeepItem, context),
        KeyBinding::new("c", CutItem, context),
        KeyBinding::new("down", NextItem, context),
        KeyBinding::new("up", PreviousItem, context),
        KeyBinding::new("space", PlayAround, context),
        KeyBinding::new("shift-space", PlayCut, context),
        KeyBinding::new("cmd-enter", Continue, context),
        KeyBinding::new("r", ReviewApplied, context),
    ]);
}

/// Asks the studio to show a screen: Edit when the review is done, Record before any take.
pub struct GoTo(pub Screen);

/// Which items the middle steps through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The review items that wait for an answer.
    Open,
    /// The cuts the rules applied on their own, once nothing waits.
    Applied,
}

/// An item answered on this screen, shown dimmed in the list.
struct Answered {
    review: Review,
    take: Id,
    decision: Decision,
    /// An applied cut: Keep restored it, Cut left it.
    applied: bool,
}

/// One row of the list: an open item from the cut, or one answered here.
enum Row {
    Open(usize),
    /// An answer, and the block it sits in when the cut still has it.
    Answered(usize, Option<TakeAt>),
}

pub struct CutScreen {
    folder: Entity<VideoFolder>,
    mode: Mode,
    /// The open item on show, by its place in the cut.
    current: Option<(TakeAt, TimeRange)>,
    /// Where the item on show sat in the queue, so the next one takes its place when it goes.
    index: usize,
    answered: Vec<Answered>,
    speech: HashMap<Id, Option<Speech>>,
    peaks: HashMap<Id, Option<Arc<Peaks>>>,
    player: Player,
    /// Play the next item shown this way: set by answers and moves, not by opening the screen.
    autoplay: Option<Hear>,
    /// What the last rule switch or pacing change did to the cut.
    note: Option<String>,
    error: Option<String>,
    focus: FocusHandle,
    list: ScrollHandle,
    /// The item the list last scrolled to.
    scrolled: Option<ItemKey>,
    transcribe: Entity<Transcribe>,
    /// Pauses playback when another screen takes the focus.
    _blur: Option<Subscription>,
    _transcribed: Subscription,
}

impl EventEmitter<GoTo> for CutScreen {}

impl Focusable for CutScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CutScreen {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        cx.observe(&folder, |this, _, cx| {
            this.bring_back_undone(cx);
            // The clip of the item on show may have changed with the cut.
            this.player.unload();
            cx.notify();
        })
        .detach();
        let transcribe = cx.new(|cx| Transcribe::new(folder.clone(), cx));
        let transcribed = cx.subscribe(&transcribe, |this, _, Transcribed(takes), cx| {
            // The rules skipped these takes for want of words; now they make their rough cut.
            let takes = takes.clone();
            this.change("Transcribed".to_owned(), cx, |folder| {
                takes
                    .iter()
                    .try_for_each(|take| run::rough_cut(folder, take))
            });
        });
        Self {
            folder,
            mode: Mode::Open,
            current: None,
            index: 0,
            answered: Vec::new(),
            speech: HashMap::new(),
            peaks: HashMap::new(),
            player: Player::default(),
            autoplay: None,
            note: None,
            error: None,
            focus: cx.focus_handle(),
            list: ScrollHandle::new(),
            scrolled: None,
            transcribe,
            _blur: None,
            _transcribed: transcribed,
        }
    }

    /// The items the middle steps through: the open review items, or the applied cuts not
    /// answered here yet.
    fn queue(&self, cx: &App) -> Vec<(TakeAt, Review)> {
        let cut = &self.folder.read(cx).shown.cut;
        match self.mode {
            Mode::Open => cut.review_queue().collect(),
            Mode::Applied => applied(cut)
                .filter(|(at, review)| {
                    let take = &cut.chapters[at.chapter].takes[at.take].take;
                    !self
                        .answered
                        .iter()
                        .any(|a| a.applied && a.take == *take && a.review.range == review.range)
                })
                .collect(),
        }
    }

    /// The answers of `mode` given here that still belong in the list (see [`stands`]), by their
    /// place in `answered`.
    fn answers<'a>(
        &'a self,
        mode: Mode,
        cut: &'a Cut,
    ) -> impl Iterator<Item = (usize, &'a Answered)> + 'a {
        let applied_mode = mode == Mode::Applied;
        self.answered
            .iter()
            .enumerate()
            .filter(move |(_, a)| a.applied == applied_mode && stands(a, cut))
    }

    /// Back to the open items as soon as one waits, as after a rule switch.
    fn follow_open(&mut self, cx: &App) {
        if self.mode == Mode::Applied
            && self
                .folder
                .read(cx)
                .shown
                .cut
                .review_queue()
                .next()
                .is_some()
        {
            self.mode = Mode::Open;
            self.current = None;
            self.index = 0;
        }
    }

    /// Steps through the cuts the rules applied, from the one at `index`.
    fn review_applied(&mut self, index: usize, cx: &mut Context<Self>) {
        let cut = &self.folder.read(cx).shown.cut;
        if self.mode == Mode::Applied
            || cut.review_queue().next().is_some()
            || applied(cut).next().is_none()
        {
            return;
        }
        self.mode = Mode::Applied;
        self.answered.retain(|a| !a.applied);
        let queue = self.queue(cx);
        self.current = None;
        self.show(&queue, index, true);
        cx.notify();
    }

    /// The open item on show: the one picked, else the one that took its place.
    fn shown(&self, queue: &[(TakeAt, Review)]) -> Option<usize> {
        let last = queue.len().checked_sub(1)?;
        self.current
            .and_then(|(at, range)| queue.iter().position(|(a, r)| *a == at && r.range == range))
            .or(Some(self.index.min(last)))
    }

    fn show(&mut self, queue: &[(TakeAt, Review)], index: usize, play: bool) {
        if let Some((at, review)) = queue.get(index) {
            self.current = Some((*at, review.range));
            self.index = index;
            self.autoplay = play.then_some(Hear::Around);
        }
    }

    /// Drops answers that ⌘Z took back from the dimmed list and shows the newest of their items.
    /// An applied cut left cut wrote nothing, so there is nothing to take back.
    fn bring_back_undone(&mut self, cx: &mut Context<Self>) {
        self.follow_open(cx);
        let cut = &self.folder.read(cx).shown.cut;
        let open: Vec<(TakeAt, Review)> = cut.review_queue().collect();
        let removed: Vec<(TakeAt, Review)> = applied(cut).collect();
        let mut back = None;
        self.answered.retain(|answered| {
            if answered.applied && answered.decision == Decision::Cut {
                return true;
            }
            let queue = if answered.applied { &removed } else { &open };
            let open = queue.iter().find(|(at, review)| {
                review.range == answered.review.range
                    && review.by == answered.review.by
                    && cut.chapters[at.chapter].takes[at.take].take == answered.take
            });
            if let Some((at, review)) = open {
                back = Some((*at, review.range));
            }
            open.is_none()
        });
        if let Some(back) = back {
            self.current = Some(back);
        }
    }

    fn decide(&mut self, decision: Decision, cx: &mut Context<Self>) {
        let queue = self.queue(cx);
        let Some(index) = self.shown(&queue) else {
            return;
        };
        let (at, review) = queue[index];
        let applied = self.mode == Mode::Applied;
        let result = self.folder.update(cx, |folder, cx| {
            let take = folder.shown.cut.chapters[at.chapter].takes[at.take]
                .take
                .clone();
            match (applied, decision) {
                (false, _) => folder.decide(at, review.range, decision)?,
                (true, Decision::Keep) => folder.restore(at, review.range)?,
                // It is cut already.
                (true, Decision::Cut) => {}
            }
            cx.notify();
            Ok::<_, String>(take)
        });
        match result {
            Ok(take) => {
                self.error = None;
                self.answered.push(Answered {
                    review,
                    take,
                    decision,
                    applied,
                });
                // The next open item takes the place of the answered one, and plays.
                let queue = self.queue(cx);
                self.current = None;
                self.index = index;
                if let Some(next) = self.shown(&queue) {
                    self.show(&queue, next, true);
                }
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let queue = self.queue(cx);
        let Some(index) = self.shown(&queue) else {
            return;
        };
        let next = index.saturating_add_signed(by).min(queue.len() - 1);
        if next != index {
            self.show(&queue, next, true);
            cx.notify();
        }
    }

    /// Plays the item on show as `hear`, or pauses it when it plays that way.
    fn play(&mut self, hear: Hear, cx: &mut Context<Self>) {
        let queue = self.queue(cx);
        let Some(index) = self.shown(&queue) else {
            return;
        };
        if self.player.is_playing() && self.player.loaded().is_some_and(|(_, h)| h == hear) {
            self.player.pause();
        } else {
            let (at, review) = queue[index];
            match self
                .player
                .load(self.folder.read(cx), at, review.range, hear)
            {
                Ok(()) => self.player.play(),
                Err(error) => self.error = Some(error),
            }
        }
        cx.notify();
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

    /// Runs a rule switch or pacing change and notes what it did to the length of the cut.
    fn change(
        &mut self,
        what: String,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut VideoFolder) -> Result<(), String>,
    ) {
        let before = kept_length(&self.folder.read(cx).shown.cut);
        let result = self.folder.update(cx, |folder, cx| {
            let result = edit(folder);
            cx.notify();
            result
        });
        let after = kept_length(&self.folder.read(cx).shown.cut);
        self.note = result.is_ok().then(|| match after.cmp(&before) {
            Ordering::Less => format!(
                "{what}: the cut is {} shorter.",
                length(Micros(before.0 - after.0))
            ),
            Ordering::Greater => format!(
                "{what}: the cut is {} longer.",
                length(Micros(after.0 - before.0))
            ),
            Ordering::Equal => format!("{what}: the length did not change."),
        });
        self.error = result.err();
        cx.notify();
    }

    /// Drops the note of the last rule switch or pacing change, once ⌘Z or ⇧⌘Z stepped past
    /// it or the screen was left.
    pub fn forget_note(&mut self, cx: &mut Context<Self>) {
        if self.note.take().is_some() {
            cx.notify();
        }
    }

    fn speech(&mut self, take: &Id, cx: &App) -> Option<&Speech> {
        let dir = &self.folder.read(cx).dir;
        self.speech
            .entry(take.clone())
            .or_insert_with(|| cache::read_speech(dir, take).ok().flatten())
            .as_ref()
    }

    /// Reads the peaks of `take`'s mic on a background thread, once.
    fn load_peaks(&mut self, take: &Id, cx: &mut Context<Self>) {
        if self.peaks.contains_key(take) {
            return;
        }
        self.peaks.insert(take.clone(), None);
        let folder = self.folder.read(cx);
        let path = folder.media(take, "mic.mp4");
        let length = model::blocks(&folder.project.cut)
            .filter(|(_, edit)| edit.take == *take)
            .filter_map(|(_, edit)| edit.ranges.last().map(|r| r.range().to()))
            .max()
            .unwrap_or_default();
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

    /// Loads the item on show into the player, and plays it when an answer or a move asked.
    fn sync_player(&mut self, queue: &[(TakeAt, Review)], cx: &mut Context<Self>) {
        let Some(index) = self.shown(queue) else {
            self.autoplay = None;
            self.player.unload();
            return;
        };
        let (at, review) = queue[index];
        let take = self.folder.read(cx).shown.cut.chapters[at.chapter].takes[at.take]
            .take
            .clone();
        self.load_peaks(&take, cx);
        let key = (take, review.range);
        let loaded = self.player.loaded().map(|(key, _)| key.clone());
        if loaded.as_ref() == Some(&key) && self.autoplay.is_none() {
            return;
        }
        let play = self.autoplay.take();
        let hear = play.unwrap_or(Hear::Around);
        match self
            .player
            .load(self.folder.read(cx), at, review.range, hear)
        {
            Ok(()) if play.is_some() => self.player.play(),
            Ok(()) => {}
            Err(error) => self.error = Some(error),
        }
    }
}

/// The rule's name in the rule list.
fn rule_name(rule: RuleKind) -> &'static str {
    match rule {
        RuleKind::Silence => "Silences",
        RuleKind::Retake => "Inline retakes",
        RuleKind::Misspoken => "Misspoken words",
        RuleKind::Pause => "Pauses mid-sentence",
        RuleKind::Filler => "Filler words",
        RuleKind::IdleScreen => "Idle screen",
    }
}

/// What an item of the rule is called. `speaks` says the item strikes words although its rule
/// cuts only gaps between them, so the name does not call speech a pause.
fn kind_name(rule: RuleKind, speaks: bool) -> &'static str {
    match rule {
        RuleKind::Silence if speaks => "Speech marked as silence",
        RuleKind::Pause if speaks => "Speech marked as a pause",
        RuleKind::Silence => "Silence",
        RuleKind::Retake => "Inline retake",
        RuleKind::Misspoken => "Misspoken word",
        RuleKind::Pause => "Long pause",
        RuleKind::Filler => "Filler",
        RuleKind::IdleScreen => "Idle screen",
    }
}

/// Why the rule asks, in a sentence. `murmur` says whether the mic hears speech in a silence,
/// `None` when the take has no speech analysis.
fn reason(rule: RuleKind, length: Micros, murmur: Option<bool>) -> String {
    let seconds = self::length(length);
    match rule {
        RuleKind::Silence => match murmur {
            Some(true) => format!(
                "{seconds} without words, but the mic hears something. It may be a breath, or a word the recognizer missed."
            ),
            Some(false) => format!(
                "{seconds} without words or voice. The rules were not sure enough to cut it on their own."
            ),
            None => format!(
                "{seconds} without words. It may hold a breath, or a word the recognizer missed."
            ),
        },
        RuleKind::Retake => "You said this more than once. The later version is usually the clean one, so the earlier one would go.".to_owned(),
        RuleKind::Misspoken => {
            "A correction follows these words. Cutting them keeps only what you meant to say.".to_owned()
        }
        RuleKind::Pause => format!(
            "A {seconds} pause in the middle of a sentence. It may be a dramatic beat, so it was not cut on its own."
        ),
        RuleKind::Filler => "Words like this often carry emphasis. Keep the same words three times and the rule stops asking.".to_owned(),
        RuleKind::IdleScreen => format!(
            "The screen does not change for {seconds} while you talk. A jump cut or b-roll could cover it."
        ),
    }
}

/// The words around an item as one paragraph: those before muted, the item struck through.
fn context_text(pre: &str, cut: &str, post: &str) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    if !pre.is_empty() {
        text.push_str(pre);
        highlights.push((
            0..text.len(),
            HighlightStyle {
                color: Some(MUTED.into()),
                ..HighlightStyle::default()
            },
        ));
        text.push(' ');
    }
    let start = text.len();
    text.push_str(cut);
    highlights.push((
        start..text.len(),
        HighlightStyle {
            color: Some(CUT_TEXT.into()),
            background_color: Some(CUT_TINT.into()),
            strikethrough: Some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(CUT.into()),
            }),
            ..HighlightStyle::default()
        },
    ));
    if !post.is_empty() {
        text.push(' ');
        text.push_str(post);
    }
    StyledText::new(text).with_highlights(highlights)
}

/// Whether an item of `rule` strikes words of the transcript although its rule cuts only the gaps
/// between words. The rules never place one like that, but a newer transcript or a cut written
/// by hand can leave the label on speech.
fn speaks(rule: RuleKind, words: &[&str]) -> bool {
    matches!(rule, RuleKind::Silence | RuleKind::Pause) && !words.is_empty()
}

/// Why an item that [`speaks`] is in the list, open or `applied`.
fn speech_reason(rule: RuleKind, length: Micros, applied: bool) -> String {
    let seconds = self::length(length);
    let what = if rule == RuleKind::Pause {
        format!("a {seconds} pause")
    } else {
        format!("{seconds} of silence")
    };
    if applied {
        format!(
            "Cut as {what}, but the transcript has words here. Play it, and restore it if the words belong."
        )
    } else {
        format!(
            "Marked as {what}, but the transcript has words here, so cutting it cuts speech. Play it before you decide."
        )
    }
}

/// Why the rules cut an applied cut on their own, in a sentence.
fn applied_reason(rule: RuleKind, length: Micros) -> String {
    let seconds = self::length(length);
    match rule {
        RuleKind::Silence => format!("{seconds} without words or voice, so the rules cut it."),
        RuleKind::Retake => {
            "You said this again later, so the rules cut the earlier version.".to_owned()
        }
        RuleKind::Misspoken => {
            "A correction follows these words, so the rules cut them.".to_owned()
        }
        RuleKind::Pause => format!(
            "A {seconds} pause in the middle of a sentence, long enough for the rules to cut it."
        ),
        RuleKind::Filler => "A filler sound. Keep it when it carries meaning.".to_owned(),
        RuleKind::IdleScreen => format!(
            "{seconds} without voice on a screen that does not change, so the rules cut it."
        ),
    }
}

/// Whether `answered` still belongs in the list. An applied cut left cut wrote nothing, so it
/// stands only while `cut` holds that exact cut: a pacing change that moved it offers the moved
/// one again instead of keeping a stale row next to it.
fn stands(answered: &Answered, cut: &Cut) -> bool {
    !(answered.applied && answered.decision == Decision::Cut)
        || applied(cut).any(|(at, review)| {
            review.range == answered.review.range
                && cut.chapters[at.chapter].takes[at.take].take == answered.take
        })
}

/// The cuts the rules applied, in cut order, as review items of their rule.
fn applied(cut: &Cut) -> impl Iterator<Item = (TakeAt, Review)> + '_ {
    model::blocks(cut).flat_map(|(at, block)| {
        block.ranges.iter().filter_map(move |piece| match *piece {
            RangeEdit::Remove {
                range,
                by: RemovedBy::Rule(by),
            } => Some((
                at,
                Review {
                    range,
                    by,
                    kept: false,
                },
            )),
            _ => None,
        })
    })
}

/// `M:SS` of a length.
fn minutes(t: Micros) -> String {
    let seconds = t.0 / 1_000_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A length as "2.1 s" under a minute, "1:48" from there.
fn length(t: Micros) -> String {
    if t.0 < 60_000_000 {
        model::seconds(t)
    } else {
        minutes(t)
    }
}

/// The words whose middle lies in `range`, and the index of the first word after it.
fn words_in(words: &[Word], range: TimeRange) -> (std::ops::Range<usize>, Vec<&str>) {
    let middle = |w: &Word| Micros((w.range.from().0 + w.range.to().0) / 2);
    let before = words.partition_point(|w| middle(w) < range.from());
    let after = words.partition_point(|w| middle(w) < range.to());
    let inside = words[before..after]
        .iter()
        .map(|w| w.text.as_str())
        .collect();
    (before..after, inside)
}

/// The edited time where `t` of the block at `at` plays, or where the material after it starts.
fn edited_time(cut: &Cut, at: TakeAt, t: Micros) -> Micros {
    let mut before = 0;
    for (block_at, block) in model::blocks(cut) {
        for piece in &block.ranges {
            if let RangeEdit::Keep(kept) = piece {
                if block_at == at && kept.to() > t {
                    return Micros(before + t.0.saturating_sub(kept.from().0));
                }
                before += kept.duration().0;
            }
        }
        if block_at == at {
            break;
        }
    }
    Micros(before)
}

fn kept_length(cut: &Cut) -> Micros {
    Micros(
        model::blocks(cut)
            .flat_map(|(_, b)| &b.ranges)
            .filter_map(|p| match p {
                RangeEdit::Keep(r) => Some(r.duration().0),
                RangeEdit::Remove { .. } => None,
            })
            .sum(),
    )
}

fn raw_length(cut: &Cut) -> Micros {
    Micros(
        model::blocks(cut)
            .flat_map(|(_, b)| &b.ranges)
            .map(|p| p.range().duration().0)
            .sum(),
    )
}

fn timecode(t: Micros) -> String {
    model::timecode(t.0 * u64::from(FPS) / 1_000_000)
}

/// What a rule did, for its row: "12 cuts, 2:41 · 3 to review", or "Off", or "On" before
/// anything is recorded. Each piece says whether it is a duration, which shows in Geist Mono.
fn rule_detail(count: &RuleCount, on: bool, recorded: bool) -> Vec<(String, bool)> {
    let mut parts = Vec::new();
    if count.removed > 0 {
        let cuts = if count.removed == 1 { "cut" } else { "cuts" };
        parts.push((format!("{} {cuts},", count.removed), false));
        // The unit is set in Geist, as a space in Geist Mono reads twice as wide.
        let time = length(count.removed_time);
        match time.strip_suffix(" s") {
            Some(number) => parts.extend([(number.to_owned(), true), ("s".to_owned(), false)]),
            None => parts.push((time, true)),
        }
    }
    if count.queued > 0 {
        if !parts.is_empty() {
            parts.push(("·".to_owned(), false));
        }
        parts.push((format!("{} to review", count.queued), false));
    }
    match (on, parts.is_empty()) {
        (false, true) => vec![("Off".to_owned(), false)],
        (true, true) if !recorded => vec![("On".to_owned(), false)],
        (true, true) => vec![("Nothing found".to_owned(), false)],
        _ => parts,
    }
}

/// A small on/off switch, lime when on.
fn switch(on: bool) -> Div {
    div()
        .flex_none()
        .w(px(36.))
        .h(px(20.))
        .p(px(2.))
        .rounded_full()
        .bg(if on { ACCENT } else { FLOAT })
        .flex()
        .when(on, |this| this.justify_end())
        .child(
            div()
                .size(px(16.))
                .rounded_full()
                .bg(if on { ON_ACCENT } else { MUTED }),
        )
}

/// A 42 px button with an optional icon and its key hint, lime when `accent`.
fn button(
    id: &'static str,
    icon_name: Option<&'static str>,
    text: impl Into<SharedString>,
    keys: &'static str,
    accent: bool,
) -> gpui::Stateful<Div> {
    let color: Hsla = if accent {
        ON_ACCENT.into()
    } else {
        TEXT.into()
    };
    div()
        .id(id)
        .flex_none()
        .h(px(42.))
        .px(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(9.))
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .text_color(color)
        .font_weight(if accent {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .when(accent, |this| this.bg(ACCENT).hover(|s| s.opacity(0.9)))
        .when(!accent, |this| this.bg(RAISED).hover(|s| s.bg(FLOAT)))
        .when_some(icon_name, |this, name| this.child(icon(name, ICON, color)))
        .child(text.into())
        .child(if accent {
            kbd(keys)
                .bg(Hsla::from(ON_ACCENT).opacity(0.14))
                .text_color(ON_ACCENT)
        } else {
            kbd(keys)
        })
}

impl CutScreen {
    fn rules_pane(&self, counts: &[RuleCount], cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let folder = self.folder.read(cx);
        // A migrated video's cuts come from the style rules of its channel's format, which the
        // rule editor (milestone 10 item 7) will change; until then the rules only show here.
        let editable = !folder.is_migrated();
        let format = &folder.project.format;
        let pacing = format.pacing;
        let format_name = folder.project.video.format;
        let recorded = !folder.project.cut.chapters.is_empty();
        let rows = counts.iter().map(|count| {
            let rule = count.rule;
            let on = run::rule_on(format, rule);
            div()
                .id(SharedString::from(format!("rule-{rule}")))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(12.))
                .py(px(10.))
                .rounded(RADIUS_MD)
                .bg(CARD)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().font_weight(FontWeight::MEDIUM).child(rule_name(rule)))
                        .child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .text_size(TEXT_SM)
                                .text_color(if editable && !on { FAINT } else { MUTED })
                                .children(
                                    rule_detail(count, on || !editable, recorded)
                                        .into_iter()
                                        .map(|(text, duration)| {
                                            if duration {
                                                mono(text)
                                            } else {
                                                div().child(text)
                                            }
                                        }),
                                ),
                        ),
                )
                .when(editable, |this| {
                    this.cursor_pointer()
                        .hover(|s| s.bg(RAISED))
                        .child(switch(on))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let what =
                                format!("{} {}", rule_name(rule), if on { "off" } else { "on" });
                            this.change(what, cx, |folder| run::set_rule(folder, rule, !on));
                        }))
                })
        });
        let pacing_note = match pacing {
            Pacing::Loose => "Loose keeps 0.5 s of breath around each cut.",
            Pacing::Natural => "Natural keeps 0.25 s of breath around each cut.",
            Pacing::Tight => "Tight is the jump-cut style, 0.05 s around each cut.",
        };
        let segment = |name: &'static str, value: Pacing, cx: &mut Context<Self>| {
            Segment::new(
                name,
                pacing == value,
                cx.listener(move |this, _, _, cx| {
                    if pacing != value {
                        this.change(format!("{name} pacing"), cx, |folder| {
                            run::set_pacing(folder, value)
                        });
                    }
                }),
            )
        };
        let note = |text: String| {
            div()
                .text_size(TEXT_SM)
                .line_height(px(19.))
                .text_color(MUTED)
                .child(text)
        };
        pane()
            .id("rules")
            .w(px(316.))
            .flex_none()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .px(px(4.))
                    .pt(px(2.))
                    .pb(px(10.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(17.))
                            .child("Auto cut"),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(20.))
                            .text_color(MUTED)
                            .child("Applied when confident. The rest waits for you."),
                    ),
            )
            .children(rows)
            .map(|this| {
                if editable {
                    this.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .px(px(4.))
                            .pt(px(14.))
                            .child(label("Pacing"))
                            .child(segmented(
                                "pacing",
                                WELL,
                                vec![
                                    segment("Loose", Pacing::Loose, cx),
                                    segment("Natural", Pacing::Natural, cx),
                                    segment("Tight", Pacing::Tight, cx),
                                ],
                            ))
                            .child(note(pacing_note.to_owned())),
                    )
                } else {
                    let format = crate::board::format_name(format_name);
                    this.child(
                        div()
                            .mt(px(8.))
                            .p(px(12.))
                            .rounded(RADIUS_MD)
                            .bg(WELL)
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Set by the channel"),
                            )
                            .child(note(format!(
                                "This video follows the style rules of its channel's format, so the rules and the pacing change in the {format} format, for every video at once. Your answers here are saved for this video."
                            ))),
                    )
                }
            })
            .when_some(self.note.clone(), |this, text| {
                this.child(div().px(px(4.)).pt(px(6.)).child(note(text).text_color(BODY)))
            })
            .child(div().pt(px(6.)).child(self.transcribe.clone()))
    }

    fn progress(&self, queue: &[(TakeAt, Review)], cx: &App) -> Div {
        let cut = &self.folder.read(cx).shown.cut;
        let answered = self.answers(self.mode, cut).count();
        let total = answered + queue.len();
        let stat = |value: String, name: &'static str, color: Hsla| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    mono(value)
                        .text_size(px(26.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color),
                )
                .child(label(name))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(28.))
                    .child(stat(minutes(raw_length(cut)), "Raw", TEXT.into()))
                    .child(stat(minutes(kept_length(cut)), "Cut", ACCENT.into()))
                    .child(stat(
                        queue.len().to_string(),
                        match self.mode {
                            Mode::Open => "Need your call",
                            Mode::Applied => "Applied, to review",
                        },
                        TEXT.into(),
                    ))
                    .when(total > 0, |this| {
                        this.child(
                            div()
                                .ml_auto()
                                .text_size(px(14.))
                                .text_color(MUTED)
                                .child(format!("{answered} of {total} answered")),
                        )
                    }),
            )
            .when(total > 0, |this| {
                this.child(
                    div().h(px(4.)).rounded_full().bg(WELL).child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(ACCENT)
                            .w(relative(answered as f32 / total as f32)),
                    ),
                )
            })
    }

    fn item(&mut self, queue: &[(TakeAt, Review)], index: usize, cx: &mut Context<Self>) -> Div {
        let (at, review) = queue[index];
        let folder = self.folder.read(cx);
        let cut = &folder.shown.cut;
        let block = &cut.chapters[at.chapter].takes[at.take];
        let take = block.take.clone();
        let chapter = cut.chapters[at.chapter].title.clone();
        let at_text = timecode(edited_time(cut, at, review.range.from()));
        let words = folder
            .transcripts
            .get(&take)
            .map(|t| t.words().to_vec())
            .unwrap_or_default();
        let (inside, inside_text) = words_in(&words, review.range);
        // Context is what plays around the item, so removed words stay out of it.
        let plays = |w: &&Word| {
            let middle = Micros((w.range.from().0 + w.range.to().0) / 2);
            TimeRange::new(middle, Micros(middle.0 + 1)).is_some_and(|t| block.keeps(t))
        };
        let mut pre: Vec<&str> = words[..inside.start]
            .iter()
            .rev()
            .filter(plays)
            .take(CONTEXT_WORDS)
            .map(|w| w.text.as_str())
            .collect();
        pre.reverse();
        let post: Vec<&str> = words[inside.end..]
            .iter()
            .filter(plays)
            .take(CONTEXT_WORDS)
            .map(|w| w.text.as_str())
            .collect();
        let duration = length(review.range.duration());
        let cut_text = match (inside_text.is_empty(), review.by) {
            (false, _) => inside_text.join(" "),
            (true, RuleKind::IdleScreen) => format!("[screen idle {duration}]"),
            (true, RuleKind::Silence) => format!("[silence {duration}]"),
            (true, _) => format!("[pause {duration}]"),
        };
        let context = context_text(&pre.join(" "), &cut_text, &post.join(" "));
        let murmur = (review.by == RuleKind::Silence)
            .then(|| {
                self.speech(&take, cx)
                    .map(|s| s.ranges.iter().any(|r| r.overlaps(review.range)))
            })
            .flatten();
        let applied = self.mode == Mode::Applied;
        let speaks = speaks(review.by, &inside_text);
        let why = if speaks {
            speech_reason(review.by, review.range.duration(), applied)
        } else if applied {
            applied_reason(review.by, review.range.duration())
        } else {
            reason(review.by, review.range.duration(), murmur)
        };

        let peaks = self.peaks.get(&take).cloned().flatten();
        let bars: Vec<f32> = self
            .player
            .columns(BARS)
            .into_iter()
            .map(|column| match (column, &peaks) {
                (Some((from, to)), Some(peaks)) => peaks.over(from, to).clamp(0.0, 1.0),
                _ => 0.0,
            })
            .collect();
        let (playhead, region) = self.player.marks().unwrap_or((0.0, 0.0..0.0));
        let playing = self.player.is_playing();
        let hearing = self.player.loaded().map(|(_, hear)| hear);
        let wave = div()
            .relative()
            .h(px(56.))
            .flex()
            .items_center()
            .gap(px(2.))
            .children(
                bars.into_iter()
                    .map(|h| div().flex_1().rounded(px(1.)).bg(WAVE).h(px(3. + h * 53.))),
            )
            .child(
                div()
                    .absolute()
                    .top(px(-6.))
                    .bottom(px(-6.))
                    .left(relative(region.start))
                    .w(relative(region.end - region.start))
                    .rounded(RADIUS_SM)
                    .bg(CUT_TINT)
                    .border_1()
                    .border_color(Hsla::from(CUT).opacity(0.45)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(-8.))
                    .bottom(px(-8.))
                    .left(relative(playhead))
                    .w(px(2.))
                    .rounded(px(1.))
                    .bg(ACCENT),
            );
        let frame = self.player.frame().map(|frame| frame.buffer.clone());
        let play_button = |hear: Hear, id: &'static str, name: &'static str, keys| {
            let on = playing && hearing == Some(hear);
            button(
                id,
                Some(if on { "pause" } else { "play" }),
                if on { "Pause" } else { name },
                keys,
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.play(hear, cx)))
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(12.))
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(kind_name(review.by, speaks)),
                            )
                            .child(mono(at_text.clone()).text_color(MUTED))
                            .child(div().min_w_0().truncate().text_color(MUTED).child(chapter)),
                    )
                    .child(
                        // Two lines tall, like the words below, so the video keeps its size.
                        div()
                            .h(px(46.))
                            .text_color(BODY)
                            .line_height(px(23.))
                            .line_clamp(2)
                            .text_ellipsis()
                            .child(why),
                    ),
            )
            .child(
                // Two lines tall whatever the words, so the video below keeps its size.
                div()
                    .flex_none()
                    .h(px(CONTEXT_LINE * 2. + 28.))
                    .px(px(20.))
                    .py(px(14.))
                    .rounded(RADIUS_LG)
                    .bg(RAISED)
                    .text_size(px(19.))
                    .line_height(px(CONTEXT_LINE))
                    .text_color(STRONG)
                    .line_clamp(2)
                    .text_ellipsis()
                    .child(context),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(120.))
                    .rounded(RADIUS_LG)
                    .overflow_hidden()
                    .bg(gpui::black())
                    .children(frame.map(|f| surface(f).size_full().object_fit(ObjectFit::Contain))),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(10.))
                    .rounded(RADIUS_LG)
                    .bg(WELL)
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(wave)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(mono(at_text))
                            .child(match hearing {
                                Some(Hear::Cut) => "As cut, with 2 s before and after",
                                _ => "As recorded, with 2 s before and after",
                            })
                            .child(mono(duration.clone())),
                    ),
            )
            .child(
                // Playback left, the decision right; in a narrow window the decision wraps
                // below as one group instead of slipping under the next pane.
                div()
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.))
                    .child(play_button(
                        Hear::Around,
                        "play-around",
                        "Play around it",
                        "Space",
                    ))
                    .child(play_button(Hear::Cut, "play-cut", "Play as cut", "⇧Space"))
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                button(
                                    "keep",
                                    None,
                                    if applied { "Restore" } else { "Keep" },
                                    "K",
                                    false,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.decide(Decision::Keep, cx)),
                                ),
                            )
                            .child(
                                button(
                                    "cut",
                                    None,
                                    if applied {
                                        "Leave cut".to_owned()
                                    } else {
                                        format!("Cut {duration}")
                                    },
                                    "C",
                                    true,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.decide(Decision::Cut, cx)),
                                ),
                            ),
                    ),
            )
    }

    /// The middle when nothing waits: what happened, and the way on.
    fn done(&self, cx: &mut Context<Self>) -> Div {
        let folder = self.folder.read(cx);
        let cut = &folder.shown.cut;
        let applied = self.mode == Mode::Applied;
        let answered: Vec<&Answered> = self.answers(self.mode, cut).map(|(_, a)| a).collect();
        let kept = answered
            .iter()
            .filter(|a| a.decision == Decision::Keep)
            .count();
        let cuts = answered.len() - kept;
        let counts = autocut::rule_counts(cut);
        let removed: usize = counts.iter().map(|c| c.removed).sum();
        let removed_time = Micros(counts.iter().map(|c| c.removed_time.0).sum());
        let untranscribed =
            model::blocks(cut).any(|(_, b)| !folder.transcripts.contains_key(&b.take));
        let runs = minutes(kept_length(cut));
        // A lime check only when the rules or the answers made a cut.
        let (icon_name, done, title, text) = if cut.chapters.is_empty() {
            (
                "record",
                false,
                "Nothing recorded yet",
                "Record a take, and the rules make a rough cut of it here.".to_owned(),
            )
        } else if applied {
            (
                "check",
                true,
                "Every applied cut reviewed",
                format!("You restored {kept} and left {cuts} cut. The cut runs {runs}."),
            )
        } else if !answered.is_empty() {
            (
                "check",
                true,
                "All answered",
                format!("You kept {kept} and cut {cuts}. The cut runs {runs}."),
            )
        } else if removed == 0 && untranscribed {
            (
                "text",
                false,
                "No transcript yet",
                "The rules work from the words. Transcribe the take, and they make a rough cut of it."
                    .to_owned(),
            )
        } else if removed == 0 {
            (
                "check",
                true,
                "Nothing needs your call",
                "The rules found nothing to cut.".to_owned(),
            )
        } else {
            (
                "check",
                true,
                "Nothing needs your call",
                format!(
                    "The rules cut {removed} places, {} in all, and were sure about each.",
                    length(removed_time)
                ),
            )
        };
        let offer_applied = !applied && removed > 0;
        let (go, go_icon) = if cut.chapters.is_empty() {
            ("Record a take", "record")
        } else {
            ("Continue to edit", "edit")
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .child(
                div()
                    .size(px(44.))
                    .mb(px(6.))
                    .rounded_full()
                    .bg(if done {
                        SELECTION.into()
                    } else {
                        Hsla::from(RAISED)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(icon_name, px(22.), if done { ACCENT } else { MUTED })),
            )
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(460.))
                    .text_center()
                    .line_height(px(22.))
                    .text_color(BODY)
                    .child(text),
            )
            .child(
                div()
                    .pt(px(14.))
                    .flex()
                    .gap(px(10.))
                    .when(offer_applied, |this| {
                        let cuts = if removed == 1 {
                            "Review the applied cut".to_owned()
                        } else {
                            format!("Review the {removed} applied cuts")
                        };
                        this.child(
                            button("review-applied", Some("cut"), cuts, "R", false)
                                .on_click(cx.listener(|this, _, _, cx| this.review_applied(0, cx))),
                        )
                    })
                    .child(
                        button("done-continue", Some(go_icon), go, "⌘↩", true)
                            .on_click(cx.listener(|this, _, _, cx| this.go_on(cx))),
                    ),
            )
    }

    /// The list on the right: what needs your call, or the applied cuts once nothing does.
    /// `None` when there is nothing to list.
    fn list_pane(&mut self, queue: &[(TakeAt, Review)], cx: &mut Context<Self>) -> Option<Div> {
        let folder = self.folder.read(cx);
        let cut = &folder.shown.cut;
        let applied_mode = self.mode == Mode::Applied;
        // With nothing open the list offers the applied cuts, and a click starts their review.
        let entering = !applied_mode && queue.is_empty();
        let offered: Vec<(TakeAt, Review)> = if entering {
            applied(cut).collect()
        } else {
            Vec::new()
        };
        let list = if entering { &offered[..] } else { queue };
        let shown = if entering { None } else { self.shown(queue) };
        let take_of = |at: TakeAt| &cut.chapters[at.chapter].takes[at.take].take;
        let mut rows: Vec<(Row, Micros)> = list
            .iter()
            .enumerate()
            .map(|(index, (at, review))| {
                (Row::Open(index), edited_time(cut, *at, review.range.from()))
            })
            .collect();
        let with_answered = !entering || offered.is_empty();
        for (i, answered) in self.answers(self.mode, cut) {
            if !with_answered {
                continue;
            }
            let at = model::blocks(cut)
                .find(|(_, b)| {
                    b.take == answered.take
                        && b.ranges
                            .first()
                            .is_some_and(|r| r.range().from() <= answered.review.range.from())
                        && b.ranges
                            .last()
                            .is_some_and(|r| r.range().to() >= answered.review.range.to())
                })
                .map(|(at, _)| at);
            let time = at.map_or(Micros(0), |at| {
                edited_time(cut, at, answered.review.range.from())
            });
            rows.push((Row::Answered(i, at), time));
        }
        rows.sort_by_key(|(_, t)| *t);
        // The words cut, or for a gap the words it follows, and whether a gap rule's item
        // strikes words. A gap that opens a chapter's part of the take says so, as the gap
        // before it can follow the same words.
        let detail = |at: Option<TakeAt>, take: &Id, review: &Review| {
            let Some(transcript) = folder.transcripts.get(take) else {
                return (format!("Take {take}, no transcript yet"), false);
            };
            let words = transcript.words();
            let (inside, text) = words_in(words, review.range);
            let before = &words[inside.start.saturating_sub(3)..inside.start];
            let opens = at.filter(|at| at.take == 0).and_then(|at| {
                let start = cut.chapters[at.chapter].takes[at.take]
                    .ranges
                    .first()?
                    .range()
                    .from();
                let spoken = before.last().is_some_and(|w| w.range.from() >= start);
                (!spoken).then(|| cut.chapters[at.chapter].title.as_str())
            });
            let text = match (text.is_empty(), before.is_empty(), opens) {
                (false, ..) => {
                    return (format!("\"{}\"", text.join(" ")), speaks(review.by, &text));
                }
                (true, false, Some(chapter)) => format!("Start of {chapter}"),
                (true, true, _) => "At the start of the take".to_owned(),
                (true, false, None) => {
                    let before: Vec<&str> = before.iter().map(|w| w.text.as_str()).collect();
                    format!("After \"{}\"", before.join(" "))
                }
            };
            (text, false)
        };
        let mut on_row = None;
        let items: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(n, (row, time))| {
                let (take, review, status, open, at) = match row {
                    Row::Open(index) => {
                        let (at, review) = list[index];
                        if Some(index) == shown {
                            on_row = Some((n, (take_of(at).clone(), review.range)));
                        }
                        (
                            take_of(at),
                            review,
                            length(review.range.duration()),
                            Some(index),
                            Some(at),
                        )
                    }
                    Row::Answered(i, at) => {
                        let answered = &self.answered[i];
                        let status = match (answered.applied, answered.decision) {
                            (false, Decision::Keep) => "Kept",
                            (false, Decision::Cut) => "Cut",
                            (true, Decision::Keep) => "Restored",
                            (true, Decision::Cut) => "Left cut",
                        };
                        (&answered.take, answered.review, status.to_owned(), None, at)
                    }
                };
                let on = open.is_some() && open == shown;
                let (detail, speaks) = detail(at, take, &review);
                // An answered row dims, but its status stays readable.
                let dim = |el: Div| el.when(open.is_none(), |this| this.opacity(0.5));
                div()
                    .id(("item", n))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(RADIUS_MD)
                    .when(on, |this| this.bg(FLOAT))
                    .when(open.is_some() && !on, |this| {
                        this.cursor_pointer().hover(|s| s.bg(RAISED))
                    })
                    .child(dim(div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(kind_name(review.by, speaks)),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .child(detail),
                        )))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(2.))
                            .child(dim(mono(timecode(time)).text_size(px(14.))))
                            .child(
                                if open.is_some() {
                                    mono(status)
                                } else {
                                    div().child(status)
                                }
                                .text_size(TEXT_SM)
                                .text_color(if open.is_some() { BODY } else { MUTED }),
                            ),
                    )
                    .when_some(open, |this, index| {
                        let queue = queue.to_vec();
                        this.on_click(cx.listener(move |this, _, _, cx| {
                            if entering {
                                this.review_applied(index, cx);
                            } else {
                                this.show(&queue, index, true);
                                cx.notify();
                            }
                        }))
                    })
            })
            .collect();
        // Keep the item on show in view as answers and moves change it.
        if let Some((n, key)) = on_row
            && self.scrolled.as_ref() != Some(&key)
        {
            self.list.scroll_to_item(n);
            self.scrolled = Some(key);
        }
        if items.is_empty() {
            return None;
        }
        let open = queue.len();
        let title = if applied_mode || entering {
            "Applied by the rules"
        } else {
            "Needs your call"
        };
        Some(
            pane()
                .w(px(330.))
                .flex_none()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(12.))
                        .pt(px(4.))
                        .pb(px(8.))
                        .child(label(title))
                        .when(open > 1, |this| {
                            this.child(div().flex().gap(px(4.)).child(kbd("↑")).child(kbd("↓")))
                        }),
                )
                .child(
                    div()
                        .id("items")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.list)
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(items),
                )
                // With nothing open the middle offers it.
                .when(open > 0, |this| {
                    this.child(
                        div().pt(px(12.)).px(px(4.)).flex().flex_col().child(
                            button("continue", Some("edit"), "Continue to edit", "⌘↩", false)
                                .on_click(cx.listener(|this, _, _, cx| this.go_on(cx))),
                        ),
                    )
                }),
        )
    }
}

impl Render for CutScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._blur.is_none() {
            self._blur = Some(cx.on_focus_out(&self.focus, window, |this, _, _, cx| {
                this.player.pause();
                cx.notify();
            }));
        }
        self.follow_open(cx);
        let queue = self.queue(cx);
        self.sync_player(&queue, cx);
        if self.player.advance() || self.player.is_playing() {
            window.request_animation_frame();
        }
        if let Some(error) = self.player.error() {
            self.error.get_or_insert(error);
        }
        let counts = autocut::rule_counts(&self.folder.read(cx).shown.cut);
        let rules = self.rules_pane(&counts, cx);
        let progress = self.progress(&queue, cx);
        let body = match self.shown(&queue) {
            Some(index) => self.item(&queue, index, cx),
            None => self.done(cx),
        };
        let list = self.list_pane(&queue, cx);
        div()
            .id("cut")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(8.))
            .pb(px(8.))
            .on_action(cx.listener(|this, _: &KeepItem, _, cx| this.decide(Decision::Keep, cx)))
            .on_action(cx.listener(|this, _: &CutItem, _, cx| this.decide(Decision::Cut, cx)))
            .on_action(cx.listener(|this, _: &NextItem, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousItem, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &PlayAround, _, cx| this.play(Hear::Around, cx)))
            .on_action(cx.listener(|this, _: &PlayCut, _, cx| this.play(Hear::Cut, cx)))
            .on_action(cx.listener(|this, _: &Continue, _, cx| this.go_on(cx)))
            .on_action(cx.listener(|this, _: &ReviewApplied, _, cx| this.review_applied(0, cx)))
            .children(self.error.clone().map(crate::ui::alert))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(8.))
                    .child(rules)
                    .child(
                        pane()
                            .flex_1()
                            .min_w(px(480.))
                            .px(px(22.))
                            .py(px(20.))
                            .flex()
                            .flex_col()
                            .gap(px(18.))
                            .child(progress)
                            .child(body),
                    )
                    .children(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::RemovedBy;

    #[test]
    fn an_item_shows_at_its_edited_time() {
        let range = |from: u64, to: u64| TimeRange::new(Micros(from), Micros(to)).unwrap();
        let take = Id::new("t01").unwrap();
        let cut = Cut::default()
            .append_take(take.clone(), "A", range(0, 10_000_000))
            .append_take(take, "B", range(10_000_000, 20_000_000));
        let mut cut = cut;
        cut.chapters[1].takes[0].ranges = vec![
            RangeEdit::Remove {
                range: range(10_000_000, 12_000_000),
                by: RemovedBy::Manual,
            },
            RangeEdit::Keep(range(12_000_000, 20_000_000)),
        ];
        let at = TakeAt {
            chapter: 1,
            take: 0,
        };
        assert_eq!(
            edited_time(&cut, at, Micros(15_000_000)),
            Micros(13_000_000)
        );
        assert_eq!(kept_length(&cut), Micros(18_000_000));
        assert_eq!(raw_length(&cut), Micros(20_000_000));
    }

    #[test]
    fn a_cut_left_cut_stands_only_while_the_cut_holds_it() {
        let range = |from: u64, to: u64| TimeRange::new(Micros(from), Micros(to)).unwrap();
        let take = Id::new("t01").unwrap();
        let mut cut = Cut::default().append_take(take.clone(), "A", range(0, 10_000_000));
        let silence = |from| RangeEdit::Remove {
            range: range(from, 6_000_000),
            by: RemovedBy::Rule(RuleKind::Silence),
        };
        let left_cut = Answered {
            review: Review {
                range: range(4_000_000, 6_000_000),
                by: RuleKind::Silence,
                kept: false,
            },
            take,
            decision: Decision::Cut,
            applied: true,
        };
        cut.chapters[0].takes[0].ranges = vec![
            RangeEdit::Keep(range(0, 4_000_000)),
            silence(4_000_000),
            RangeEdit::Keep(range(6_000_000, 10_000_000)),
        ];
        assert!(stands(&left_cut, &cut));
        // A looser pacing moved the cut's edge: the old answer no longer has a row.
        cut.chapters[0].takes[0].ranges = vec![
            RangeEdit::Keep(range(0, 4_500_000)),
            silence(4_500_000),
            RangeEdit::Keep(range(6_000_000, 10_000_000)),
        ];
        assert!(!stands(&left_cut, &cut));
    }

    #[test]
    fn lengths_and_rule_rows_read_plainly() {
        assert_eq!(length(Micros(800_000)), "0.8 s");
        assert_eq!(length(Micros(108_400_000)), "1:48");
        let count = |removed, queued| RuleCount {
            rule: RuleKind::Silence,
            removed,
            removed_time: Micros(161_000_000),
            queued,
        };
        let text = |pieces: Vec<(String, bool)>| {
            let durations: Vec<String> = pieces
                .iter()
                .filter(|(_, d)| *d)
                .map(|(t, _)| t.clone())
                .collect();
            let words: Vec<String> = pieces.into_iter().map(|(t, _)| t).collect();
            (words.join(" "), durations)
        };
        assert_eq!(
            text(rule_detail(&count(12, 3), true, true)),
            (
                "12 cuts, 2:41 · 3 to review".to_owned(),
                vec!["2:41".to_owned()]
            )
        );
        let short = RuleCount {
            removed_time: Micros(3_700_000),
            ..count(2, 0)
        };
        assert_eq!(
            text(rule_detail(&short, true, true)),
            ("2 cuts, 3.7 s".to_owned(), vec!["3.7".to_owned()])
        );
        assert_eq!(
            text(rule_detail(&count(0, 0), true, true)).0,
            "Nothing found"
        );
        assert_eq!(text(rule_detail(&count(0, 0), true, false)).0, "On");
        assert_eq!(text(rule_detail(&count(0, 0), false, true)).0, "Off");
    }
}
