//! The outline pane of the Record screen: what the next take records, and every take.

use gpui::{Context, Div, FontWeight, Stateful, div, prelude::*, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Disableable, Sizable};
use shot_core::Script;

use super::outline::{Part, Take, length};
use super::{RecordScreen, ReviewTakes};
use crate::takes::take_name;
use crate::theme::*;
use crate::ui::{icon, kbd, label, mono};

/// Width of the outline pane.
const WIDTH: f32 = 300.;

impl RecordScreen {
    pub(super) fn outline_pane(&self, busy: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let folder = self.folder.read(cx);
        let script = folder.project.script.clone();
        let has_beats = script.chapters.iter().any(|c| !c.beats.is_empty());
        let wpm = self
            .prompter
            .read(cx)
            .live()
            .lock()
            .pace()
            .words_per_minute
            .max(1.);
        let count = self.outline.takes.len();
        crate::ui::pane()
            .id("outline")
            .w(px(WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(10.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(6.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .child(label("Outline"))
                    .when(has_beats && !busy, |row| {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(TEXT_XS)
                                .text_color(MUTED)
                                .child(kbd("↑"))
                                .child(kbd("↓")),
                        )
                    }),
            )
            .when(!has_beats, |pane| {
                pane.child(
                    div()
                        .px(px(6.))
                        .pb(px(10.))
                        .text_size(TEXT_SM)
                        .line_height(px(19.))
                        .text_color(MUTED)
                        .child(
                            "Write chapters and beats in the script to record it part by part, \
                             with the prompter. Without them every take records freely.",
                        ),
                )
            })
            .when(has_beats, |pane| {
                pane.child(self.part_row(
                    Part::Script,
                    "Whole script",
                    format!(
                        "{} chapters · ~{}",
                        script.chapters.len(),
                        minutes(spoken(&script, None), wpm)
                    ),
                    None,
                    busy,
                    cx,
                ))
                .children(
                    script
                        .chapters
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| !c.beats.is_empty())
                        .map(|(index, chapter)| {
                            let takes = self
                                .outline
                                .chapters
                                .get(index)
                                .map_or(0, |c| c.takes.len());
                            let covered = self.outline.chapters.get(index).map_or(0, |c| c.covered);
                            let beats = chapter.beats.len();
                            let status = match takes {
                                0 => format!(
                                    "{beats} beats · ~{}",
                                    minutes(spoken(&script, Some(index)), wpm)
                                ),
                                1 => "1 take".into(),
                                n => format!("{n} takes"),
                            };
                            let status = if takes > 0 && covered < beats {
                                format!("{status} · {covered} of {beats} beats")
                            } else {
                                status
                            };
                            let done = takes > 0 && covered == beats;
                            let mut row = self.part_row(
                                Part::Chapter {
                                    chapter: index,
                                    from: 0,
                                },
                                &chapter.title,
                                status,
                                Some(done),
                                busy,
                                cx,
                            );
                            if let Part::Chapter {
                                chapter: picked,
                                from,
                            } = self.part
                                && picked == index
                            {
                                row = row.child(self.beats(index, from, &script, busy, cx));
                            }
                            row
                        }),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(6.))
                    .pt(px(16.))
                    .pb(px(6.))
                    .child(label("Takes"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(MUTED)
                                    .child(match count {
                                        0 => "None yet".to_owned(),
                                        1 => "1 take".to_owned(),
                                        n => format!("{n} takes"),
                                    }),
                            )
                            .child(
                                Button::new("import-clip")
                                    .xsmall()
                                    .ghost()
                                    .label("Import clip")
                                    .tooltip(
                                        "Make a phone clip or other video the next take. Or drop \
                                         it on this screen.",
                                    )
                                    .disabled(busy || self.import.read(cx).is_running())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.import.update(cx, |import, cx| import.pick(cx))
                                    })),
                            ),
                    ),
            )
            .children(
                self.import
                    .read(cx)
                    .row()
                    .map(|row| row.px(px(6.)).pb(px(6.))),
            )
            .children(
                self.outline
                    .takes
                    .iter()
                    .rev()
                    .map(|take| self.take_row(take, &script, busy, cx)),
            )
    }

    /// A row of the outline the next take can record.
    fn part_row(
        &self,
        part: Part,
        title: &str,
        status: String,
        done: Option<bool>,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let picked = match (part, self.part) {
            (Part::Chapter { chapter: a, .. }, Part::Chapter { chapter: b, .. }) => a == b,
            (a, b) => a == b,
        };
        let key = match part {
            Part::Script => 0,
            Part::Chapter { chapter, .. } => chapter + 1,
        };
        div().flex().flex_col().child(
            div()
                .id(("part", key))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(8.))
                .py(px(8.))
                .rounded(RADIUS_SM)
                .when(picked, |row| row.bg(RAISED))
                .when(!picked && !busy, |row| row.hover(|s| s.bg(CARD)))
                .when(!busy, |row| row.cursor_pointer())
                .child(
                    div()
                        .size(px(18.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(match done {
                            None => icon("script", ICON_SM, if picked { TEXT } else { MUTED }),
                            Some(true) => icon("check", ICON_SM, ACCENT),
                            Some(false) => {
                                icon("record", ICON_SM, if picked { RECORD } else { FAINT })
                            }
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if picked { STRONG } else { TEXT })
                                .child(title.to_owned()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .child(status),
                        ),
                )
                .when(picked, |row| {
                    row.child(div().w(px(3.)).h(px(22.)).rounded(px(2.)).bg(RECORD))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.pick_part(part, cx))),
        )
    }

    /// The picked chapter's beats: a click records from that beat on.
    fn beats(
        &self,
        chapter: usize,
        from: usize,
        script: &Script,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let beats = &script.chapters[chapter].beats;
        div()
            .flex()
            .flex_col()
            .pl(px(36.))
            .pr(px(6.))
            .py(px(4.))
            .children(beats.iter().enumerate().map(|(index, beat)| {
                let on = index >= from;
                div()
                    .id(("beat", index))
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .py(px(4.))
                    .px(px(6.))
                    .rounded(RADIUS_SM)
                    .text_size(TEXT_SM)
                    .text_color(if on { BODY } else { FAINT })
                    .when(!busy, |row| {
                        row.cursor_pointer()
                            .hover(|s| s.bg(CARD))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pick_part(
                                    Part::Chapter {
                                        chapter,
                                        from: index,
                                    },
                                    cx,
                                )
                            }))
                    })
                    .child(
                        mono((index + 1).to_string())
                            .w(px(14.))
                            .flex_none()
                            .text_color(MUTED),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .line_height(px(18.))
                            .child(beat.text.clone()),
                    )
                    .when(index == from && from > 0, |row| {
                        row.child(
                            div()
                                .text_size(TEXT_XS)
                                .text_color(RECORD_TEXT)
                                .child("starts here"),
                        )
                    })
            }))
    }

    /// A take: what it recorded, how long, whether the cut plays it, Review and Remove.
    fn take_row(&self, take: &Take, script: &Script, busy: bool, cx: &mut Context<Self>) -> Div {
        let id = take.id.clone();
        let covers = if take.chapters.is_empty() {
            if take.transcribed {
                "No beats matched".to_owned()
            } else {
                "No words yet".to_owned()
            }
        } else {
            take.chapters
                .iter()
                .filter_map(|&c| script.chapters.get(c).map(|c| c.title.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let confirming = self.removing.as_ref() == Some(&take.id);
        let key = take.id.to_string();
        let row = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(8.))
            .py(px(8.))
            .rounded(RADIUS_SM)
            .when(confirming, |row| row.bg(CUT_TINT))
            .child(
                div()
                    .id(gpui::SharedString::from(format!("take-{key}")))
                    .group("take")
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(
                                "takes",
                                ICON_SM,
                                if take.in_cut { ACCENT } else { FAINT },
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(take_name(&take.id)),
                                    )
                                    .child(
                                        mono(length(take.length))
                                            .text_size(TEXT_SM)
                                            .text_color(MUTED),
                                    ),
                            )
                            .child(div().truncate().text_size(TEXT_SM).text_color(MUTED).child(
                                if take.in_cut {
                                    covers
                                } else {
                                    format!("{covers} · not in the cut")
                                },
                            )),
                    )
                    .when(!busy && !confirming, |row| {
                        row.child(
                            small_button(("review", key.clone()), "takes", "Review in Cut")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(ReviewTakes))),
                        )
                        .child(
                            small_button(("remove", key.clone()), "delete", "Remove the take")
                                .on_click({
                                    let id = id.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.remove_take(id.clone(), cx)
                                    })
                                }),
                        )
                    }),
            );
        if confirming {
            row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(TEXT_SM)
                            .line_height(px(18.))
                            .text_color(BODY)
                            .child("Remove from the video? The media goes to the Trash."),
                    )
                    .child(
                        text_button(("keep", key.clone()), "Keep", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.removing = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        text_button(("confirm", key.clone()), "Remove", true).on_click({
                            let id = id.clone();
                            cx.listener(move |this, _, _, cx| this.remove_take(id.clone(), cx))
                        }),
                    ),
            )
        } else {
            row
        }
    }
}

/// An icon button of a take row, with its name as tooltip.
fn small_button(
    id: (&'static str, String),
    icon_name: &'static str,
    tip: &'static str,
) -> Stateful<Div> {
    div()
        .id(gpui::SharedString::from(format!("{}-{}", id.0, id.1)))
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_SM)
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(icon(icon_name, ICON_SM, MUTED))
}

fn text_button(id: (&'static str, String), text: &'static str, danger: bool) -> Stateful<Div> {
    div()
        .id(gpui::SharedString::from(format!("{}-{}", id.0, id.1)))
        .px(px(10.))
        .h(px(26.))
        .flex()
        .items_center()
        .rounded(RADIUS_SM)
        .text_size(TEXT_SM)
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .bg(if danger { CUT } else { FLOAT })
        .text_color(if danger { ON_ACCENT } else { TEXT })
        .child(text)
}

/// Words the reader speaks in `chapter`, or in the whole script.
fn spoken(script: &Script, chapter: Option<usize>) -> usize {
    script
        .chapters
        .iter()
        .enumerate()
        .filter(|(i, _)| chapter.is_none_or(|c| c == *i))
        .flat_map(|(_, c)| &c.beats)
        .map(|b| {
            let text = if b.script.is_empty() {
                &b.text
            } else {
                &b.script
            };
            text.split_whitespace().count()
        })
        .sum()
}

/// "1:20" for `words` at `wpm`.
fn minutes(words: usize, wpm: f32) -> String {
    let seconds = (words as f32 * 60. / wpm).round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
