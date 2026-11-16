//! The Cut screen, where the style rules' rough cut is reviewed right after recording. The
//! middle shows one review item at a time: what the rule found and why it asks, the words around
//! it, its picture and waveform. Space plays it with a little of what plays before and after,
//! ⇧Space plays it as it would sound cut. K keeps it, C cuts it, and the next item plays. What
//! the rules cut sits on the left, the queue on the right. Once nothing waits, the cuts the
//! rules applied on their own can be stepped through the same way: K restores the words, C
//! leaves them cut.
//!
//! The queue is the lock's `review` instances, read through [`Cut::review_queue`]; every answer
//! goes through [`VideoFolder::decide`], which writes an `override { keep }` or `{ cut }`. The
//! screen keeps only which item is on show and the answers given since it opened, so the list
//! can show them dimmed. An answer undone with ⌘Z brings its item back on show.

pub mod player;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle,
    Hsla, ObjectFit, ScrollHandle, SharedString, StrikethroughStyle, StyledText, Subscription,
    Window, actions, div, prelude::*, px, relative, surface,
};
use shot_analyze::{Speech, cache};
use shot_core::{
    Applied, Cut, Decision, ElementKind, Id, Lock, Micros, RangeEdit, RemovedBy, Review, TakeAt,
    Target, TimeRange, Word,
};

use crate::folder::VideoFolder;
use crate::model::{self, FPS};
use crate::peaks::Peaks;
use crate::studio::Screen;
use crate::takes::TakesView;
use crate::theme::*;
use crate::transcribe::Transcribe;
use crate::ui::{Segment, icon, kbd, label, mono, pane, segmented};
use player::{Hear, ItemKey, Player};

pub(crate) const CONTEXT: &str = "CutScreen";
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
        ReviewApplied,
        ShowReview,
        ShowTakes
    ]
);

/// Asks the studio to show a screen: Edit when the review is done, Record before any take.
pub struct GoTo(pub Screen);

/// The two halves of the Cut screen: the review of the rules' findings, and the takes side by
/// side to compose the cut from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Review,
    Takes,
}

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
    tab: Tab,
    takes: Entity<TakesView>,
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
    error: Option<String>,
    focus: FocusHandle,
    list: ScrollHandle,
    /// The item the list last scrolled to.
    scrolled: Option<ItemKey>,
    transcribe: Entity<Transcribe>,
    /// Pauses playback when another screen takes the focus.
    _blur: Option<Subscription>,
    _takes: Subscription,
}

impl EventEmitter<GoTo> for CutScreen {}

impl Focusable for CutScreen {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.tab {
            Tab::Review => self.focus.clone(),
            Tab::Takes => self.takes.focus_handle(cx),
        }
    }
}

impl CutScreen {
    pub fn new(
        folder: Entity<VideoFolder>,
        import: Entity<crate::takes::import::Import>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&folder, |this, _, cx| {
            this.bring_back_undone(cx);
            // The clip of the item on show may have changed with the cut.
            this.player.unload();
            cx.notify();
        })
        .detach();
        let transcribe = cx.new(|cx| Transcribe::new(folder.clone(), cx));
        let takes = cx.new(|cx| TakesView::new(folder.clone(), import, cx));
        let forward = cx.subscribe(&takes, |_, _, GoTo(screen), cx| cx.emit(GoTo(*screen)));
        // A video with several takes in the cut at the same beats opens on its takes.
        let tab = if takes.read(cx).needs_composing(cx) {
            Tab::Takes
        } else {
            Tab::Review
        };
        Self {
            folder,
            tab,
            takes,
            mode: Mode::Open,
            current: None,
            index: 0,
            answered: Vec::new(),
            speech: HashMap::new(),
            peaks: HashMap::new(),
            player: Player::default(),
            autoplay: None,
            error: None,
            focus: cx.focus_handle(),
            list: ScrollHandle::new(),
            scrolled: None,
            transcribe,
            _blur: None,
            _takes: forward,
        }
    }

    fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.player.pause();
        window.focus(&self.focus_handle(cx), cx);
        cx.notify();
    }

    /// After a new recording: shows the takes when the newest take repeats beats an earlier
    /// take has, so the cut does not quietly play both.
    pub fn recorded(&mut self, cx: &mut Context<Self>) {
        if self.takes.update(cx, |takes, cx| takes.recorded(cx)) {
            self.tab = Tab::Takes;
            cx.notify();
        }
    }

    /// The Review and Takes switch for the title bar.
    pub fn title_controls(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab;
        let this = cx.entity().downgrade();
        let switch = move |to: Tab| {
            let this = this.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                this.update(cx, |screen, cx| screen.set_tab(to, window, cx))
                    .ok();
            }
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(segmented(
                "cut-tab",
                SURFACE,
                vec![
                    Segment::new("Review", tab == Tab::Review, switch(Tab::Review)).icon("cut"),
                    Segment::new("Takes", tab == Tab::Takes, switch(Tab::Takes)).icon("takes"),
                ],
            ))
            .child(kbd(match tab {
                Tab::Review => "⌥2",
                Tab::Takes => "⌥1",
            }))
    }

    /// The items the middle steps through: the open review items, or the applied cuts not
    /// answered here yet.
    fn queue(&self, cx: &App) -> Vec<(TakeAt, Review)> {
        let folder = self.folder.read(cx);
        let (cut, lock) = (&folder.project.cut, folder.lock());
        match self.mode {
            Mode::Open => cut.review_queue(lock).collect(),
            Mode::Applied => applied(cut, lock)
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
        lock: &'a Lock,
    ) -> impl Iterator<Item = (usize, &'a Answered)> + 'a {
        let applied_mode = mode == Mode::Applied;
        self.answered
            .iter()
            .enumerate()
            .filter(move |(_, a)| a.applied == applied_mode && stands(a, cut, lock))
    }

    /// Back to the open items as soon as one waits, as after a rule switch.
    fn follow_open(&mut self, cx: &App) {
        let folder = self.folder.read(cx);
        if self.mode == Mode::Applied
            && folder
                .project
                .cut
                .review_queue(folder.lock())
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
        let folder = self.folder.read(cx);
        let (cut, lock) = (&folder.project.cut, folder.lock());
        if self.mode == Mode::Applied
            || cut.review_queue(lock).next().is_some()
            || applied(cut, lock).next().is_none()
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
        let folder = self.folder.read(cx);
        let (cut, lock) = (&folder.project.cut, folder.lock());
        let open: Vec<(TakeAt, Review)> = cut.review_queue(lock).collect();
        let removed: Vec<(TakeAt, Review)> = applied(cut, lock).collect();
        let mut back = None;
        self.answered.retain(|answered| {
            if answered.applied && answered.decision == Decision::Cut {
                return true;
            }
            let queue = if answered.applied { &removed } else { &open };
            let open = queue.iter().find(|(at, review)| {
                review.range == answered.review.range
                    && review.kind == answered.review.kind
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
            let take = folder.project.cut.chapters[at.chapter].takes[at.take]
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
        let take = self.folder.read(cx).project.cut.chapters[at.chapter].takes[at.take]
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

/// The name of what the rules cut of an element kind, in the rule list.
fn rule_name(kind: ElementKind) -> String {
    match kind {
        ElementKind::Silence => "Silences".to_owned(),
        ElementKind::Retake => "Inline retakes".to_owned(),
        ElementKind::Misspoken => "Misspoken words".to_owned(),
        ElementKind::Pause => "Pauses mid-sentence".to_owned(),
        ElementKind::Filler => "Filler words".to_owned(),
        ElementKind::Idle => "Idle screen".to_owned(),
        other => format!("Cuts on {other}"),
    }
}

/// What an item of the kind is called. `speaks` says the item strikes words although its rule
/// cuts only gaps between them, so the name does not call speech a pause.
fn kind_name(kind: ElementKind, speaks: bool) -> &'static str {
    match kind {
        ElementKind::Silence if speaks => "Speech marked as silence",
        ElementKind::Pause if speaks => "Speech marked as a pause",
        ElementKind::Silence => "Silence",
        ElementKind::Retake => "Inline retake",
        ElementKind::Misspoken => "Misspoken word",
        ElementKind::Pause => "Long pause",
        ElementKind::Filler => "Filler",
        ElementKind::Idle => "Idle screen",
        _ => "Cut by a rule",
    }
}

/// Why the rule asks, in a sentence. `murmur` says whether the mic hears speech in a silence,
/// `None` when the take has no speech analysis.
fn reason(kind: ElementKind, length: Micros, murmur: Option<bool>) -> String {
    let seconds = self::length(length);
    match kind {
        ElementKind::Silence => match murmur {
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
        ElementKind::Retake => "You said this more than once. The later version is usually the clean one, so the earlier one would go.".to_owned(),
        ElementKind::Misspoken => {
            "A correction follows these words. Cutting them keeps only what you meant to say.".to_owned()
        }
        ElementKind::Pause => format!(
            "A {seconds} pause in the middle of a sentence. It may be a dramatic beat, so it was not cut on its own."
        ),
        ElementKind::Filler => "Words like this often carry emphasis, so the rules were not sure enough to cut it.".to_owned(),
        ElementKind::Idle => format!(
            "The screen does not change for {seconds} while you talk. A jump cut or b-roll could cover it."
        ),
        _ => format!("{seconds} a rule was not sure enough to cut on its own."),
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
fn speaks(kind: ElementKind, words: &[&str]) -> bool {
    matches!(kind, ElementKind::Silence | ElementKind::Pause) && !words.is_empty()
}

/// Why an item that [`speaks`] is in the list, open or `applied`.
fn speech_reason(kind: ElementKind, length: Micros, applied: bool) -> String {
    let seconds = self::length(length);
    let what = if kind == ElementKind::Pause {
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
fn applied_reason(kind: ElementKind, length: Micros) -> String {
    let seconds = self::length(length);
    match kind {
        ElementKind::Silence => format!("{seconds} without words or voice, so the rules cut it."),
        ElementKind::Retake => {
            "You said this again later, so the rules cut the earlier version.".to_owned()
        }
        ElementKind::Misspoken => {
            "A correction follows these words, so the rules cut them.".to_owned()
        }
        ElementKind::Pause => format!(
            "A {seconds} pause in the middle of a sentence, long enough for the rules to cut it."
        ),
        ElementKind::Filler => "A filler sound. Keep it when it carries meaning.".to_owned(),
        ElementKind::Idle => format!(
            "{seconds} without voice on a screen that does not change, so the rules cut it."
        ),
        _ => format!("{seconds} a rule cut on its own."),
    }
}

/// Whether `answered` still belongs in the list. An applied cut left cut wrote nothing, so it
/// stands only while the lock holds that exact cut: a rule change that moved it offers the moved
/// one again instead of keeping a stale row next to it.
fn stands(answered: &Answered, cut: &Cut, lock: &Lock) -> bool {
    !(answered.applied && answered.decision == Decision::Cut)
        || applied(cut, lock).any(|(at, review)| {
            review.range == answered.review.range
                && cut.chapters[at.chapter].takes[at.take].take == answered.take
        })
}

/// The cuts the rules applied in the blocks' kept material, in cut order, as review items of
/// their element.
fn applied<'a>(cut: &'a Cut, lock: &'a Lock) -> impl Iterator<Item = (TakeAt, Review)> + 'a {
    model::blocks(cut).flat_map(move |(at, block)| {
        let instances = lock
            .take(&block.take)
            .into_iter()
            .flat_map(|t| &t.instances);
        instances.flat_map(move |i| {
            let pieces = match (&i.applied, i.element.target) {
                (&Applied::Cut(range), Target::Kind(kind)) => block
                    .ranges
                    .iter()
                    .filter_map(|r| match r {
                        RangeEdit::Keep(kept) => kept.intersect(range),
                        RangeEdit::Remove(_) => None,
                    })
                    .map(|range| Review {
                        range,
                        kind,
                        t: i.element.t,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            pieces.into_iter().map(move |review| (at, review))
        })
    })
}

/// What the lock and a person removed of one element kind, for its row in the rule list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KindCount {
    kind: ElementKind,
    removed: usize,
    removed_time: Micros,
    queued: usize,
}

/// The rules' cuts and open review items by element kind, in the order kinds come first.
fn counts(cut: &Cut, lock: &Lock) -> Vec<KindCount> {
    let mut out: Vec<KindCount> = Vec::new();
    let mut add = |kind: ElementKind, removed: Option<Micros>| {
        let index = match out.iter().position(|c| c.kind == kind) {
            Some(index) => index,
            None => {
                out.push(KindCount {
                    kind,
                    removed: 0,
                    removed_time: Micros(0),
                    queued: 0,
                });
                out.len() - 1
            }
        };
        let count = &mut out[index];
        match removed {
            Some(length) => {
                count.removed += 1;
                count.removed_time.0 += length.0;
            }
            None => count.queued += 1,
        }
    };
    for (_, edit) in model::blocks(cut) {
        for (range, by) in model::removals(edit, lock) {
            if let RemovedBy::Rule(Target::Kind(kind)) = by {
                add(kind, Some(range.duration()));
            }
        }
    }
    for (_, review) in cut.review_queue(lock) {
        add(review.kind, None);
    }
    out
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
fn edited_time(cut: &Cut, lock: &Lock, at: TakeAt, t: Micros) -> Micros {
    let mut before = 0;
    for (block_at, block) in model::blocks(cut) {
        for played in model::played(block, lock) {
            if block_at == at && played.to() > t {
                return Micros(before + t.0.saturating_sub(played.from().0));
            }
            before += played.duration().0;
        }
        if block_at == at {
            break;
        }
    }
    Micros(before)
}

fn kept_length(cut: &Cut, lock: &Lock) -> Micros {
    Micros(
        model::blocks(cut)
            .flat_map(|(_, b)| model::played(b, lock))
            .map(|r| r.duration().0)
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

/// "Take t03 was" or "Takes t03, t05 were", for the takes of `cut` recorded with the mic off.
fn without_mic(folder: &VideoFolder, cut: &Cut) -> Option<String> {
    let mut takes: Vec<&Id> = Vec::new();
    for (_, block) in model::blocks(cut) {
        if !takes.contains(&&block.take) && folder.without_mic(&block.take) {
            takes.push(&block.take);
        }
    }
    let names = takes
        .iter()
        .map(|t| t.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    match takes.len() {
        0 => None,
        1 => Some(format!("Take {names} was")),
        _ => Some(format!("Takes {names} were")),
    }
}

fn timecode(t: Micros) -> String {
    model::timecode(t.0 * u64::from(FPS) / 1_000_000)
}

/// What the rules did of a kind, for its row: "12 cuts, 2:41 · 3 to review". Each piece says
/// whether it is a duration, which shows in Geist Mono.
fn rule_detail(count: &KindCount) -> Vec<(String, bool)> {
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
    parts
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
    fn rules_pane(&self, counts: &[KindCount], cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let folder = self.folder.read(cx);
        let format = crate::board::format_name(folder.project.video.format);
        let recorded = !folder.project.cut.chapters.is_empty();
        let rows = counts.iter().map(|count| {
            div()
                .id(SharedString::from(format!("rule-{}", count.kind)))
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
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(rule_name(count.kind)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .children(rule_detail(count).into_iter().map(
                                    |(text, duration)| {
                                        if duration {
                                            mono(text)
                                        } else {
                                            div().child(text)
                                        }
                                    },
                                )),
                        ),
                )
        });
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
            .when(recorded && counts.is_empty(), |this| {
                this.child(div().px(px(4.)).child(note("Nothing cut yet.".to_owned())))
            })
            .child(
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
            .child(div().pt(px(6.)).child(self.transcribe.clone()))
    }

    fn progress(&self, queue: &[(TakeAt, Review)], cx: &App) -> Div {
        let folder = self.folder.read(cx);
        let (cut, lock) = (&folder.project.cut, folder.lock());
        let answered = self.answers(self.mode, cut, lock).count();
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
                    .child(stat(minutes(kept_length(cut, lock)), "Cut", ACCENT.into()))
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
        let (cut, lock) = (&folder.project.cut, folder.lock());
        let block = &cut.chapters[at.chapter].takes[at.take];
        let take = block.take.clone();
        let chapter = cut.chapters[at.chapter].title.clone();
        let at_text = timecode(edited_time(cut, lock, at, review.range.from()));
        let played = model::played(block, lock);
        let words = folder
            .transcripts
            .get(&take)
            .map(|t| t.words().to_vec())
            .unwrap_or_default();
        let (inside, inside_text) = words_in(&words, review.range);
        // Context is what plays around the item, so removed words stay out of it.
        let plays = |w: &&Word| {
            let middle = Micros((w.range.from().0 + w.range.to().0) / 2);
            played.iter().any(|p| p.from() <= middle && middle < p.to())
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
        let cut_text = match (inside_text.is_empty(), review.kind) {
            (false, _) => inside_text.join(" "),
            (true, ElementKind::Idle) => format!("[screen idle {duration}]"),
            (true, ElementKind::Silence) => format!("[silence {duration}]"),
            (true, _) => format!("[pause {duration}]"),
        };
        let context = context_text(&pre.join(" "), &cut_text, &post.join(" "));
        let murmur = (review.kind == ElementKind::Silence)
            .then(|| {
                self.speech(&take, cx)
                    .map(|s| s.ranges.iter().any(|r| r.overlaps(review.range)))
            })
            .flatten();
        let applied = self.mode == Mode::Applied;
        let speaks = speaks(review.kind, &inside_text);
        let why = if speaks {
            speech_reason(review.kind, review.range.duration(), applied)
        } else if applied {
            applied_reason(review.kind, review.range.duration())
        } else {
            reason(review.kind, review.range.duration(), murmur)
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
                                    .child(kind_name(review.kind, speaks)),
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
        let (cut, lock) = (&folder.project.cut, folder.lock());
        let applied = self.mode == Mode::Applied;
        let answered: Vec<&Answered> = self.answers(self.mode, cut, lock).map(|(_, a)| a).collect();
        let kept = answered
            .iter()
            .filter(|a| a.decision == Decision::Keep)
            .count();
        let cuts = answered.len() - kept;
        let counts = counts(cut, lock);
        let removed: usize = counts.iter().map(|c| c.removed).sum();
        let removed_time = Micros(counts.iter().map(|c| c.removed_time.0).sum());
        let untranscribed =
            model::blocks(cut).any(|(_, b)| !folder.transcripts.contains_key(&b.take));
        let runs = minutes(kept_length(cut, lock));
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
        } else if untranscribed {
            (
                "text",
                false,
                "No transcript yet",
                "The rules and the text editor work from the words. Transcribe the take on the \
                 left, and the rules make a rough cut."
                    .to_owned(),
            )
        } else if removed == 0
            && let Some(takes) = without_mic(folder, cut)
        {
            (
                "mic",
                false,
                "No words to cut by",
                format!(
                    "{takes} recorded without a mic, so the rules have nothing to cut. \
                     Trim by hand on the timeline."
                ),
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
                        // Without words, Transcribe on the left is the main action.
                        button("done-continue", Some(go_icon), go, "⌘↩", !untranscribed)
                            .on_click(cx.listener(|this, _, _, cx| this.go_on(cx))),
                    ),
            )
    }

    /// The list on the right: what needs your call, or the applied cuts once nothing does.
    /// `None` when there is nothing to list.
    fn list_pane(&mut self, queue: &[(TakeAt, Review)], cx: &mut Context<Self>) -> Option<Div> {
        let folder = self.folder.read(cx);
        let (cut, lock) = (&folder.project.cut, folder.lock());
        let applied_mode = self.mode == Mode::Applied;
        // With nothing open the list offers the applied cuts, and a click starts their review.
        let entering = !applied_mode && queue.is_empty();
        let offered: Vec<(TakeAt, Review)> = if entering {
            applied(cut, lock).collect()
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
                (
                    Row::Open(index),
                    edited_time(cut, lock, *at, review.range.from()),
                )
            })
            .collect();
        let with_answered = !entering || offered.is_empty();
        for (i, answered) in self.answers(self.mode, cut, lock) {
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
                edited_time(cut, lock, at, answered.review.range.from())
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
                    return (
                        format!("\"{}\"", text.join(" ")),
                        speaks(review.kind, &text),
                    );
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
                                .child(kind_name(review.kind, speaks)),
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
        if self.tab == Tab::Takes {
            return div()
                .size_full()
                .px(px(8.))
                .pb(px(8.))
                .on_action(cx.listener(|this, _: &ShowReview, window, cx| {
                    this.set_tab(Tab::Review, window, cx)
                }))
                .child(self.takes.clone())
                .into_any_element();
        }
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
        let folder = self.folder.read(cx);
        let counts = counts(&folder.project.cut, folder.lock());
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
            .on_action(
                cx.listener(|this, _: &ShowTakes, window, cx| this.set_tab(Tab::Takes, window, cx)),
            )
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
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    /// A lock of `t01` that cuts silences at `from`.
    fn silences(cuts: &[(u64, u64)]) -> Lock {
        let instances: String = cuts
            .iter()
            .map(|(from, to)| {
                format!("    cut from={from} to={to} on=silence t={from} layer=format rule=\"silence\"\n")
            })
            .collect();
        Lock::parse(&format!("lock 1\n\ntake t01 {{\n{instances}}}\n")).unwrap()
    }

    #[test]
    fn an_item_shows_at_its_edited_time() {
        let take = Id::new("t01").unwrap();
        let cut = Cut::default()
            .append_take(take.clone(), "A", range(0, 10_000_000))
            .append_take(take, "B", range(10_000_000, 20_000_000));
        let mut cut = cut;
        cut.chapters[1].takes[0].ranges = vec![
            RangeEdit::Remove(range(10_000_000, 12_000_000)),
            RangeEdit::Keep(range(12_000_000, 20_000_000)),
        ];
        let at = TakeAt {
            chapter: 1,
            take: 0,
        };
        let lock = silences(&[(1_000_000, 2_000_000)]);
        assert_eq!(
            edited_time(&cut, &lock, at, Micros(15_000_000)),
            Micros(12_000_000)
        );
        assert_eq!(kept_length(&cut, &lock), Micros(17_000_000));
        assert_eq!(raw_length(&cut), Micros(20_000_000));
    }

    #[test]
    fn a_cut_left_cut_stands_only_while_the_lock_holds_it() {
        let take = Id::new("t01").unwrap();
        let cut = Cut::default().append_take(take.clone(), "A", range(0, 10_000_000));
        let left_cut = Answered {
            review: Review {
                range: range(4_000_000, 6_000_000),
                kind: ElementKind::Silence,
                t: Micros(4_000_000),
            },
            take,
            decision: Decision::Cut,
            applied: true,
        };
        assert!(stands(
            &left_cut,
            &cut,
            &silences(&[(4_000_000, 6_000_000)])
        ));
        // A looser pacing moved the cut's edge: the old answer no longer has a row.
        assert!(!stands(
            &left_cut,
            &cut,
            &silences(&[(4_500_000, 6_000_000)])
        ));
    }

    #[test]
    fn lengths_and_rule_rows_read_plainly() {
        assert_eq!(length(Micros(800_000)), "0.8 s");
        assert_eq!(length(Micros(108_400_000)), "1:48");
        let count = |removed, queued| KindCount {
            kind: ElementKind::Silence,
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
            text(rule_detail(&count(12, 3))),
            (
                "12 cuts, 2:41 · 3 to review".to_owned(),
                vec!["2:41".to_owned()]
            )
        );
        let short = KindCount {
            removed_time: Micros(3_700_000),
            ..count(2, 0)
        };
        assert_eq!(
            text(rule_detail(&short)),
            ("2 cuts, 3.7 s".to_owned(), vec!["3.7".to_owned()])
        );
        let cut = Cut::default().append_take(Id::new("t01").unwrap(), "A", range(0, 10_000_000));
        let found = counts(
            &cut,
            &silences(&[(1_000_000, 2_000_000), (5_000_000, 5_500_000)]),
        );
        assert_eq!(
            found,
            [KindCount {
                kind: ElementKind::Silence,
                removed: 2,
                removed_time: Micros(1_500_000),
                queued: 0,
            }]
        );
    }
}
