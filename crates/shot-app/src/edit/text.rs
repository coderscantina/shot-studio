//! The Text workspace: chapters, the transcript as a document, the preview with a selection
//! inspector, and the overview bar. The outline and the preview column resize by their edges.

use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, Stateful, Window, div, prelude::*, px,
};
use shot_core::{TakeAt, WordState};

use super::EditScreen;
use super::parts::{self, layout_name, row};
use super::proposals::{self, Card};
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
            pane.child(
                div()
                    .px(px(10.))
                    .text_color(MUTED)
                    .child("No chapters yet. Record a take to start one."),
            )
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
    let mut rows: Vec<Div> = Vec::new();
    let mut scroll_to = None;
    let following = *follow.read(cx);
    let playing_in = current_paragraph(this);
    if let Some(note) = proposals::agent_note(this) {
        rows.push(doc_row(rail(None, None), note));
    }
    for block in blocks {
        if !block.transcribed {
            rows.push(doc_row(
                rail(None, None),
                div().text_color(MUTED).child(format!(
                    "Take {} has no transcript yet. Run shot transcribe --video <folder> --take {} to make one.",
                    block.take, block.take
                )),
            ));
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
                        proposals::card_element(this, card, false, cx),
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
                paragraph_element(this, block, paragraph, cx),
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
        .flex_1()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(28.))
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
                .px(px(28.))
                .py(px(26.))
                .flex()
                .flex_col()
                .gap(px(22.))
                .children(rows),
        )
}

/// A row of the transcript: the rail and its content, centered at the document's width.
fn doc_row(rail: Div, content: impl IntoElement) -> Div {
    div()
        .mx_auto()
        .w_full()
        .max_w(px(132. + 24. + 680.))
        .flex()
        .flex_none()
        .gap(px(24.))
        .child(rail)
        .child(div().flex_1().min_w_0().max_w(px(680.)).child(content))
}

/// The paragraph's timecode and layout, right-aligned beside it.
fn rail(start: Option<u64>, layout: Option<&'static str>) -> Div {
    div()
        .w(px(132.))
        .pt(px(6.))
        .flex()
        .flex_col()
        .items_end()
        .gap(px(4.))
        .text_size(TEXT_SM)
        .font_weight(FontWeight::MEDIUM)
        .text_color(FAINT)
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
    let selection = this.selection.filter(|s| s.at == at).map(|s| s.words());
    let mut children: Vec<gpui::AnyElement> = Vec::new();
    let mut ends_selection = false;
    for item in &paragraph.items {
        match item {
            Item::Word(i) => {
                ends_selection |= selection.as_ref().is_some_and(|s| *s.end() == *i);
                children.push(word(this, block, *i, cx).into_any_element());
            }
            Item::Cut(run) => {
                let open = this
                    .expanded
                    .contains(&super::run_key(at, run.range.from()));
                children.push(cut(at, run, open, cx).into_any_element());
                if open {
                    for i in run.words.clone() {
                        ends_selection |= selection.as_ref().is_some_and(|s| *s.end() == i);
                        children.push(word(this, block, i, cx).into_any_element());
                    }
                }
            }
        }
    }
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .text_size(px(18.))
                .line_height(px(32.))
                .text_color(BODY)
                .children(children),
        )
        .when(ends_selection, |this_paragraph| {
            this_paragraph.child(selection_toolbar(this, cx))
        })
}

fn word(
    this: &EditScreen,
    block: &BlockDoc,
    index: usize,
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
    let key = at.chapter * 1_000_000 + at.take * 100_000 + index;
    div()
        .id(("word", key))
        .pr(px(5.))
        .cursor_text()
        .rounded(px(3.))
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
        .when(selected, |w| w.bg(SELECTION).text_color(STRONG))
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

/// Anchored under the paragraph that holds the end of the selection, 8 px clear of the text.
fn selection_toolbar(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
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
                .flex()
                .items_center()
                .gap(px(2.))
                .p(px(4.))
                .rounded(px(11.))
                .bg(FLOAT)
                .shadow_lg()
                .text_size(TEXT_SM)
                .child(
                    div()
                        .px(px(10.))
                        .pl(px(8.))
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
            bar.child(super::place::sound_picker(this, cx).max_w(px(560.)))
        })
}

fn tool(name: &'static str, text: &'static str, keys: &'static str) -> Div {
    div()
        .flex()
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
    ui::pane()
        .id("inspector")
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(16.))
        .px(px(18.))
        .py(px(16.))
        .overflow_y_scroll()
        .map(|pane| match &selection {
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
                )),
            None => pane.child(chapter_summary(this)).child(
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
        })
        .child(
            div()
                .mt_auto()
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(10.))
                .p(px(14.))
                .mx(px(-4.))
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

/// Words of a long selection the inspector quotes.
const QUOTE_WORDS: usize = 40;

/// The chapter on show: its length and what the cuts took out of it.
fn chapter_summary(this: &EditScreen) -> Div {
    let Some(chapter) = this.map.chapters.get(this.chapter) else {
        return div();
    };
    let runs: Vec<&CutRun> = this
        .doc
        .iter()
        .filter(|b| b.at.chapter == this.chapter)
        .flat_map(|b| &b.paragraphs)
        .flat_map(|p| &p.items)
        .filter_map(|item| match item {
            Item::Cut(run) => Some(run),
            Item::Word(_) => None,
        })
        .collect();
    let removed = shot_core::Micros(runs.iter().map(|r| r.removed.0).sum());
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
            div().text_color(TEXT).child(match runs.len() {
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
