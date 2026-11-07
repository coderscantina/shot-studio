//! The Picture workspace: the preview fills the stage, the layout picker on the left, what is on
//! screen now and next on the right, three rolling transcript lines, and the overview bar.

use gpui::{Context, Div, FontWeight, Window, div, prelude::*, px};
use shot_core::{Layout, WordState};

use super::EditScreen;
use super::parts::{self, layout_name};
use crate::model::{LAYOUT_KEYS, Span, timecode};
use crate::theme::*;
use crate::ui::{self, icon, kbd, label};

/// Words per rolling line.
const LINE_WORDS: usize = 9;

pub fn render(this: &mut EditScreen, _: &mut Window, cx: &mut Context<EditScreen>) -> Div {
    let chapter_frames = this
        .map
        .chapters
        .get(this.chapter)
        .map_or(0..0, |c| c.frames.clone());
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(parts::error_strip(this))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap(px(8.))
                .mx(px(8.))
                .child(layouts(this, cx))
                .child(stage(this, cx))
                .child(layers(this)),
        )
        .child(parts::overview_footer(this, chapter_frames, cx))
}

fn layouts(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let current = this.map.span_at(this.playhead).map(|s| s.layout);
    ui::pane()
        .w(px(176.))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(10.))
        .py(px(14.))
        .children(LAYOUT_KEYS.iter().enumerate().map(|(index, layout)| {
            let on = current == Some(*layout);
            div()
                .id(("layout", index))
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(8.))
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when(on, |this| this.bg(RAISED))
                .child(thumbnail(*layout))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .text_size(TEXT_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if on { STRONG } else { MUTED })
                        .child(layout_name(*layout))
                        .child(div().flex_1())
                        .child(kbd((index + 1).to_string())),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_layout(index, cx)))
        }))
}

/// A small drawing of where screen and face sit in a layout.
fn thumbnail(layout: Layout) -> Div {
    let page = gpui::rgb(0xe9eae5);
    let face = gpui::rgb(0x596170);
    let frame = div()
        .relative()
        .w_full()
        .aspect_ratio(16. / 9.)
        .rounded(px(5.))
        .overflow_hidden()
        .bg(gpui::rgb(0x1d2026));
    let block = |left: f32, top: f32, width: f32, height: f32| {
        div()
            .absolute()
            .left(gpui::relative(left))
            .top(gpui::relative(top))
            .w(gpui::relative(width))
            .h(gpui::relative(height))
            .rounded(px(3.))
    };
    let lines = |frame: Div| {
        frame
            .child(block(0.08, 0.14, 0.5, 0.08).bg(gpui::rgb(0xc4c6bf)))
            .child(block(0.08, 0.3, 0.44, 0.06).bg(gpui::rgb(0xd3d4ce)))
    };
    match layout {
        Layout::Face => frame.child(block(0.36, 0.18, 0.28, 0.64).rounded_full().bg(face)),
        Layout::FaceContent => {
            lines(frame.bg(page)).child(block(0.68, 0.48, 0.26, 0.44).bg(gpui::rgb(0x3a404b)))
        }
        Layout::Split => frame
            .child(block(0.0, 0.0, 0.5, 1.0).bg(page))
            .child(block(0.62, 0.22, 0.26, 0.56).rounded_full().bg(face)),
        _ => lines(frame.bg(page)).child(block(0.08, 0.42, 0.52, 0.06).bg(gpui::rgb(0xd3d4ce))),
    }
}

fn stage(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    ui::pane()
        .flex_1()
        .bg(WELL)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(26.))
        .p(px(24.))
        .child(
            div()
                .w_full()
                .max_w(px(1040.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(parts::preview(this))
                .child(parts::transport(this, false, cx)),
        )
        .child(lines(this))
}

/// The line being spoken, with the one before and after it.
fn lines(this: &EditScreen) -> impl IntoElement {
    let words: Vec<(&str, Option<u64>)> = this
        .doc
        .iter()
        .filter(|b| b.at.chapter == this.chapter)
        .flat_map(|b| {
            b.words
                .iter()
                .filter(|w| w.state == WordState::Kept)
                .map(|w| (w.text.as_str(), w.output))
        })
        .collect();
    let spoken = words
        .iter()
        .filter(|(_, output)| output.is_some_and(|o| o <= this.playhead))
        .count();
    let current = spoken.saturating_sub(1) / LINE_WORDS;
    let line = |index: usize| -> Option<&[(&str, Option<u64>)]> {
        let start = index * LINE_WORDS;
        (start < words.len()).then(|| &words[start..(start + LINE_WORDS).min(words.len())])
    };
    let text =
        |line: &[(&str, Option<u64>)]| line.iter().map(|(t, _)| *t).collect::<Vec<_>>().join(" ");
    div()
        .w_full()
        .max_w(px(1040.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.))
        .text_center()
        .child(
            div()
                .h(px(24.))
                .text_size(px(17.))
                .text_color(FAINT)
                .children(current.checked_sub(1).and_then(line).map(text)),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap_x(px(7.))
                .min_h(px(34.))
                .text_size(px(24.))
                .line_height(px(34.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(MUTED)
                .children(line(current).into_iter().flatten().enumerate().map(
                    |(offset, (word, _))| {
                        let index = current * LINE_WORDS + offset;
                        div()
                            .when(index < spoken, |w| w.text_color(TEXT))
                            .when(index + 1 == spoken, |w| {
                                w.text_color(STRONG)
                                    .underline()
                                    .text_decoration_2()
                                    .text_decoration_color(ACCENT)
                            })
                            .child(word.to_string())
                    },
                )),
        )
        .child(
            div()
                .h(px(24.))
                .text_size(px(17.))
                .text_color(FAINT)
                .children(line(current + 1).map(text)),
        )
}

fn layers(this: &EditScreen) -> impl IntoElement {
    let now = this.map.span_at(this.playhead);
    let source_frame = now.map(|s| s.source_frame(this.playhead));
    let zoom = now
        .zip(source_frame)
        .map(|(s, f)| s.zoom_at(f))
        .filter(|z| *z > 1.0);
    let time = source_frame.map(|f| shot_render::plan::frame_time(f, crate::model::FPS));
    let overlays: Vec<String> = now
        .zip(time)
        .map(|(s, t)| {
            s.overlays
                .iter()
                .filter(|o| o.range.from() <= t && t < o.range.to())
                .map(|o| o.template.to_string())
                .collect()
        })
        .unwrap_or_default();
    let next = upcoming(this, now);
    ui::pane()
        .w(px(236.))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(10.))
        .py(px(14.))
        .child(label("On screen now").px(px(10.)).pb(px(8.)))
        .children(now.map(|s| layer("layout", "Layout", layout_name(s.layout).into(), true)))
        .children(zoom.map(|z| layer("zoom", "Zoom", format!("{:.0}%", z * 100.0), false)))
        .children(
            overlays
                .into_iter()
                .map(|name| layer("picture", "Overlay", name, false)),
        )
        .child(div().h(px(14.)))
        .child(label("Next up").px(px(10.)).pb(px(8.)))
        .children(
            next.into_iter()
                .map(|(name, what, frame)| layer(name, &what, timecode(frame), false)),
        )
}

fn layer(icon_name: &str, name: &str, value: String, on: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(9.))
        .rounded(RADIUS_SM)
        .text_size(px(14.))
        .when(on, |this| this.bg(RAISED).text_color(STRONG))
        .child(icon(icon_name, ICON, if on { STRONG } else { MUTED }))
        .child(div().font_family(SANS).child(name.to_owned()))
        .child(div().flex_1())
        .child(
            div()
                .font_family(MONO)
                .text_size(TEXT_SM)
                .text_color(MUTED)
                .child(value),
        )
}

/// The next layout change and the next zoom after the playhead, with their output frames.
fn upcoming(this: &EditScreen, now: Option<&Span>) -> Vec<(&'static str, String, u64)> {
    let mut out = Vec::new();
    let after = this
        .map
        .spans
        .iter()
        .filter(|s| s.output_frame > this.playhead);
    if let Some(span) = after
        .clone()
        .find(|s| now.is_none_or(|n| n.layout != s.layout))
    {
        out.push((
            "layout",
            layout_name(span.layout).to_owned(),
            span.output_frame,
        ));
    }
    let zoom = this
        .map
        .spans
        .iter()
        .filter(|s| s.output_frames().end > this.playhead)
        .flat_map(|s| {
            s.zooms.iter().filter_map(move |z| {
                let start = shot_render::plan::snap(z.range, crate::model::FPS).start;
                (s.source_frames.contains(&start))
                    .then(|| (z.scale, s.output_frame + (start - s.source_frames.start)))
            })
        })
        .find(|(_, frame)| *frame > this.playhead);
    if let Some((scale, frame)) = zoom {
        out.push(("zoom", format!("Zoom {scale}%"), frame));
    }
    out.sort_by_key(|(.., frame)| *frame);
    out
}
