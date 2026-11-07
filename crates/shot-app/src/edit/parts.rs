//! Pieces every workspace shares: the preview with its transport, the whole-video overview bar,
//! the workspace switcher and export in the title bar.

use gpui::{
    BorderStyle, Bounds, Context, Div, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    ObjectFit, Pixels, canvas, div, fill, point, prelude::*, px, quad, size, surface,
};
use gpui_component::Disableable;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::progress::Progress;

use super::{EditScreen, ExportStatus, Workspace};
use crate::model::{View, fraction, frame_at, timecode, timecode_fine};
use crate::theme::*;
use crate::ui::{self, Segment, icon, mono, segmented};

/// The preview frame at a 16:9 aspect, black until the first frame arrives.
pub fn preview(this: &EditScreen) -> Div {
    div()
        .w_full()
        .aspect_ratio(16. / 9.)
        .flex_none()
        .rounded(px(6.))
        .overflow_hidden()
        .bg(gpui::black())
        .shadow_lg()
        .children(this.shown.as_ref().map(|frame| {
            surface(frame.buffer.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
        }))
}

/// Play, the playhead's timecode and the Edited/Untrimmed switch, under every preview.
pub fn transport(this: &EditScreen, total: bool, cx: &mut Context<EditScreen>) -> Div {
    let playing = this.playing;
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(2.))
        .child(
            Button::new("play")
                .icon(
                    gpui_component::Icon::empty().path(crate::theme::icon_path(if playing {
                        "pause"
                    } else {
                        "play"
                    })),
                )
                .accessibility_label(if playing { "Pause" } else { "Play" })
                .w(px(34.))
                .h(px(32.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_playing(!this.playing);
                    cx.notify();
                })),
        )
        .child(
            div()
                .flex()
                .gap(px(6.))
                .child(mono(timecode_fine(this.playhead)))
                .when(total, |row| {
                    row.child(mono(format!("/ {}", timecode(this.map.frames))).text_color(MUTED))
                }),
        )
        .child(div().flex_1())
        .child(segmented(
            "view",
            WELL,
            vec![
                Segment::new(
                    "Edited",
                    this.view == View::Edited,
                    cx.listener(|this, _, _, cx| this.set_view(View::Edited, cx)),
                ),
                Segment::new(
                    "Untrimmed",
                    this.view == View::Untrimmed,
                    cx.listener(|this, _, _, cx| this.set_view(View::Untrimmed, cx)),
                ),
            ],
        ))
}

/// The whole video as one bar: chapter blocks, cut ticks, a frame around `window` and the
/// playhead. Clicking or dragging seeks.
pub fn overview(
    this: &EditScreen,
    window_frames: std::ops::Range<u64>,
    height: Pixels,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let total = this.map.frames;
    let chapters: Vec<(f32, f32, bool)> = this
        .map
        .chapters
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.frames.is_empty())
        .map(|(index, c)| {
            (
                fraction(c.frames.start, total),
                fraction(c.frames.end, total),
                index == this.chapter,
            )
        })
        .collect();
    let ticks: Vec<f32> = this
        .map
        .cut_frames()
        .into_iter()
        .map(|f| fraction(f, total))
        .collect();
    let view = (
        fraction(window_frames.start, total),
        fraction(window_frames.end, total),
    );
    let playhead = fraction(this.playhead, total);
    let bounds = this.overview_bounds.clone();
    div()
        .id("overview")
        .relative()
        .flex_1()
        .h(height)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.seek_overview(event.position.x, cx)
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            if event.pressed_button == Some(MouseButton::Left) {
                this.seek_overview(event.position.x, cx);
            }
        }))
        .child(
            canvas(
                move |b, _, _| bounds.set(b),
                move |b, (), window, _| {
                    let x = |f: f32| b.origin.x + b.size.width * f;
                    let lane_top = b.origin.y + (b.size.height - px(12.)) / 2.;
                    for (from, to, on) in &chapters {
                        let left = x(*from);
                        let width = (x(*to) - left - px(3.)).max(px(2.));
                        window.paint_quad(
                            fill(
                                Bounds::new(point(left, lane_top), size(width, px(12.))),
                                if *on { BLOCK_ON } else { KBD },
                            )
                            .corner_radii(px(4.)),
                        );
                    }
                    for tick in &ticks {
                        window.paint_quad(fill(
                            Bounds::new(point(x(*tick), lane_top), size(px(1.), px(12.))),
                            Hsla::from(CUT).opacity(0.7),
                        ));
                    }
                    let left = x(view.0);
                    let width = (x(view.1) - left).max(px(6.));
                    window.paint_quad(quad(
                        Bounds::new(
                            point(left, b.origin.y + px(3.)),
                            size(width, b.size.height - px(6.)),
                        ),
                        px(6.),
                        Hsla::from(TEXT).opacity(0.06),
                        px(1.5),
                        TEXT,
                        BorderStyle::Solid,
                    ));
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(x(playhead) - px(1.), b.origin.y),
                                size(px(2.), b.size.height),
                            ),
                            ACCENT,
                        )
                        .corner_radii(px(1.)),
                    );
                },
            )
            .size_full(),
        )
}

/// The footer of Text and Picture: playhead and length, and the overview bar.
pub fn overview_footer(
    this: &EditScreen,
    window_frames: std::ops::Range<u64>,
    cx: &mut Context<EditScreen>,
) -> Div {
    ui::pane()
        .h(px(64.))
        .flex_none()
        .m(px(8.))
        .flex()
        .items_center()
        .gap(px(16.))
        .px(px(16.))
        .child(
            div()
                .w(px(150.))
                .flex()
                .gap(px(6.))
                .child(mono(timecode(this.playhead)))
                .child(mono(format!("/ {}", timecode(this.map.frames))).text_color(MUTED)),
        )
        .child(overview(this, window_frames, px(30.), cx))
}

/// A row of a muted name and its value, for inspectors and lists.
pub fn row(name: impl Into<gpui::SharedString>, value: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(px(14.))
        .child(div().text_color(MUTED).child(name.into()))
        .child(div().flex_1())
        .child(value)
}

/// An error from the last edit or the preview, above the workspace.
pub fn error_strip(this: &EditScreen) -> Option<Div> {
    this.error
        .clone()
        .map(|error| div().px(px(8.)).pb(px(8.)).child(ui::alert(error)))
}

impl EditScreen {
    fn seek_overview(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let bounds = self.overview_bounds.get();
        if bounds.size.width <= px(0.) {
            return;
        }
        let at = (x - bounds.origin.x) / bounds.size.width;
        self.seek(frame_at(at, self.map.frames));
        if let Some(chapter) = self.map.chapter_at(self.playhead) {
            self.chapter = chapter;
        }
        cx.notify();
    }

    /// The title bar's workspace switcher and export.
    pub fn title_controls(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self.workspace;
        let export = self
            .export
            .as_ref()
            .map(|s| match &*s.lock().expect("export status") {
                ExportStatus::Running {
                    rendered,
                    to_render,
                } => div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(
                        div()
                            .w(px(120.))
                            .child(Progress::new("export-progress").value(if *to_render == 0 {
                                0.0
                            } else {
                                *rendered as f32 / *to_render as f32 * 100.0
                            })),
                    )
                    .child(match to_render {
                        0 => "Exporting".to_owned(),
                        n => format!("Exporting {rendered} of {n} segments"),
                    }),
                ExportStatus::Done { path, seconds } => div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(icon("check", ICON, ACCENT))
                    .child(format!("Exported in {seconds:.0} s to"))
                    .child(mono(path.clone()).text_color(TEXT)),
                ExportStatus::Failed(error) => div()
                    .max_w(px(420.))
                    .text_size(TEXT_SM)
                    .text_color(RECORD_TEXT)
                    .child(format!("Export failed: {error}")),
            });
        let running = self.export.as_ref().is_some_and(|s| {
            matches!(
                *s.lock().expect("export status"),
                ExportStatus::Running { .. }
            )
        });
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .children(export)
            .child(segmented(
                "workspace",
                SURFACE,
                vec![
                    Segment::new(
                        "Text",
                        workspace == Workspace::Text,
                        cx.listener(|this, _, _, cx| this.set_workspace(Workspace::Text, cx)),
                    )
                    .icon("text"),
                    Segment::new(
                        "Picture",
                        workspace == Workspace::Picture,
                        cx.listener(|this, _, _, cx| this.set_workspace(Workspace::Picture, cx)),
                    )
                    .icon("picture"),
                    Segment::new(
                        "Timeline",
                        workspace == Workspace::Timeline,
                        cx.listener(|this, _, _, cx| this.set_workspace(Workspace::Timeline, cx)),
                    )
                    .icon("timeline"),
                    Segment::new(
                        "Product",
                        workspace == Workspace::Product,
                        cx.listener(|this, _, _, cx| this.set_workspace(Workspace::Product, cx)),
                    )
                    .icon("cursor"),
                ],
            ))
            .child(
                Button::new("export")
                    .primary()
                    .icon(gpui_component::Icon::empty().path(crate::theme::icon_path("export")))
                    .label("Export")
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| this.start_export("main", cx))),
            )
    }
}

/// Words, cuts and timecodes of the playhead's layout for the inspectors.
pub fn layout_name(layout: shot_core::Layout) -> &'static str {
    match layout {
        shot_core::Layout::Face => "Face",
        shot_core::Layout::FaceContent => "Face + content",
        shot_core::Layout::Split => "Split",
        shot_core::Layout::Content => "Content",
        shot_core::Layout::Screen => "Screen",
        shot_core::Layout::FaceCorner => "Face corner",
        shot_core::Layout::Window => "Window",
    }
}
