//! The Text workspace: chapters, the transcript as a document, the preview with a selection
//! inspector, and the overview bar. The outline and the preview column resize by their edges.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, ScrollHandle, Stateful, Window, div, prelude::*, px,
};
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use shot_core::{Layout, Micros, Placed, RangeEdit, TakeAt, WordState, Zoom};

use super::EditScreen;
use super::marks::{self, Kind, Mark, MarkId, Own, Switch};
use super::parts::{self, layout_name, row};
use super::proposals::{self, Card};
use super::timeline::{Pick, zoom_scales};
use crate::layout_store;
use crate::model::{
    BlockDoc, CutRun, DocWord, Item, LAYOUT_KEYS, Paragraph, apply_layout, seconds, timecode,
};
use crate::theme::*;
use crate::ui::{self, hint, icon, kbd, label, mono};

/// The Text workspace's view of the effects: the one under the pointer, the one picked for the
/// inspector, the layout picker and the last note of the selection tools.
#[derive(Default)]
pub struct TextState {
    pub hovered: Option<MarkId>,
    pub picked: Option<MarkId>,
    pub layouts_open: bool,
    note: Option<String>,
    /// Vision is looking for the selected words on the screen.
    finding: bool,
}

impl EditScreen {
    /// Picks an effect from its chip: the inspector shows it and Delete removes a person's.
    fn pick_mark(&mut self, id: MarkId, cx: &mut Context<Self>) {
        let Some(mark) = marks::marks(self, id.at, cx)
            .into_iter()
            .find(|m| m.id == id)
        else {
            return;
        };
        self.selection = None;
        self.place.picker = None;
        self.text.layouts_open = false;
        self.text.picked = Some(id);
        self.lanes.picked = match mark.own {
            Some(Own::Placed(placed)) => Some(Pick::Placed(id.at, placed)),
            _ => None,
        };
        if let Some(frame) = self.map.output_frame(id.at, mark.range.from()) {
            self.set_playing(false);
            self.seek(frame);
        }
        cx.notify();
    }

    /// The picked effect, read again from the files.
    fn picked_mark(&self, cx: &App) -> Option<Mark> {
        let id = self.text.picked?;
        marks::marks(self, id.at, cx)
            .into_iter()
            .find(|m| m.id == id)
    }

    /// Drops the picked effect. Returns whether one was picked.
    pub(super) fn drop_text_pick(&mut self) -> bool {
        let had = self.text.picked.take().is_some();
        if had {
            self.lanes.picked = None;
        }
        had
    }

    /// Removes the picked sound a person placed. Returns whether there was one.
    pub(super) fn remove_picked_sound(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(mark) = self.picked_mark(cx) else {
            return false;
        };
        let Some(Own::Sound { at }) = mark.own else {
            return false;
        };
        let Some(take) = self.block(mark.id.at).map(|b| b.take.clone()) else {
            return false;
        };
        self.edit(cx, |cut| {
            let mut cut =
                crate::style::with_override(cut, &take, shot_core::ElementKind::Word, at, |o| {
                    o.effects.retain(|e| {
                        !matches!(
                            e,
                            shot_core::OverrideEffect::Set(shot_core::Effect::Sfx { .. })
                        )
                    })
                });
            cut.overrides
                .retain(|o| o.decision.is_some() || !o.effects.is_empty());
            Ok(cut)
        });
        true
    }

    /// H: a highlight around where the selected words show on the screen as they are said.
    /// Vision reads the screen in the background.
    pub(super) fn add_highlight(&mut self, cx: &mut Context<Self>) {
        let Some((at, range, true, _)) = self.selected() else {
            return;
        };
        let (Some(block), Some(selection)) = (self.block(at), self.selection) else {
            return;
        };
        if self.text.finding {
            return;
        }
        let phrase = block.words[selection.words()]
            .iter()
            .filter(|w| w.state == WordState::Kept)
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let take = block.take.clone();
        let dir = self.folder.read(cx).dir.clone();
        self.text.finding = true;
        self.text.note = Some("Looking for these words on the screen…".into());
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    shot_render::overlay_rules::highlight_said(&dir, &take, range, &phrase)
                })
                .await;
            this.update(cx, |this, cx| {
                this.text.finding = false;
                match found {
                    Ok(overlay) => {
                        this.text.note = None;
                        this.selection = None;
                        this.edit(cx, |cut| crate::place::with_overlay(cut, at, overlay));
                    }
                    Err(error) => {
                        this.text.note = Some(format!(
                            "No highlight: {error}. A highlight marks words that are on the screen"
                        ));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// L: opens the layout picker under the selection.
    pub(super) fn toggle_layouts(&mut self, cx: &mut Context<Self>) {
        if self.selected().is_none() {
            return;
        }
        self.text.layouts_open = !self.text.layouts_open;
        self.place.picker = None;
        cx.notify();
    }
}

pub fn render(this: &mut EditScreen, window: &mut Window, cx: &mut Context<EditScreen>) -> Div {
    let chapter_frames = this
        .map
        .chapters
        .get(this.chapter)
        .map_or(0..0, |c| c.frames.clone());
    // Set by a click in the outline: the transcript scrolls to the playhead's paragraph once.
    let follow = window.use_keyed_state("text-follow", cx, |_, _| false);
    let outline = outline(this, &follow, cx);
    let document = document(this, &follow, cx);
    let side = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            ui::pane()
                .flex_none()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(parts::preview(this))
                .child(parts::transport(this, false, cx)),
        )
        .child(if this.captions.open {
            ui::pane()
                .id("inspector")
                .flex_1()
                .flex()
                .flex_col()
                .child(super::captions::panel(this, cx))
        } else {
            inspector(this, cx)
        });
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(parts::error_strip(this))
        .child(layout_store::TEXT.render(outline, document, side, window, cx))
        .child(parts::overview_footer(this, chapter_frames, cx))
}

/// The chapters, and under the one on show its paragraphs by their first words. A click on a
/// paragraph seeks there and scrolls the transcript to it.
fn outline(
    this: &EditScreen,
    follow: &Entity<bool>,
    cx: &mut Context<EditScreen>,
) -> Stateful<Div> {
    let current = current_paragraph(this);
    let mut list: Vec<AnyElement> = Vec::new();
    for (index, chapter) in this.map.chapters.iter().enumerate() {
        let on = index == this.chapter;
        let length = chapter.frames.end - chapter.frames.start;
        let start = (!chapter.frames.is_empty()).then_some(chapter.frames.start);
        list.push(
            div()
                .id(("chapter", index))
                .flex()
                .items_baseline()
                .gap(px(8.))
                .px(px(10.))
                .py(px(9.))
                .rounded(RADIUS_SM)
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { STRONG } else { MUTED })
                .when(on, |this| this.bg(RAISED))
                .when(!on, |this| this.hover(|s| s.bg(RAISED).text_color(TEXT)))
                .cursor_pointer()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(chapter.title.clone()),
                )
                .child(mono(timecode(length)).text_size(TEXT_SM))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.chapter = index;
                    this.selection = None;
                    this.lanes.picked = None;
                    if let Some(start) = start {
                        this.seek(start);
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        if !on {
            continue;
        }
        let mut row = 0usize;
        let mut paragraphs = div().flex().flex_col().pl(px(12.)).pt(px(2.)).pb(px(8.));
        for block in this.doc.iter().filter(|b| b.at.chapter == index) {
            for (number, paragraph) in block.paragraphs.iter().enumerate() {
                let Some(start) = paragraph.start else {
                    continue;
                };
                let words: Vec<&str> = paragraph
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        Item::Word(i) => block.words.get(*i).map(|w| w.text.as_str()),
                        Item::Cut(_) => None,
                    })
                    .take(12)
                    .collect();
                let now = current == Some((block.at, number));
                let follow = follow.clone();
                paragraphs = paragraphs.child(
                    div()
                        .id(("paragraph", row))
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(RADIUS_SM)
                        .text_size(px(14.))
                        .text_color(if now { TEXT } else { MUTED })
                        .cursor_pointer()
                        .hover(|s| s.bg(RAISED).text_color(TEXT))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .line_height(px(20.))
                                .line_clamp(2)
                                .text_ellipsis()
                                .child(words.join(" ")),
                        )
                        .child(
                            mono(timecode(start))
                                .pt(px(2.))
                                .text_size(TEXT_XS)
                                .text_color(if now { MUTED } else { FAINT }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selection = None;
                            this.seek(start);
                            follow.update(cx, |follow, _| *follow = true);
                            cx.notify();
                        })),
                );
                row += 1;
            }
        }
        if row > 0 {
            list.push(paragraphs.into_any_element());
        }
    }
    ui::pane()
        .id("chapters")
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(10.))
        .py(px(14.))
        .overflow_y_scroll()
        .when(list.is_empty(), |pane| {
            pane.child(div().px(px(10.)).text_color(MUTED).child("No chapters yet"))
        })
        .children(list)
}

/// The paragraph of the chapter on show that the playhead is in, by its block and index.
fn current_paragraph(this: &EditScreen) -> Option<(TakeAt, usize)> {
    this.doc
        .iter()
        .filter(|b| b.at.chapter == this.chapter)
        .flat_map(|b| {
            b.paragraphs
                .iter()
                .enumerate()
                .filter_map(move |(i, p)| Some((b.at, i, p.start?)))
        })
        .take_while(|(.., start)| *start <= this.playhead)
        .last()
        .map(|(at, i, _)| (at, i))
}

fn document(this: &mut EditScreen, follow: &Entity<bool>, cx: &mut Context<EditScreen>) -> Div {
    if this.map.chapters.is_empty() {
        return ui::pane().flex_1().flex().flex_col().child(
            ui::empty_state(
                "record",
                "Nothing recorded yet",
                "Record a take, and its words show here to edit like text.",
            )
            .child(
                div().pt(px(14.)).child(
                    ui::big_button("record-a-take", "record", "Record a take", "⌘↩", true)
                        .on_click(cx.listener(|this, _, _, cx| this.go_record(cx))),
                ),
            ),
        );
    }
    let title = this
        .map
        .chapters
        .get(this.chapter)
        .map(|c| c.title.clone())
        .unwrap_or_default();
    let range = this
        .map
        .chapters
        .get(this.chapter)
        .map(|c| format!("{} to {}", timecode(c.frames.start), timecode(c.frames.end)));
    let blocks: Vec<&BlockDoc> = this
        .doc
        .iter()
        .filter(|b| b.at.chapter == this.chapter)
        .collect();
    let default_layout = this.folder.read(cx).shown.format.default_layout;
    let columns = Columns::for_width(f32::from(this.document_bounds.get().size.width));
    let doc_row = |rail: Div, content: AnyElement| doc_row(columns, rail, content);
    let rail = |start, layout| rail(columns, start, layout);
    let mut rows: Vec<Div> = Vec::new();
    let mut scroll_to = None;
    let following = *follow.read(cx);
    let playing_in = current_paragraph(this);
    if let Some(note) = proposals::agent_note(this) {
        rows.push(doc_row(rail(None, None), note.into_any_element()));
    }
    let mut offered_transcribe = false;
    for block in blocks {
        if !block.transcribed {
            // One Transcribe action covers every take without words.
            if !offered_transcribe {
                offered_transcribe = true;
                rows.push(doc_row(
                    rail(None, None),
                    this.transcribe.clone().into_any_element(),
                ));
            }
            continue;
        }
        let effects = Effects {
            marks: marks::marks(this, block.at, cx),
            switches: marks::switches(this, block.at, default_layout),
        };
        let mut index = 0;
        while let Some(paragraph) = block.paragraphs.get(index) {
            let here: Vec<&Card> = this
                .cards
                .iter()
                .filter(|c| c.at == block.at && c.paragraphs.start == index)
                .collect();
            if let Some(end) = here.iter().map(|c| c.paragraphs.end).max() {
                for card in here {
                    if this.scroll_to_card
                        && this.proposal.as_deref()
                            == Some(this.proposals[card.proposal].name.as_str())
                    {
                        scroll_to = Some(rows.len());
                    }
                    rows.push(doc_row(
                        rail(paragraph.start, Some(layout_name(paragraph.layout))),
                        proposals::card_element(this, card, false, cx).into_any_element(),
                    ));
                }
                index = end;
                continue;
            }
            if following && playing_in == Some((block.at, index)) {
                scroll_to = Some(rows.len());
            }
            let current = paragraph.start.is_some_and(|start| start <= this.playhead)
                && block
                    .paragraphs
                    .get(index + 1)
                    .and_then(|next| next.start)
                    .is_none_or(|next| this.playhead < next);
            rows.push(doc_row(
                rail(paragraph.start, Some(layout_name(paragraph.layout)))
                    .when(current, |rail| rail.text_color(MUTED)),
                paragraph_element(this, block, paragraph, &effects, cx).into_any_element(),
            ));
            index += 1;
        }
    }
    if let Some(row) = scroll_to {
        this.scroll.scroll_to_top_of_item(row);
    }
    this.scroll_to_card = false;
    if following {
        follow.update(cx, |follow, _| *follow = false);
    }
    ui::pane()
        .relative()
        .flex_1()
        .flex()
        .flex_col()
        .child(
            ui::measure(this.document_bounds.clone())
                .absolute()
                .size_full(),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(columns.pad))
                .pt(px(18.))
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::BOLD)
                        .child(title),
                )
                .children(range.map(|r| mono(r).text_color(MUTED))),
        )
        .child(
            div()
                .id("transcript")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&this.scroll)
                .px(px(columns.pad))
                .py(px(26.))
                .flex()
                .flex_col()
                .gap(px(22.))
                .children(rows)
                .children(floating_toolbar(this, columns.pad, cx)),
        )
}

/// A word's element key, unique across the video.
fn word_key(at: TakeAt, index: usize) -> usize {
    at.chapter * 1_000_000 + at.take * 100_000 + index
}

/// Where a selected word sits, by its key and its bounds in the transcript's scrolled
/// content, so the selection toolbar scrolls with it.
pub type Anchor = Rc<Cell<Option<(usize, Bounds<Pixels>)>>>;

/// An invisible canvas over a selected word that records it in `anchor`. A new place
/// draws one more frame, so the toolbar is never a frame behind the selection.
fn anchor(anchor: Anchor, key: usize, scroll: ScrollHandle) -> gpui::Canvas<()> {
    gpui::canvas(
        move |bounds, window, _| {
            let content = scroll.bounds().origin + scroll.offset();
            let at = Some((key, Bounds::new(bounds.origin - content, bounds.size)));
            if anchor.get() != at {
                anchor.set(at);
                window.on_next_frame(|window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
}

/// Room kept between the toolbar and the selection or the pane's edge.
const CLEAR: f32 = 8.;

/// The selection toolbar over the transcript: above the first selected word, below the last
/// when there is no room above in the pane, inside the pane's side padding and [`CLEAR`] off the
/// words. It covers the text under it and never moves it. Hidden for the one frame before the
/// selected words are measured.
fn floating_toolbar(this: &EditScreen, pad: f32, cx: &mut Context<EditScreen>) -> Option<Div> {
    let selection = this.selection?;
    this.selected()?;
    let words = selection.words();
    let first = word_key(selection.at, *words.start());
    let last = word_key(selection.at, *words.end());
    let viewport = this.scroll.bounds().size;
    let offset = f32::from(this.scroll.offset().y);
    let room = f32::from(viewport.width) - 2. * pad;
    let size = this.toolbar_bounds.get().size;
    let (width, height) = (f32::from(size.width), f32::from(size.height));
    let start = this.selection_start.get().filter(|(k, _)| *k == first);
    let end = this.selection_anchor.get().filter(|(k, _)| *k == last);
    let measured = start.zip(end);
    let (left, top) = measured.map_or((pad, 0.), |((_, first), (_, last))| {
        let above = f32::from(first.top()) - CLEAR - height;
        let (word, top) = if above + offset >= 0. {
            (first, above)
        } else {
            (last, f32::from(last.bottom()) + CLEAR)
        };
        let left = f32::from(word.left()).min(pad + room - width).max(pad);
        (left, top)
    });
    Some(
        div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .max_w(px(room.max(0.)))
            .when(measured.is_none(), |bar| bar.invisible())
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(
                ui::measure(this.toolbar_bounds.clone())
                    .absolute()
                    .size_full(),
            )
            .child(selection_toolbar(this, room, cx)),
    )
}

/// The transcript's widest reading column.
const READING: f32 = 680.;

/// The rail, gap and side padding of the transcript for the document pane's width. They
/// narrow before the reading column does; below [`STACK_BELOW`] the rail moves above each
/// paragraph, so lines stay long down to the 1280x800 minimum.
#[derive(Clone, Copy)]
struct Columns {
    /// The rail's width, 0 when it sits above the paragraph.
    rail: f32,
    gap: f32,
    pad: f32,
}

/// The document pane's width below which the rail sits above each paragraph.
const STACK_BELOW: f32 = 560.;

impl Columns {
    fn for_width(width: f32) -> Self {
        let (rail, gap, pad) = if width >= 900. {
            (132., 24., 28.)
        } else if width >= 680. {
            (96., 18., 22.)
        } else if width >= STACK_BELOW {
            (76., 14., 16.)
        } else {
            (0., 4., 18.)
        };
        Self { rail, gap, pad }
    }

    fn stacked(self) -> bool {
        self.rail == 0.
    }
}

/// A row of the transcript: the rail and its content, centered at the document's width.
fn doc_row(columns: Columns, rail: Div, content: AnyElement) -> Div {
    div()
        .mx_auto()
        .w_full()
        .max_w(px(columns.rail + columns.gap + READING))
        .flex()
        .when(columns.stacked(), |row| row.flex_col())
        .flex_none()
        .gap(px(columns.gap))
        .child(rail)
        .child(div().flex_1().min_w_0().max_w(px(READING)).child(content))
}

/// The paragraph's timecode and layout, right-aligned beside it or in a line above it.
fn rail(columns: Columns, start: Option<u64>, layout: Option<&'static str>) -> Div {
    let rail = div()
        .flex()
        .flex_none()
        .text_size(TEXT_SM)
        .font_weight(FontWeight::MEDIUM)
        .text_color(FAINT);
    if columns.stacked() {
        return rail
            .items_baseline()
            .gap(px(8.))
            .children(start.map(|s| mono(timecode(s)).text_color(MUTED)))
            .children(layout);
    }
    rail.w(px(columns.rail))
        .text_right()
        .pt(px(6.))
        .flex_col()
        .items_end()
        .gap(px(4.))
        .children(start.map(|s| mono(timecode(s)).text_color(MUTED)))
        .children(layout)
}

/// The effects of one block, read once for all its paragraphs.
struct Effects {
    marks: Vec<Mark>,
    switches: Vec<Switch>,
}

impl Effects {
    /// The effects covering a word that is spoken at `middle`, sounds at its start.
    fn on(&self, word: &DocWord) -> impl Iterator<Item = &Mark> {
        let middle = Micros((word.range.from().0 + word.range.to().0) / 2);
        let start = word.range.from();
        self.marks.iter().filter(move |m| match m.kind {
            Kind::Sound { .. } => {
                sound_word(m, word) || (m.covers(start) && m.range.from() == start)
            }
            _ => m.covers(middle),
        })
    }
}

/// Whether the sound `mark` sits on `word`: the word spoken at its time, or starting nearest.
fn sound_word(mark: &Mark, word: &DocWord) -> bool {
    let at = mark.range.from();
    word.range.from() <= at && at < word.range.to()
}

/// The index of the first word of `block` an effect starting at `from` lands on: the first
/// kept word whose middle is at or after it, or that it starts inside.
fn first_word(block: &BlockDoc, from: Micros) -> Option<usize> {
    block.words.iter().position(|w| {
        w.state == WordState::Kept
            && (w.range.from() <= from && from < w.range.to()
                || Micros((w.range.from().0 + w.range.to().0) / 2) >= from)
    })
}

fn paragraph_element(
    this: &EditScreen,
    block: &BlockDoc,
    paragraph: &Paragraph,
    effects: &Effects,
    cx: &mut Context<EditScreen>,
) -> Stateful<Div> {
    let at = block.at;
    let mut children: Vec<gpui::AnyElement> = Vec::new();
    let push_word = |children: &mut Vec<AnyElement>, i: usize, cx: &mut Context<EditScreen>| {
        for switch in effects
            .switches
            .iter()
            .filter(|s| first_word(block, s.run.range.from()) == Some(i))
        {
            children.push(layout_chip(this, at, switch, i, cx).into_any_element());
        }
        for mark in effects
            .marks
            .iter()
            .filter(|m| first_word(block, m.range.from()) == Some(i))
        {
            children.push(mark_chip(this, mark, cx).into_any_element());
        }
        children.push(word(this, block, i, effects, cx).into_any_element());
    };
    for item in &paragraph.items {
        match item {
            Item::Word(i) => push_word(&mut children, *i, cx),
            Item::Cut(run) => {
                let open = this
                    .expanded
                    .contains(&super::run_key(at, run.range.from()));
                children.push(cut(at, run, open, cx).into_any_element());
                if open {
                    for i in run.words.clone() {
                        children.push(word(this, block, i, effects, cx).into_any_element());
                    }
                }
            }
        }
    }
    let key = paragraph.items.first().map(|item| match item {
        Item::Word(i) => word_key(at, *i),
        Item::Cut(run) => word_key(at, run.words.start),
    });
    div()
        .id(("paragraph-text", key.unwrap_or_default()))
        .flex()
        .flex_wrap()
        .items_center()
        .text_size(px(18.))
        .line_height(px(32.))
        .text_color(BODY)
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if !hovered && this.text.hovered.take().is_some() {
                cx.notify();
            }
        }))
        .children(children)
}

/// How a word shows an effect on it: the bar under it or the wash behind it, and whether the
/// bar runs on into the next word.
fn effect_bars(
    this: &EditScreen,
    block: &BlockDoc,
    index: usize,
    effects: &Effects,
) -> (Vec<Div>, Option<Hsla>) {
    let word = &block.words[index];
    let next = block.words.get(index + 1);
    let mut bars = Vec::new();
    let mut wash = None;
    let mut lane = 0.;
    for mark in effects.on(word) {
        let lit = this.text.picked == Some(mark.id) || this.text.hovered == Some(mark.id);
        let strength = match (lit, mark.own.is_some()) {
            (true, _) => 1.,
            (false, true) => 0.85,
            (false, false) => 0.45,
        };
        let hue = mark.hue();
        if mark.washes() {
            wash = Some(hue.opacity(if lit { 0.34 } else { 0.18 * strength + 0.02 }));
            continue;
        }
        if let Kind::Sound { .. } = mark.kind {
            continue;
        }
        let runs_on = next
            .is_some_and(|n| n.state == WordState::Kept && effects.on(n).any(|m| m.id == mark.id));
        bars.push(
            div()
                .absolute()
                .left_0()
                .right(px(if runs_on { 0. } else { 5. }))
                .bottom(px(1. + lane))
                .h(px(if lit { 3. } else { 2. }))
                .rounded(px(1.))
                .bg(hue.opacity(strength)),
        );
        lane += 4.;
        if lit {
            wash = wash.or(Some(hue.opacity(0.16)));
        }
    }
    (bars, wash)
}

fn word(
    this: &EditScreen,
    block: &BlockDoc,
    index: usize,
    effects: &Effects,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let at = block.at;
    let word = &block.words[index];
    let selected = this
        .selection
        .is_some_and(|s| s.at == at && s.words().contains(&index));
    let next_output = block.words[index + 1..].iter().find_map(|w| w.output);
    let now = word.output.is_some_and(|start| {
        start <= this.playhead && next_output.is_none_or(|next| this.playhead < next)
    });
    let removed = word.state != WordState::Kept;
    let key = word_key(at, index);
    let (starts_selection, ends_selection) = this
        .selection
        .filter(|s| s.at == at)
        .map_or((false, false), |s| {
            (*s.words().start() == index, *s.words().end() == index)
        });
    let (bars, wash) = if removed {
        (Vec::new(), None)
    } else {
        effect_bars(this, block, index, effects)
    };
    let hover = effects.on(word).next().map(|m| m.id);
    div()
        .id(("word", key))
        .relative()
        .when(starts_selection, |w| {
            w.child(
                anchor(this.selection_start.clone(), key, this.scroll.clone())
                    .absolute()
                    .size_full(),
            )
        })
        .when(ends_selection, |w| {
            w.child(
                anchor(this.selection_anchor.clone(), key, this.scroll.clone())
                    .absolute()
                    .size_full(),
            )
        })
        .pr(px(5.))
        .cursor_text()
        .rounded(px(3.))
        .when_some(wash, |w, wash| w.bg(wash))
        .children(bars)
        .when(removed, |w| {
            w.text_color(gpui::rgb(0xb98b87))
                .line_through()
                .text_decoration_color(CUT)
                .bg(Hsla::from(CUT).opacity(0.08))
        })
        .when(now && !removed, |w| {
            w.text_color(STRONG)
                .underline()
                .text_decoration_2()
                .text_decoration_color(ACCENT)
        })
        .when(selected, |w| {
            w.bg(super::word_selection()).text_color(STRONG)
        })
        .child(word.text.clone())
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered && this.text.hovered != hover {
                this.text.hovered = hover;
                cx.notify();
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.dragging = true;
                this.text.picked = None;
                if !this.text.finding {
                    this.text.note = None;
                }
                this.select_word(at, index, event.modifiers.shift, cx);
            }),
        )
        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            if this.dragging
                && event.pressed_button == Some(MouseButton::Left)
                && let Some(selection) = this.selection
                && selection.at == at
                && selection.head != index
            {
                this.selection = Some(crate::model::Selection {
                    head: index,
                    ..selection
                });
                cx.notify();
            }
        }))
}

/// The chip where an effect starts: its icon and a few words in its hue, filled for a
/// person's effect and outlined for the style's. A click picks it for the inspector, and
/// Delete removes a person's.
fn mark_chip(this: &EditScreen, mark: &Mark, cx: &mut Context<EditScreen>) -> Stateful<Div> {
    let id = mark.id;
    let hue = mark.hue();
    let lit = this.text.picked == Some(id) || this.text.hovered == Some(id);
    let own = mark.own.is_some();
    let tip = match &mark.style {
        Some(style) => format!("{}: {}. {style}", mark.name(), mark.label),
        None => format!("{}: {}", mark.name(), mark.label),
    };
    let key = word_key(id.at, id.index) * 4 + id.kind as usize;
    div()
        .id(("mark", key))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(20.))
        .px(px(6.))
        .mr(px(5.))
        .rounded(px(5.))
        .text_size(TEXT_XS)
        .font_weight(FontWeight::MEDIUM)
        .line_height(px(16.))
        .cursor_pointer()
        .when(own, |chip| {
            chip.bg(hue.opacity(if lit { 0.34 } else { 0.18 }))
        })
        .when(!own, |chip| {
            chip.border_1()
                .border_color(hue.opacity(if lit { 0.8 } else { 0.4 }))
        })
        .text_color(if own { hue } else { hue.opacity(0.75) })
        .child(icon(mark.icon(), px(12.), hue))
        .child(div().max_w(px(140.)).truncate().child(mark.label.clone()))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            let next = hovered.then_some(id);
            if this.text.hovered != next {
                this.text.hovered = next;
                cx.notify();
            }
        }))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, _, cx| this.pick_mark(id, cx)))
}

/// The chip where the layout switches: the layout's name, and a menu to change it or remove
/// the switch, which brings the layout before it back.
fn layout_chip(
    this: &EditScreen,
    at: TakeAt,
    switch: &Switch,
    word: usize,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let screen = cx.entity();
    let (layout, range, before) = (switch.run.layout, switch.run.range, switch.before);
    let default = this.folder.read(cx).shown.format.default_layout;
    let choices = layout_choices(this, cx);
    Button::new(("layout-switch", word_key(at, word)))
        .ghost()
        .xsmall()
        .mr(px(5.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(TEXT_XS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(TEXT)
                .child(icon("layout", px(12.), MUTED))
                .child(layout_name(layout))
                .child(icon("chevron-down", px(10.), MUTED)),
        )
        .dropdown_menu(move |menu, _, _| {
            let menu = menu.min_w(px(200.)).label("Layout from here");
            let menu = choices.iter().fold(menu, |menu, choice| {
                let (screen, choice) = (screen.clone(), *choice);
                menu.item(
                    PopupMenuItem::new(layout_name(choice))
                        .checked(choice == layout)
                        .on_click(move |_, _, cx| {
                            screen.update(cx, |this, cx| {
                                this.edit(cx, |cut| apply_layout(cut, at, range, choice, default))
                            })
                        }),
                )
            });
            let screen = screen.clone();
            menu.separator().item(
                PopupMenuItem::new(format!(
                    "Remove the switch, back to {}",
                    layout_name(before)
                ))
                .on_click(move |_, _, cx| {
                    screen.update(cx, |this, cx| {
                        this.edit(cx, |cut| apply_layout(cut, at, range, before, default))
                    })
                }),
            )
        })
}

/// The layouts on offer, one list for the switch menus and the L picker: the four on the number
/// keys, then the format's other layouts.
fn layout_choices(this: &EditScreen, cx: &App) -> Vec<Layout> {
    let format = &this.folder.read(cx).shown.format;
    let mut choices = LAYOUT_KEYS.to_vec();
    for layout in std::iter::once(&format.default_layout).chain(&format.other_layouts) {
        if !choices.contains(layout) {
            choices.push(*layout);
        }
    }
    choices
}

/// A cut between words: a faint seam for short auto cuts, a red length chip for the rest.
/// Clicking opens it to show the removed words.
fn cut(at: TakeAt, run: &CutRun, open: bool, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let key = (run.range.from().0 / 1000) as usize;
    let from = run.range.from();
    let toggle = cx.listener(move |this, _, _, cx| {
        let key = super::run_key(at, from);
        if !this.expanded.remove(&key) {
            this.expanded.insert(key);
        }
        cx.notify();
    });
    if run.is_seam() && !open && run.words.is_empty() {
        return div()
            .id(("seam", key))
            .w(px(2.))
            .h(px(14.))
            .mx(px(3.))
            .mr(px(8.))
            .rounded(px(1.))
            .bg(FAINT)
            .on_click(toggle);
    }
    div()
        .id(("cut", key))
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(20.))
        .px(px(6.))
        .mr(px(6.))
        .rounded(px(5.))
        .bg(CUT_TINT)
        .text_color(CUT_TEXT)
        .font_family(MONO)
        .text_size(TEXT_XS)
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .child(div().w(px(2.)).h(px(10.)).rounded(px(1.)).bg(CUT))
        .child(seconds(run.removed))
        .on_click(toggle)
}

/// The selected words' tools, wrapping inside `room` when it is too narrow for one row: cut
/// or restore, and the effects in their lane hues with their keys. The open picker and the
/// last note sit under it.
fn selection_toolbar(this: &EditScreen, room: f32, cx: &mut Context<EditScreen>) -> Div {
    let Some((_, range, kept, removed)) = this.selected() else {
        return div();
    };
    let picker = this.place.picker;
    let count = this.selection.map_or(0, |s| s.words().count());
    let words = if count == 1 { "word" } else { "words" };
    let effect = |id: &'static str,
                  name: &'static str,
                  text: &'static str,
                  keys: &'static str,
                  hue: Hsla,
                  on: bool| {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(10.))
            .rounded(RADIUS_SM)
            .cursor_pointer()
            .hover(|s| s.bg(FLOAT_ON))
            .when(on, |t| t.bg(FLOAT_ON))
            .child(icon(name, ICON, hue))
            .child(text)
            .child(kbd(keys))
    };
    let overlay: Hsla = gpui::rgb(marks::OVERLAY).into();
    let width = px(560f32.min(room).max(0.));
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(8.))
        .child(
            div()
                .max_w_full()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(2.))
                .p(px(4.))
                .rounded(px(11.))
                .bg(FLOAT)
                .shadow_lg()
                .text_size(TEXT_SM)
                .child(
                    div()
                        .flex_none()
                        .px(px(10.))
                        .pl(px(8.))
                        .whitespace_nowrap()
                        .child(format!("{count} {words}, {}", seconds(range.duration()))),
                )
                .when(kept, |bar| {
                    bar.child(
                        tool("cut", "Cut", super::DELETE_KEYS)
                            .id("text-tool-cut")
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT_ON))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.delete_selection(&super::DeleteSelection, window, cx)
                            })),
                    )
                })
                .when(removed, |bar| {
                    bar.child(
                        tool("restore", "Restore", "R")
                            .id("text-tool-restore")
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT_ON))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore_selection(&super::RestoreSelection, window, cx)
                            })),
                    )
                })
                .when(kept, |bar| {
                    bar.child(
                        effect("text-tool-zoom", "zoom", "Zoom", "Z", ZOOM.into(), false)
                            .on_click(cx.listener(|this, _, _, cx| this.add_zoom(cx))),
                    )
                    .child(
                        effect(
                            "text-tool-highlight",
                            "edit",
                            if this.text.finding {
                                "Finding…"
                            } else {
                                "Highlight"
                            },
                            "H",
                            ACCENT.into(),
                            this.text.finding,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.add_highlight(cx))),
                    )
                })
                .child(
                    effect(
                        "tool-sound",
                        "system-audio",
                        "Sound",
                        "S",
                        MUTED.into(),
                        picker == Some(super::place::Picker::Sound),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_picker(super::place::Picker::Sound, cx)
                    })),
                )
                .child(
                    effect(
                        "text-tool-pop",
                        "spark",
                        "Pop",
                        "P",
                        overlay,
                        picker == Some(super::place::Picker::Pop),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_picker(super::place::Picker::Pop, cx)
                    })),
                )
                .child(
                    effect(
                        "text-tool-layout",
                        "layout",
                        "Layout",
                        "L",
                        MUTED.into(),
                        this.text.layouts_open,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_layouts(cx))),
                ),
        )
        .map(|bar| match picker {
            Some(super::place::Picker::Sound) => {
                bar.child(super::place::sound_picker(this, cx).w(width))
            }
            Some(super::place::Picker::Pop) => {
                bar.child(super::place::pop_picker(this, cx).w(width))
            }
            None if this.text.layouts_open => bar.child(layout_picker(this, cx).w(width)),
            None => bar,
        })
        .children(this.text.note.clone().map(|note| {
            div()
                .max_w(width)
                .px(px(12.))
                .py(px(8.))
                .rounded(RADIUS_SM)
                .bg(FLOAT)
                .text_size(TEXT_XS)
                .text_color(if this.text.finding { MUTED } else { WARN })
                .child(note)
        }))
}

/// The layouts the selected words can switch to, the current one lit, with their keys.
fn layout_picker(this: &EditScreen, cx: &mut Context<EditScreen>) -> Div {
    let current = this
        .selection_frames()
        .and_then(|f| this.map.span_at(f.start))
        .map(|s| s.layout);
    let default = this.folder.read(cx).shown.format.default_layout;
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(10.))
        .rounded(px(11.))
        .bg(FLOAT)
        .shadow_lg()
        .text_size(TEXT_SM)
        .child(
            div()
                .flex()
                .items_center()
                .font_weight(FontWeight::MEDIUM)
                .child("Layout over the selection")
                .child(div().flex_1())
                .child(kbd("esc")),
        )
        .child(
            div().flex().flex_wrap().gap(px(4.)).children(
                layout_choices(this, cx)
                    .into_iter()
                    .enumerate()
                    .map(|(index, layout)| {
                        let on = current == Some(layout);
                        let key = LAYOUT_KEYS.iter().position(|l| *l == layout);
                        div()
                            .id(("text-layout-choice", index))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(28.))
                            .px(px(10.))
                            .rounded(RADIUS_SM)
                            .bg(if on { FLOAT_ON } else { RAISED })
                            .border_1()
                            .border_color(if on {
                                Hsla::from(ACCENT)
                            } else {
                                Hsla::from(RAISED)
                            })
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT_ON))
                            .child(icon("layout", ICON_SM, if on { STRONG } else { MUTED }))
                            .child(layout_name(layout))
                            .children(key.map(|k| kbd((k + 1).to_string())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let Some((at, range, ..)) = this.selected() else {
                                    return;
                                };
                                this.text.layouts_open = false;
                                this.edit(cx, |cut| apply_layout(cut, at, range, layout, default));
                            }))
                    }),
            ),
        )
}

fn tool(name: &'static str, text: &'static str, keys: &'static str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(28.))
        .px(px(10.))
        .rounded(RADIUS_SM)
        .child(icon(name, ICON, TEXT))
        .child(text)
        .when(!keys.is_empty(), |tool| tool.child(kbd(keys)))
}

/// Under the preview: the selected words, or without a selection the chapter's cuts and the
/// keys that edit it; at the bottom what is on screen at the playhead.
fn inspector(this: &EditScreen, cx: &mut Context<EditScreen>) -> Stateful<Div> {
    let span = this.map.span_at(this.playhead);
    let selection = this
        .selected()
        .zip(this.selection)
        .and_then(|((at, range, ..), s)| {
            let block = this.block(at)?;
            let words = &block.words[s.words()];
            let mut text: Vec<&str> = words
                .iter()
                .take(QUOTE_WORDS)
                .map(|w| w.text.as_str())
                .collect();
            if words.len() > QUOTE_WORDS {
                text.push("…");
            }
            Some((
                text.join(" "),
                words.len(),
                range,
                block.words[*s.words().start()].output,
            ))
        });
    let zoom = inspected_zoom(this, cx);
    let picked_zoom = matches!(this.lanes.picked, Some(Pick::Placed(_, Placed::Zoom(_))));
    // A person's zoom keeps its scale controls; every other picked effect gets its section.
    let mark = this
        .picked_mark(cx)
        .filter(|m| !(matches!(m.kind, Kind::Zoom { .. }) && m.own.is_some()));
    let body = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .px(px(18.))
        .py(px(16.))
        .map(|pane| match &selection {
            _ if mark.is_some() => pane.children(mark.map(|mark| mark_section(this, mark, cx))),
            _ if picked_zoom => pane.children(zoom.map(|zoom| zoom_section(this, zoom, true, cx))),
            Some((text, count, range, start)) => pane
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("Selection"))
                        .child(div().flex_1())
                        .child(label(match count {
                            1 => "1 word".to_owned(),
                            n => format!("{n} words"),
                        })),
                )
                .child(
                    div()
                        .text_size(px(16.))
                        .line_height(px(24.))
                        .text_color(STRONG)
                        .child(format!("“{text}”")),
                )
                .child(row(
                    "Starts at",
                    mono(start.map_or("removed".into(), timecode)).text_color(TEXT),
                ))
                .child(row(
                    "Length",
                    div().text_color(TEXT).child(seconds(range.duration())),
                ))
                .children(zoom.map(|zoom| zoom_section(this, zoom, false, cx))),
            // Nothing recorded: the document says what to do, so no word keys here.
            None if this.map.chapters.is_empty() => pane,
            None => pane.child(chapter_summary(this, cx)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label("Click a word to select it, Shift-click to extend"))
                    .child(hint(super::DELETE_KEYS, "Cut the selected words"))
                    .child(hint("R", "Restore cut words"))
                    .child(hint("Z", "Zoom in on them"))
                    .child(hint("H", "Highlight them on the screen"))
                    .child(hint("S", "Place a sound"))
                    .child(hint("P", "Pop an emoji or sticker"))
                    .child(hint("L", "Switch the layout, or 1–4"))
                    .child(hint("←→", "Step word by word, ⇧ to extend")),
            ),
        });
    // What is on screen stays in view under the rest, which scrolls when the pane is short.
    ui::pane()
        .id("inspector")
        .flex_1()
        .flex()
        .flex_col()
        .child(ui::scrolling_list(
            "inspector-body",
            &this.inspector_scroll,
            body,
        ))
        .child(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(10.))
                .p(px(14.))
                .mx(px(14.))
                .mb(px(16.))
                .rounded(RADIUS_MD)
                .bg(RAISED)
                .child(label(format!("At {}", timecode(this.playhead))))
                .child(here(
                    "layout",
                    "Layout",
                    div()
                        .text_color(TEXT)
                        .child(span.map_or("", |s| layout_name(s.layout))),
                ))
                .when_some(
                    span.map(|s| s.zoom_at(s.source_frame(this.playhead)))
                        .filter(|z| *z > 1.0),
                    |block, zoom| {
                        block.child(here(
                            "zoom",
                            "Zoom",
                            div()
                                .text_color(TEXT)
                                .child(format!("{:.0}%", zoom * 100.0)),
                        ))
                    },
                )
                .child(here("text", "Captions", super::captions::entry(this, cx))),
        )
}

/// A picked effect other than a person's zoom: what it is and where it plays, where a style
/// rule made it, and Remove for a person's.
fn mark_section(this: &EditScreen, mark: Mark, cx: &mut Context<EditScreen>) -> Div {
    let hue = mark.hue();
    let start = this.map.output_frame(mark.id.at, mark.range.from());
    let sound = match &mark.kind {
        Kind::Sound { name } => Some(name.clone()),
        _ => None,
    };
    let own = mark.own.is_some();
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(icon(mark.icon(), ICON_SM, hue))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(mark.name()))
                .child(div().flex_1())
                .child(label(if own { "Yours" } else { "From the style" })),
        )
        .child(
            div()
                .text_size(px(16.))
                .line_height(px(24.))
                .text_color(STRONG)
                .child(mark.label.clone()),
        )
        .child(row(
            "Starts at",
            mono(start.map_or("removed".into(), timecode)).text_color(TEXT),
        ))
        .when(sound.is_none(), |section| {
            section.child(row(
                "Length",
                div().text_color(TEXT).child(seconds(mark.range.duration())),
            ))
        })
        .children(mark.style.clone().map(label))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .children(sound.map(|name| {
                    tool("play", "Listen", "")
                        .id("text-mark-listen")
                        .bg(RAISED)
                        .cursor_pointer()
                        .hover(|s| s.bg(FLOAT))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.audition(name.clone(), cx)),
                        )
                }))
                .when(own, |row| {
                    row.child(
                        tool("close", "Remove", super::DELETE_KEYS)
                            .id("text-mark-remove")
                            .bg(RAISED)
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.delete_selection(&super::DeleteSelection, window, cx)
                            })),
                    )
                }),
        )
        .child(label(if own {
            "Move or trim it by its edges in the Timeline"
        } else {
            "Change the rule in the brand or the format to change it everywhere"
        }))
}

/// The zoom the inspector shows: the picked one, else a person's zoom the selection overlaps.
fn inspected_zoom(this: &EditScreen, cx: &App) -> Option<(TakeAt, usize, Zoom)> {
    let cut = &this.folder.read(cx).project.cut;
    let block = |at: TakeAt| cut.chapters.get(at.chapter)?.takes.get(at.take);
    if let Some(Pick::Placed(at, Placed::Zoom(index))) = this.lanes.picked {
        return block(at)?.zooms.get(index).map(|z| (at, index, *z));
    }
    let (at, range, ..) = this.selected()?;
    block(at)?
        .zooms
        .iter()
        .enumerate()
        .find(|(_, z)| z.range.overlaps(range))
        .map(|(index, z)| (at, index, *z))
}

/// A zoom's scale, where it starts and Remove, with the same choices as the Timeline's
/// toolbar. `picked` when it is the inspector's subject rather than under a selection.
fn zoom_section(
    this: &EditScreen,
    (at, index, zoom): (TakeAt, usize, Zoom),
    picked: bool,
    cx: &mut Context<EditScreen>,
) -> Div {
    let start = this.map.output_range(at, zoom.range).map(|f| f.start);
    let title = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(icon("zoom", ICON_SM, ZOOM))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(if picked {
            "Zoom"
        } else {
            "In a zoom"
        }))
        .child(div().flex_1())
        .child(label(format!("{}%", zoom.scale)));
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .when(!picked, |section| section.pt(px(6.)))
        .child(title)
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(zoom_scales(at, index, zoom.scale, cx)),
        )
        .when(picked, |section| {
            section
                .child(row(
                    "Starts at",
                    mono(start.map_or("removed".into(), timecode))
                        .id("text-zoom-start")
                        .text_color(TEXT)
                        .when(start.is_some(), |time| {
                            time.underline()
                                .cursor_pointer()
                                .hover(|s| s.text_color(STRONG))
                                .tooltip(|window, cx| {
                                    Tooltip::new("Go to the zoom").build(window, cx)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.seek_to_zoom(at, index, cx)
                                }))
                        }),
                ))
                .child(row(
                    "Length",
                    div().text_color(TEXT).child(seconds(zoom.range.duration())),
                ))
        })
        .child(
            div().flex().items_center().gap(px(10.)).child(
                tool("close", "Remove", super::DELETE_KEYS)
                    .id("text-zoom-remove")
                    .bg(RAISED)
                    .cursor_pointer()
                    .hover(|s| s.bg(FLOAT))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.lanes.picked = Some(Pick::Placed(at, Placed::Zoom(index)));
                        this.remove_picked(cx);
                    })),
            ),
        )
        .child(label("Move or trim it by its edges in the Timeline"))
}

/// Words of a long selection the inspector quotes.
const QUOTE_WORDS: usize = 40;

/// The chapter on show: its length and what the cuts took out of it. Counts from the cut list,
/// so takes without a transcript count too; back-to-back removals are one cut.
fn chapter_summary(this: &EditScreen, cx: &App) -> Div {
    let Some(chapter) = this.map.chapters.get(this.chapter) else {
        return div();
    };
    let cut = &this.folder.read(cx).project.cut;
    let (mut cuts, mut removed) = (0, 0);
    for take in cut
        .chapters
        .get(this.chapter)
        .into_iter()
        .flat_map(|c| &c.takes)
    {
        let mut previous_removed = false;
        for edit in &take.ranges {
            let is_removed = matches!(edit, RangeEdit::Remove { .. });
            if is_removed {
                cuts += usize::from(!previous_removed);
                removed += edit.range().duration().0;
            }
            previous_removed = is_removed;
        }
    }
    let removed = shot_core::Micros(removed);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .child(chapter.title.clone()),
        )
        .child(row(
            "Length",
            mono(timecode(chapter.frames.end - chapter.frames.start)).text_color(TEXT),
        ))
        .child(row(
            "Cut out",
            div().text_color(TEXT).child(match cuts {
                0 => "Nothing yet".to_owned(),
                1 => format!("{} in 1 cut", seconds(removed)),
                n => format!("{} in {n} cuts", seconds(removed)),
            }),
        ))
}

/// A row of the "At" block: its icon, name and value.
fn here(icon_name: &'static str, name: &'static str, value: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(14.))
        .child(icon(icon_name, ICON_SM, MUTED))
        .child(div().text_color(MUTED).child(name))
        .child(div().flex_1())
        .child(value)
}
