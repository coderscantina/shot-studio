//! The Picture workspace: the preview fills the stage, the layout picker on the left, what is on
//! screen now and next on the right with the tools that place callouts, pops and sounds, three
//! rolling transcript lines, and the overview bar. The side panes resize by their edges.

use gpui::{
    Context, Div, Entity, FontWeight, Hsla, MouseButton, MouseDownEvent, Pixels, Size, Stateful,
    Window, canvas, div, prelude::*, px,
};
use shot_core::{Layout, WordState};

use super::EditScreen;
use super::parts::{self, layout_name};
use super::place::{self, Picker, tool_button};
use crate::layout_store;
use crate::model::{FPS, LAYOUT_KEYS, Span, timecode};
use crate::theme::*;
use crate::ui::{self, icon, kbd, label, mono};

/// Words per rolling line.
const LINE_WORDS: usize = 9;
/// The stage's padding around the preview.
const STAGE_PADDING: f32 = 24.;
/// Height of the transport under the preview, with the gap above it.
const TRANSPORT: f32 = 46.;
/// Height of the three rolling lines, with the gap above them.
const LINES: f32 = 26. + 24. + 6. + 34. + 6. + 24.;

pub fn render(this: &mut EditScreen, window: &mut Window, cx: &mut Context<EditScreen>) -> Div {
    let chapter_frames = this
        .map
        .chapters
        .get(this.chapter)
        .map_or(0..0, |c| c.frames.clone());
    // The stage's size as last laid out, which sizes the preview to fill it.
    let stage_size = window.use_keyed_state("picture-stage", cx, |_, _| Size::<Pixels>::default());
    let layouts = layouts(this, cx);
    let stage = stage(this, &stage_size, cx);
    let layers = layers(this, cx);
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(parts::error_strip(this))
        .child(layout_store::PICTURE.render(layouts, stage, layers, window, cx))
        .child(parts::overview_footer(this, chapter_frames, cx))
}

fn layouts(this: &EditScreen, cx: &mut Context<EditScreen>) -> Stateful<Div> {
    let current = this.map.span_at(this.playhead).map(|s| s.layout);
    ui::pane()
        .id("layouts")
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(10.))
        .py(px(14.))
        .overflow_y_scroll()
        .children(LAYOUT_KEYS.iter().enumerate().map(|(index, layout)| {
            let on = current == Some(*layout);
            div()
                .id(("layout", index))
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(6.))
                .p(px(8.))
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when(on, |this| this.bg(RAISED))
                .when(!on, |this| this.hover(|s| s.bg(RAISED)))
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

/// The widest 16:9 preview that fits a stage of `size` with the transport and the lines.
fn preview_width(size: Size<Pixels>) -> Pixels {
    let width = f32::from(size.width) - 2. * STAGE_PADDING;
    let height = f32::from(size.height) - 2. * STAGE_PADDING - TRANSPORT - LINES;
    px(width.min(height * 16. / 9.).max(0.))
}

fn stage(this: &EditScreen, size: &Entity<Size<Pixels>>, cx: &mut Context<EditScreen>) -> Div {
    let measured = *size.read(cx);
    // Before the first layout the stage has no size yet: the settled 1040 px stands in.
    let width = if measured.width > px(0.) {
        preview_width(measured)
    } else {
        px(1040.)
    };
    let size = size.clone();
    ui::pane()
        .flex_1()
        .relative()
        .overflow_hidden()
        .bg(WELL)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(26.))
        .p(px(STAGE_PADDING))
        .child(
            canvas(
                move |bounds, _, cx| {
                    size.update(cx, |size, cx| {
                        if *size != bounds.size {
                            *size = bounds.size;
                            cx.notify();
                        }
                    })
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(
            div()
                .w(width)
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(callout_target(this, cx))
                .child(parts::transport(this, false, cx)),
        )
        .child(lines(this).w(width))
}

/// The preview, which places a callout where it is clicked while the callout tool is on.
fn callout_target(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let bounds = this.preview_bounds.clone();
    let on = this.place.callout;
    div()
        .id("picture-preview")
        .relative()
        .child(parts::preview(this))
        .child(crate::ui::measure(bounds).absolute().size_full())
        .when(on, |preview| {
            preview.cursor_crosshair().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let b = this.preview_bounds.get();
                    if b.size.width > px(0.) && b.size.height > px(0.) {
                        let x = (event.position.x - b.origin.x) / b.size.width;
                        let y = (event.position.y - b.origin.y) / b.size.height;
                        this.place_callout((x, y), cx);
                    }
                }),
            )
        })
}

/// The line being spoken, with the one before and after it.
fn lines(this: &EditScreen) -> Div {
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
                .truncate()
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
                .truncate()
                .children(line(current + 1).map(text)),
        )
}

/// What is on screen at the playhead, what changes next, and the tools that place effects.
fn layers(this: &EditScreen, cx: &mut Context<EditScreen>) -> Stateful<Div> {
    let now = this.map.span_at(this.playhead);
    let zoom = now
        .map(|s| s.zoom_at(s.source_frame(this.playhead)))
        .filter(|z| *z > 1.0);
    let folder = this.folder.read(cx);
    let captions = match folder
        .project
        .cut
        .captions
        .unwrap_or(folder.project.format.captions)
    {
        shot_core::CaptionStyle::WordPop => "Word pop",
        shot_core::CaptionStyle::Clean => "Clean",
        shot_core::CaptionStyle::Off => "Off",
    };
    let overlays = overlays(this, cx);
    let showing: Vec<&Overlay> = overlays
        .iter()
        .filter(|o| o.frames.contains(&this.playhead))
        .collect();
    let next = upcoming(this, now, &overlays);
    ui::pane()
        .id("layers")
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(10.))
        .py(px(14.))
        .overflow_y_scroll()
        .child(
            label(format!("On screen at {}", timecode(this.playhead)))
                .px(px(10.))
                .pb(px(8.)),
        )
        .children(now.map(|s| {
            layer(
                "layout",
                MUTED.into(),
                "Layout",
                div().child(layout_name(s.layout)),
            )
        }))
        .child(layer(
            "text",
            MUTED.into(),
            "Captions",
            div().child(captions),
        ))
        .children(zoom.map(|z| {
            layer(
                "zoom",
                ZOOM.into(),
                "Zoom",
                div().child(format!("{:.0}%", z * 100.0)),
            )
        }))
        .children(showing.into_iter().map(|o| {
            let length = (o.frames.end - o.frames.start) as f32 / FPS as f32;
            layer("picture", o.color, &o.text, mono(format!("{length:.1} s")))
        }))
        .child(div().flex_none().h(px(14.)))
        .child(label("Next up").px(px(10.)).pb(px(8.)))
        .when(next.is_empty(), |pane| {
            pane.child(
                div()
                    .px(px(10.))
                    .pb(px(6.))
                    .text_size(TEXT_SM)
                    .text_color(FAINT)
                    .child("Nothing changes after the playhead"),
            )
        })
        .children(next.into_iter().enumerate().map(|(index, next)| {
            let frame = next.frame;
            layer(next.icon, next.color, &next.name, mono(timecode(frame)))
                .id(("next-up", index))
                .cursor_pointer()
                .hover(|s| s.bg(RAISED).text_color(TEXT))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_playing(false);
                    this.seek(frame);
                    cx.notify();
                }))
        }))
        .child(div().flex_none().h(px(14.)))
        .child(label("Place at the playhead").px(px(10.)).pb(px(8.)))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .px(px(6.))
                .child(
                    tool_button("place-callout", "cursor", "Callout", this.place.callout)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_callout(cx))),
                )
                .child(
                    tool_button(
                        "place-pop",
                        "spark",
                        "Pop",
                        this.place.picker == Some(Picker::Pop),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_picker(Picker::Pop, cx))),
                )
                .child(
                    tool_button(
                        "place-sound",
                        "system-audio",
                        "Sound",
                        this.place.picker == Some(Picker::Sound),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_picker(Picker::Sound, cx))),
                ),
        )
        .child(
            div()
                .flex_none()
                .pt(px(8.))
                .map(|panel| match this.place.picker {
                    Some(Picker::Pop) => panel.child(place::pop_picker(this, cx)),
                    Some(Picker::Sound) => panel.child(place::sound_picker(this, cx)),
                    None => panel.children(place::note(this).map(|n| n.px(px(10.)))),
                }),
        )
}

fn layer(icon_name: &'static str, color: Hsla, name: &str, value: Div) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(9.))
        .rounded(RADIUS_SM)
        .text_size(px(14.))
        .child(icon(icon_name, ICON, color))
        .child(div().flex_1().min_w_0().truncate().child(name.to_owned()))
        .child(value.flex_none().text_size(TEXT_SM).text_color(MUTED))
}

/// An overlay or highlight of the edit, with its output frames and its lane's color.
struct Overlay {
    text: String,
    frames: std::ops::Range<u64>,
    color: Hsla,
}

fn overlays(this: &EditScreen, cx: &Context<EditScreen>) -> Vec<Overlay> {
    [false, true]
        .into_iter()
        .flat_map(|highlight| {
            let color = super::overlays::lane_colors(highlight).1;
            super::overlays::lane_items(this, highlight, cx)
                .into_iter()
                .map(move |item| Overlay {
                    text: item.text,
                    frames: item.frames,
                    color,
                })
        })
        .collect()
}

/// A change after the playhead, which a click seeks to.
struct Next {
    icon: &'static str,
    color: Hsla,
    name: String,
    frame: u64,
}

/// The next layout change, zoom and overlay after the playhead, in the order they come.
fn upcoming(this: &EditScreen, now: Option<&Span>, overlays: &[Overlay]) -> Vec<Next> {
    let mut out = Vec::new();
    if let Some(span) = this
        .map
        .spans
        .iter()
        .filter(|s| s.output_frame > this.playhead)
        .find(|s| now.is_none_or(|n| n.layout != s.layout))
    {
        out.push(Next {
            icon: "layout",
            color: MUTED.into(),
            name: layout_name(span.layout).to_owned(),
            frame: span.output_frame,
        });
    }
    let zoom = this
        .map
        .spans
        .iter()
        .filter(|s| s.output_frames().end > this.playhead)
        .flat_map(|s| {
            s.zooms.iter().filter_map(move |z| {
                let start = shot_render::plan::snap(z.range, FPS).start;
                (s.source_frames.contains(&start))
                    .then(|| (z.scale, s.output_frame + (start - s.source_frames.start)))
            })
        })
        .find(|(_, frame)| *frame > this.playhead);
    if let Some((scale, frame)) = zoom {
        out.push(Next {
            icon: "zoom",
            color: ZOOM.into(),
            name: format!("Zoom {scale}%"),
            frame,
        });
    }
    if let Some(overlay) = overlays
        .iter()
        .filter(|o| o.frames.start > this.playhead)
        .min_by_key(|o| o.frames.start)
    {
        out.push(Next {
            icon: "picture",
            color: overlay.color,
            name: overlay.text.clone(),
            frame: overlay.frames.start,
        });
    }
    out.sort_by_key(|next| next.frame);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_fills_the_stage_by_width_or_by_height() {
        let stage = |w: f32, h: f32| preview_width(Size::new(px(w), px(h)));
        assert_eq!(stage(1088., 900.), px(1040.), "wide enough at 1600x1000");
        let tall_enough = 2. * STAGE_PADDING + TRANSPORT + LINES + 585.;
        assert_eq!(
            stage(2000., tall_enough),
            px(1040.),
            "short: the height decides"
        );
        assert_eq!(stage(10., 10.), px(0.));
    }
}
