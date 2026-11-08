//! The Text workspace: chapters, the transcript as a document, the preview with a selection
//! inspector, and the overview bar. The outline and the preview column resize by their edges.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, ScrollHandle, Stateful, Window, div, prelude::*, px,
};
use gpui_component::tooltip::Tooltip;
use shot_core::{Placed, RangeEdit, TakeAt, TimeRange, WordState, Zoom};

use super::EditScreen;
use super::parts::{self, layout_name, row};
use super::proposals::{self, Card};
use super::timeline::{Pick, zoom_scales};
use crate::layout_store;
use crate::model::{BlockDoc, CutRun, Item, LAYOUT_KEYS, Paragraph, seconds, timecode};
use crate::theme::*;
use crate::ui::{self, hint, icon, kbd, label, mono};

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
        .child(inspector(this, cx));
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
                paragraph_element(this, block, paragraph, cx).into_any_element(),
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

fn paragraph_element(
    this: &EditScreen,
    block: &BlockDoc,
    paragraph: &Paragraph,
    cx: &mut Context<EditScreen>,
) -> Div {
    let at = block.at;
    let zooms = block_zooms(this, at, cx);
    let mut children: Vec<gpui::AnyElement> = Vec::new();
    for item in &paragraph.items {
        match item {
            Item::Word(i) => {
                children.push(word(this, block, *i, &zooms, cx).into_any_element());
            }
            Item::Cut(run) => {
                let open = this
                    .expanded
                    .contains(&super::run_key(at, run.range.from()));
                children.push(cut(at, run, open, cx).into_any_element());
                if open {
                    for i in run.words.clone() {
                        children.push(word(this, block, i, &zooms, cx).into_any_element());
                    }
                }
            }
        }
    }
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .text_size(px(18.))
        .line_height(px(32.))
        .text_color(BODY)
        .children(children)
}

/// The zooms on show in the block at `at`, each with whether it is the picked one.
fn block_zooms(this: &EditScreen, at: TakeAt, cx: &App) -> Vec<(TimeRange, bool)> {
    let folder = this.folder.read(cx);
    let picked = match this.lanes.picked {
        Some(Pick::Placed(p, Placed::Zoom(index))) if p == at => folder
            .project
            .cut
            .chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .and_then(|b| b.zooms.get(index))
            .map(|z| z.range),
        _ => None,
    };
    folder
        .shown
        .cut
        .chapters
        .get(at.chapter)
        .and_then(|c| c.takes.get(at.take))
        .map(|b| {
            b.zooms
                .iter()
                .map(|z| (z.range, picked == Some(z.range)))
                .collect()
        })
        .unwrap_or_default()
}

fn word(
    this: &EditScreen,
    block: &BlockDoc,
    index: usize,
    zooms: &[(TimeRange, bool)],
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let at = block.at;
    let word = &block.words[index];
    // A word is zoomed when its middle is inside a zoom; the picked zoom is tinted stronger.
    let middle = shot_core::Micros((word.range.from().0 + word.range.to().0) / 2);
    let zoom = zooms
        .iter()
        .find(|(range, _)| range.from() <= middle && middle < range.to())
        .map(|(_, picked)| *picked);
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
    div()
        .id(("word", key))
        .when(starts_selection || ends_selection, |w| w.relative())
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
        .when_some(zoom.filter(|_| !removed), |w, picked| {
            w.bg(if picked {
                Hsla::from(ZOOM_TINT)
            } else {
                Hsla::from(ZOOM).opacity(0.14)
            })
        })
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
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.dragging = true;
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

/// The selected words' tools, wrapping inside `room` when it is too narrow for one row.
fn selection_toolbar(this: &EditScreen, room: f32, cx: &mut Context<EditScreen>) -> Div {
    let Some((_, range, kept, removed)) = this.selected() else {
        return div();
    };
    let sound_open = this.place.picker == Some(super::place::Picker::Sound);
    let count = this.selection.map_or(0, |s| s.words().count());
    let words = if count == 1 { "word" } else { "words" };
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
                        tool("cut", "Cut", "⌫")
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
                        tool("zoom", "Zoom", "Z")
                            .id("text-tool-zoom")
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT_ON))
                            .on_click(cx.listener(|this, _, _, cx| this.add_zoom(cx))),
                    )
                })
                .child(
                    tool("system-audio", "Sound", "S")
                        .id("tool-sound")
                        .cursor_pointer()
                        .hover(|s| s.bg(FLOAT_ON))
                        .when(sound_open, |t| t.bg(FLOAT_ON))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_picker(super::place::Picker::Sound, cx)
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(2.))
                        .h(px(28.))
                        .pl(px(10.))
                        .pr(px(4.))
                        .child(div().pr(px(6.)).text_color(MUTED).child("Layout"))
                        .children(LAYOUT_KEYS.iter().enumerate().map(|(index, layout)| {
                            let name = layout_name(*layout);
                            div()
                                .id(("text-tool-layout", index))
                                .h(px(24.))
                                .px(px(3.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .hover(|s| s.bg(FLOAT_ON))
                                .child(kbd((index + 1).to_string()))
                                .tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(name).build(window, cx)
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.set_layout(index, cx)),
                                )
                        })),
                ),
        )
        .when(sound_open, |bar| {
            bar.child(super::place::sound_picker(this, cx).w(px(560f32.min(room).max(0.))))
        })
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
        .child(kbd(keys))
}

/// Under the preview: the selected words, or without a selection the chapter's cuts and the
/// keys that edit it; at the bottom what is on screen at the playhead.
fn inspector(this: &EditScreen, cx: &mut Context<EditScreen>) -> Stateful<Div> {
    let folder = this.folder.read(cx);
    let captions = folder
        .project
        .cut
        .captions
        .unwrap_or(folder.project.format.captions);
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
    let body = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .px(px(18.))
        .py(px(16.))
        .map(|pane| match &selection {
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
                    .child(hint("⌫", "Cut the selected words"))
                    .child(hint("R", "Restore cut words"))
                    .child(hint("Z", "Zoom in on them"))
                    .child(hint("S", "Place a sound"))
                    .child(hint("1–4", "Set the layout"))
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
                .child(here(
                    "text",
                    "Captions",
                    div().text_color(TEXT).child(match captions {
                        shot_core::CaptionStyle::WordPop => "Word pop",
                        shot_core::CaptionStyle::Clean => "Clean",
                        shot_core::CaptionStyle::Off => "Off",
                    }),
                )),
        )
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
                .child(super::zoom::controls(this, at, index, zoom, cx))
        })
        .child(
            div().flex().items_center().gap(px(10.)).child(
                tool("close", "Remove", "⌫")
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
