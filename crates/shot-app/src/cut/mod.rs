//! The Cut screen: the auto-cut rules with their switches and counts, the pacing, and the review
//! queue with Keep (K) and Cut (C).
//!
//! The queue is the `review` nodes of `cut.kdl`, read through [`Cut::review_queue`]; every
//! answer is [`Cut::decide`] written back. The screen keeps only which item is on show and the
//! answers given since it opened, so the list can show them dimmed like the mockup.

pub mod run;

use std::collections::HashMap;

use gpui::{
    App, Context, Div, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla, KeyBinding,
    SharedString, Window, actions, div, prelude::*, px, relative,
};
use shot_analyze::autocut::{self, RuleCount};
use shot_analyze::{Speech, cache};
use shot_core::{
    Cut, Decision, Id, Micros, Pacing, RangeEdit, Review, RuleKind, TakeAt, TimeRange,
};

use crate::folder::VideoFolder;
use crate::model::{self, FPS};
use crate::theme::*;
use crate::ui::{Segment, kbd, label, mono, pane, segmented};

const CONTEXT: &str = "CutScreen";
/// Words of context shown on each side of a finding.
const CONTEXT_WORDS: usize = 8;
/// Bars of the waveform under a finding.
const BARS: usize = 90;
/// Media shown on each side of a finding in the waveform.
const WAVE_MARGIN: Micros = Micros(2_000_000);

actions!(cut, [KeepItem, CutItem, NextItem, PreviousItem]);

pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("k", KeepItem, context),
        KeyBinding::new("c", CutItem, context),
        KeyBinding::new("down", NextItem, context),
        KeyBinding::new("up", PreviousItem, context),
    ]);
}

/// Asks the studio to show the Edit screen.
pub struct ContinueToEdit;

/// A finding answered on this screen, shown dimmed in the list.
struct Answered {
    review: Review,
    take: Id,
    decision: Decision,
}

/// One row of the list: an open item from the cut, or one answered here.
enum Row {
    Open(TakeAt, Review),
    Answered(usize),
}

pub struct CutScreen {
    folder: Entity<VideoFolder>,
    /// The open item on show, by its place in the cut.
    current: Option<(TakeAt, TimeRange)>,
    answered: Vec<Answered>,
    speech: HashMap<Id, Option<Speech>>,
    error: Option<String>,
    focus: FocusHandle,
}

use gpui::Entity;

impl EventEmitter<ContinueToEdit> for CutScreen {}

impl Focusable for CutScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CutScreen {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        cx.observe(&folder, |_, _, cx| cx.notify()).detach();
        Self {
            folder,
            current: None,
            answered: Vec::new(),
            speech: HashMap::new(),
            error: None,
            focus: cx.focus_handle(),
        }
    }

    fn queue(&self, cx: &App) -> Vec<(TakeAt, Review)> {
        self.folder.read(cx).project.cut.review_queue().collect()
    }

    /// The open item on show: the one picked, else the first.
    fn shown(&self, queue: &[(TakeAt, Review)]) -> Option<usize> {
        self.current
            .and_then(|(at, range)| queue.iter().position(|(a, r)| *a == at && r.range == range))
            .or((!queue.is_empty()).then_some(0))
    }

    fn decide(&mut self, decision: Decision, cx: &mut Context<Self>) {
        let queue = self.queue(cx);
        let Some(index) = self.shown(&queue) else {
            return;
        };
        let (at, review) = queue[index];
        let result = self.folder.update(cx, |folder, cx| {
            let take = folder.project.cut.chapters[at.chapter].takes[at.take]
                .take
                .clone();
            let cut = folder
                .project
                .cut
                .decide(at, review.range, decision)
                .map_err(|e| e.to_string())?;
            folder.write_cut(cut)?;
            cx.notify();
            Ok::<_, String>(take)
        });
        match result {
            Ok(take) => {
                self.answered.push(Answered {
                    review,
                    take,
                    decision,
                });
                // The next open item takes the place of the answered one.
                let queue = self.queue(cx);
                self.current = queue
                    .get(index.min(queue.len().saturating_sub(1)))
                    .map(|(at, r)| (*at, r.range));
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
        let next = (index as isize + by).clamp(0, queue.len() as isize - 1) as usize;
        self.current = Some((queue[next].0, queue[next].1.range));
        cx.notify();
    }

    fn change(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut VideoFolder) -> Result<(), String>,
    ) {
        let result = self.folder.update(cx, |folder, cx| {
            let result = edit(folder);
            cx.notify();
            result
        });
        self.error = result.err();
        cx.notify();
    }

    fn speech(&mut self, take: &Id, dir: &std::path::Path) -> Option<&Speech> {
        self.speech
            .entry(take.clone())
            .or_insert_with(|| cache::read_speech(dir, take).ok().flatten())
            .as_ref()
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

/// What a finding of the rule is called, why the rule asks, and the reason in a sentence.
fn finding_text(rule: RuleKind, length: Micros) -> (&'static str, &'static str, String) {
    let seconds = model::seconds(length);
    match rule {
        RuleKind::Silence => (
            "Silence",
            "A sound without words",
            format!("A {seconds} gap between sentences holds a sound the transcript has no word for. It may be a breath, or a word the recognizer missed."),
        ),
        RuleKind::Retake => (
            "Inline retake",
            "Unsure which take is better",
            "You said this more than once. The later version is usually the clean one, so the earlier one would go.".to_owned(),
        ),
        RuleKind::Misspoken => (
            "Misspoken word",
            "You corrected yourself",
            "A correction follows these words. Cutting them keeps only what you meant to say.".to_owned(),
        ),
        RuleKind::Pause => (
            "Long pause",
            "Could be on purpose",
            format!("A {seconds} pause in the middle of a sentence. It may be a dramatic beat, so it was not cut automatically."),
        ),
        RuleKind::Filler => (
            "Filler",
            "Might be your voice",
            "Words like this often carry emphasis. Keep it a few more times and the rule stops asking.".to_owned(),
        ),
        RuleKind::IdleScreen => (
            "Idle screen",
            "Screen idle while you talk",
            format!("The screen does not change for {seconds} while you talk. A jump cut or b-roll could cover it."),
        ),
    }
}

/// `M:SS` of a length, for the rule counts.
fn minutes(t: Micros) -> String {
    let seconds = t.0 / 1_000_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
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

fn button(
    id: &'static str,
    text: impl Into<SharedString>,
    keys: &'static str,
    accent: bool,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(42.))
        .px(px(18.))
        .flex()
        .items_center()
        .gap(px(10.))
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .font_weight(if accent {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .when(accent, |this| {
            this.bg(ACCENT)
                .text_color(ON_ACCENT)
                .hover(|s| s.opacity(0.9))
        })
        .when(!accent, |this| {
            this.bg(RAISED).text_color(TEXT).hover(|s| s.bg(FLOAT))
        })
        .child(text.into())
        .child(kbd(keys))
}

impl CutScreen {
    fn rules_pane(&self, counts: &[RuleCount], cx: &mut Context<Self>) -> Div {
        let folder = self.folder.read(cx);
        let format = &folder.project.format;
        let pacing = format.pacing;
        let rows = counts.iter().map(|count| {
            let rule = count.rule;
            let on = run::rule_on(format, rule);
            let numbers = if count.queued > 0 {
                format!(
                    "{} · {} · {} to review",
                    count.removed,
                    minutes(count.removed_time),
                    count.queued
                )
            } else {
                format!("{} · {}", count.removed, minutes(count.removed_time))
            };
            div()
                .id(SharedString::from(format!("rule-{rule}")))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(12.))
                .py(px(10.))
                .rounded(RADIUS_MD)
                .bg(RAISED)
                .cursor_pointer()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().font_weight(FontWeight::MEDIUM).child(rule_name(rule)))
                        .child(mono(numbers).text_size(TEXT_SM).text_color(MUTED)),
                )
                .child(switch(on))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change(cx, |folder| run::set_rule(folder, rule, !on));
                }))
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
                    this.change(cx, |folder| run::set_pacing(folder, value));
                }),
            )
        };
        pane()
            .w(px(316.))
            .flex_none()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(6.))
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
                            .text_color(MUTED)
                            .child("Applied when confident. The rest waits for you."),
                    ),
            )
            .children(rows)
            .child(
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
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(pacing_note),
                    ),
            )
    }

    fn finding_pane(&mut self, queue: &[(TakeAt, Review)], cx: &mut Context<Self>) -> Div {
        let folder = self.folder.read(cx);
        let cut = folder.project.cut.clone();
        let dir = folder.dir.clone();
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
        let shown = self.shown(queue);
        let stats = div()
            .flex()
            .items_end()
            .gap(px(28.))
            .child(stat(timecode(raw_length(&cut)), "Raw", TEXT.into()))
            .child(stat(timecode(kept_length(&cut)), "Cut", ACCENT.into()))
            .child(stat(queue.len().to_string(), "Need your call", TEXT.into()))
            .child(
                div()
                    .ml_auto()
                    .text_size(px(14.))
                    .text_color(MUTED)
                    .children(shown.map(|i| format!("{} of {}", i + 1, queue.len()))),
            );
        let body = pane()
            .flex_1()
            .min_w(px(520.))
            .px(px(22.))
            .py(px(20.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(stats);
        let Some(index) = shown else {
            return body.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Nothing needs your call"),
                    )
                    .child(div().text_color(MUTED).child(
                        "Every confident cut is applied. Continue to edit when you are ready.",
                    )),
            );
        };
        let (at, review) = queue[index];
        let take = cut.chapters[at.chapter].takes[at.take].take.clone();
        let chapter = cut.chapters[at.chapter].title.clone();
        let (kind, _, why) = finding_text(review.by, review.range.duration());
        let words = folder
            .transcripts
            .get(&take)
            .map(|t| t.words().to_vec())
            .unwrap_or_default();
        let middle = |w: &shot_core::Word| Micros((w.range.from().0 + w.range.to().0) / 2);
        let inside: Vec<&str> = words
            .iter()
            .filter(|w| review.range.from() <= middle(w) && middle(w) < review.range.to())
            .map(|w| w.text.as_str())
            .collect();
        let before = words.partition_point(|w| middle(w) < review.range.from());
        let after = words.partition_point(|w| middle(w) < review.range.to());
        // Context is what plays around the finding, so removed words stay out of it.
        let block = &cut.chapters[at.chapter].takes[at.take];
        let plays = |w: &&shot_core::Word| {
            TimeRange::new(middle(w), Micros(middle(w).0 + 1)).is_some_and(|t| block.keeps(t))
        };
        let mut pre: Vec<&str> = words[..before]
            .iter()
            .rev()
            .filter(plays)
            .take(CONTEXT_WORDS)
            .map(|w| w.text.as_str())
            .collect();
        pre.reverse();
        let post: Vec<&str> = words[after..]
            .iter()
            .filter(plays)
            .take(CONTEXT_WORDS)
            .map(|w| w.text.as_str())
            .collect();
        let cut_text = if inside.is_empty() {
            match review.by {
                RuleKind::IdleScreen => {
                    format!("[screen idle {}]", model::seconds(review.range.duration()))
                }
                _ => format!("[pause {}]", model::seconds(review.range.duration())),
            }
        } else {
            inside.join(" ")
        };
        let length = model::seconds(review.range.duration());

        let window = TimeRange::new(
            Micros(review.range.from().0.saturating_sub(WAVE_MARGIN.0)),
            Micros(review.range.to().0 + WAVE_MARGIN.0),
        )
        .expect("the window holds the finding");
        let speech = self.speech(&take, &dir).cloned();
        let bars: Vec<f32> = (0..BARS)
            .map(|i| {
                let step = window.duration().0 / BARS as u64;
                let from = Micros(window.from().0 + i as u64 * step);
                let level = speech
                    .as_ref()
                    .and_then(|s| s.level(TimeRange::new(from, Micros(from.0 + step.max(1)))?));
                level.map_or(0.0, |db| ((db + 60.0) / 60.0).clamp(0.0, 1.0))
            })
            .collect();
        let region_from =
            (review.range.from().0 - window.from().0) as f32 / window.duration().0 as f32;
        let region_width = review.range.duration().0 as f32 / window.duration().0 as f32;
        let wave = div()
            .relative()
            .h(px(84.))
            .flex()
            .items_center()
            .gap(px(2.))
            .children(
                bars.into_iter()
                    .map(|h| div().flex_1().rounded(px(1.)).bg(WAVE).h(px(6. + h * 72.))),
            )
            .child(
                div()
                    .absolute()
                    .top(px(-6.))
                    .bottom(px(-6.))
                    .left(relative(region_from))
                    .w(relative(region_width))
                    .rounded(RADIUS_SM)
                    .bg(CUT_TINT)
                    .border_1()
                    .border_color(Hsla::from(CUT).opacity(0.45)),
            );
        let at_text = timecode(edited_time(&cut, at, review.range.from()));
        body.child(
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
                                .child(kind),
                        )
                        .child(mono(at_text.clone()).text_color(MUTED))
                        .child(div().text_color(MUTED).child(chapter)),
                )
                .child(div().text_color(BODY).line_height(px(23.)).child(why)),
        )
        .child(
            div()
                .px(px(20.))
                .py(px(18.))
                .rounded(RADIUS_LG)
                .bg(RAISED)
                .text_size(px(19.))
                .line_height(px(31.))
                .flex()
                .flex_wrap()
                .gap_x(px(6.))
                .when(!pre.is_empty(), |this| {
                    this.child(div().text_color(MUTED).child(pre.join(" ")))
                })
                .child(
                    div()
                        .px(px(4.))
                        .rounded(px(5.))
                        .bg(CUT_TINT)
                        .text_color(CUT_TEXT)
                        .line_through()
                        .text_decoration_color(CUT)
                        .child(cut_text),
                )
                .when(!post.is_empty(), |this| {
                    this.child(div().text_color(STRONG).child(post.join(" ")))
                }),
        )
        .child(
            div()
                .px(px(18.))
                .py(px(16.))
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
                        .child(mono(length.clone())),
                ),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(10.))
                .child(
                    button("keep", "Keep", "K", false)
                        .on_click(cx.listener(|this, _, _, cx| this.decide(Decision::Keep, cx))),
                )
                .child(
                    button("cut", format!("Cut {length}"), "C", true)
                        .on_click(cx.listener(|this, _, _, cx| this.decide(Decision::Cut, cx))),
                ),
        )
    }

    fn queue_pane(&self, queue: &[(TakeAt, Review)], cx: &mut Context<Self>) -> Div {
        let folder = self.folder.read(cx);
        let cut = &folder.project.cut;
        let shown = self.shown(queue);
        let mut rows: Vec<(Row, Micros)> = queue
            .iter()
            .map(|(at, review)| {
                (
                    Row::Open(*at, *review),
                    edited_time(cut, *at, review.range.from()),
                )
            })
            .collect();
        for (i, answered) in self.answered.iter().enumerate() {
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
            rows.push((Row::Answered(i), time));
        }
        rows.sort_by_key(|(_, t)| *t);
        let items = rows.into_iter().enumerate().map(|(n, (row, time))| {
            let (review, status, on, open_index) = match row {
                Row::Open(at, review) => {
                    let index = queue
                        .iter()
                        .position(|(a, r)| *a == at && r.range == review.range);
                    (
                        review,
                        model::seconds(review.range.duration()),
                        index == shown,
                        index,
                    )
                }
                Row::Answered(i) => {
                    let answered = &self.answered[i];
                    let status = match answered.decision {
                        Decision::Keep => "Kept",
                        Decision::Cut => "Cut",
                    };
                    (answered.review, status.to_owned(), false, None)
                }
            };
            let (kind, source, _) = finding_text(review.by, review.range.duration());
            let answered = open_index.is_none();
            div()
                .id(("finding", n))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(10.))
                .rounded(RADIUS_MD)
                .when(on, |this| this.bg(RAISED))
                .when(answered, |this| this.opacity(0.5))
                .when(!answered, |this| {
                    this.cursor_pointer().hover(|s| s.bg(RAISED))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().font_weight(FontWeight::MEDIUM).child(kind))
                        .child(div().text_size(TEXT_SM).text_color(MUTED).child(source)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(2.))
                        .child(mono(timecode(time)).text_size(px(14.)))
                        .child(
                            div()
                                .text_size(TEXT_SM)
                                .text_color(if answered { FAINT } else { BODY })
                                .child(status),
                        ),
                )
                .when_some(open_index, |this, index| {
                    let (at, review) = queue[index];
                    this.on_click(cx.listener(move |this, _, _, cx| {
                        this.current = Some((at, review.range));
                        cx.notify();
                    }))
                })
        });
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
                    .justify_between()
                    .px(px(12.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .child(label("Needs your call"))
                    .child(label("↑ ↓")),
            )
            .child(
                div()
                    .id("findings")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(items),
            )
            .child(
                div().pt(px(12.)).px(px(4.)).child(
                    div()
                        .id("continue")
                        .h(px(42.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(RADIUS_MD)
                        .bg(ACCENT)
                        .text_color(ON_ACCENT)
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.9))
                        .child("Continue to edit")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ContinueToEdit))),
                ),
            )
    }
}

impl Render for CutScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let queue = self.queue(cx);
        let counts = autocut::rule_counts(&self.folder.read(cx).project.cut);
        let rules = self.rules_pane(&counts, cx);
        let finding = self.finding_pane(&queue, cx);
        let list = self.queue_pane(&queue, cx);
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
            .children(self.error.clone().map(crate::ui::alert))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(8.))
                    .child(rules)
                    .child(finding)
                    .child(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::RemovedBy;

    #[test]
    fn a_finding_shows_at_its_edited_time() {
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
}
