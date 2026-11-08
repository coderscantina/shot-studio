//! The picked zoom's framing and ramps in the inspector, and the knob slider they share with the
//! camera's hand-held strength. Every change is a file change with undo; a framing or ramp
//! change replays the ramp it touched, so the preview shows it at once.
//!
//! A knob is a tab stop: ←→ step it, ⇧ steps five times as far, and a drag on its track sets it
//! when the button comes up, so a drag is one change.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, DragMoveEvent, FontWeight, MouseButton, MouseDownEvent, Pixels, Point,
    Render, SharedString, Stateful, Window, actions, div, prelude::*, px, relative,
};
use gpui_component::tooltip::Tooltip;
use shot_core::{Framing, Micros, TakeAt, Zoom};
use shot_render::zoom::ramps;

use super::EditScreen;
use crate::model::FPS;
use crate::theme::*;
use crate::ui::{self, label};

actions!(knob, [StepDown, StepUp, StepDownMore, StepUpMore]);

pub(crate) const KNOB_CONTEXT: &str = "Knob";

/// The longest ramp a slider sets. Ramps longer than the zoom share it.
const MAX_RAMP: u64 = 2_000_000;
const RAMP_STEP: u64 = 50_000;
const HANDHELD_STEP: u64 = 5;
/// How much of the picture plays before and after a ramp when it replays.
const REPLAY_LEAD: Duration = Duration::from_millis(500);
const REPLAY_TAIL: Duration = Duration::from_millis(600);

/// What a knob sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Knob {
    RampIn(TakeAt, usize),
    RampOut(TakeAt, usize),
    /// The camera's hand-held strength in the video's brand layer.
    Handheld,
}

impl Knob {
    /// The largest value and one step, in microseconds for a ramp, as a strength otherwise.
    fn range(self) -> (u64, u64) {
        match self {
            Self::RampIn(..) | Self::RampOut(..) => (MAX_RAMP, RAMP_STEP),
            Self::Handheld => (100, HANDHELD_STEP),
        }
    }
}

/// The knob being dragged and its value so far, and the latest replay.
#[derive(Default)]
pub(super) struct KnobState {
    dragging: Option<(Knob, u64)>,
    replay: u64,
}

/// The drag of a knob's handle. Draws nothing: the handle itself follows the pointer.
struct KnobDrag(Knob);

impl Render for KnobDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Which end of a zoom a change moves, so its replay shows that end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    In,
    Out,
}

impl EditScreen {
    /// Sets the zoom's framing and replays its way in.
    fn set_framing(&mut self, at: TakeAt, index: usize, framing: Framing, cx: &mut Context<Self>) {
        let Some(zoom) = self.zoom(at, index, cx) else {
            return;
        };
        let shape = shot_core::ZoomShape {
            framing,
            ..zoom.shape
        };
        self.edit(cx, |cut| cut.set_zoom_shape(at, index, shape));
        self.replay(at, index, End::In, cx);
    }

    /// Sets or, with `None`, resets a ramp of the zoom, and replays it.
    fn set_ramp(
        &mut self,
        at: TakeAt,
        index: usize,
        end: End,
        ramp: Option<Micros>,
        cx: &mut Context<Self>,
    ) {
        let Some(zoom) = self.zoom(at, index, cx) else {
            return;
        };
        let mut shape = zoom.shape;
        match end {
            End::In => shape.ramp_in = ramp,
            End::Out => shape.ramp_out = ramp,
        }
        if shape != zoom.shape {
            self.edit(cx, |cut| cut.set_zoom_shape(at, index, shape));
        }
        self.replay(at, index, end, cx);
    }

    /// Writes a knob's value.
    fn set_knob(&mut self, knob: Knob, value: u64, cx: &mut Context<Self>) {
        match knob {
            Knob::RampIn(at, index) => self.set_ramp(at, index, End::In, Some(Micros(value)), cx),
            Knob::RampOut(at, index) => self.set_ramp(at, index, End::Out, Some(Micros(value)), cx),
            Knob::Handheld => self.set_handheld(Some(u8::try_from(value).unwrap_or(100)), cx),
        }
    }

    /// Sets the camera's hand-held strength in the video's brand layer, or with `None` leaves
    /// it to the channel's brand.
    pub(super) fn set_handheld(&mut self, strength: Option<u8>, cx: &mut Context<Self>) {
        let result = self.folder.update(cx, |folder, cx| {
            let result = folder.set_handheld(strength);
            cx.notify();
            result
        });
        self.wrote(result, cx);
    }

    /// A knob's value as the files have it.
    fn knob_value(&self, knob: Knob, cx: &App) -> u64 {
        let folder = self.folder.read(cx);
        let motion = folder.project.format.effects().zoom_motion;
        let ramp = |at, index, end| {
            self.zoom(at, index, cx).map_or(0, |zoom| {
                let (ramp_in, ramp_out) = ramps(&zoom, motion);
                match end {
                    End::In => ramp_in.0,
                    End::Out => ramp_out.0,
                }
            })
        };
        match knob {
            Knob::RampIn(at, index) => ramp(at, index, End::In),
            Knob::RampOut(at, index) => ramp(at, index, End::Out),
            Knob::Handheld => folder
                .theme
                .as_ref()
                .map_or(0, |theme| u64::from(theme.handheld)),
        }
    }

    /// Moves a knob by `steps` steps, onto the nearest step: a ramp the zoom fitted down sits
    /// between steps.
    fn step_knob(&mut self, knob: Knob, steps: i64, cx: &mut Context<Self>) {
        let (max, step) = knob.range();
        let value = self.knob_value(knob, cx) as i64 + steps * step as i64;
        let value = value.clamp(0, max as i64) as u64;
        self.set_knob(knob, ((value + step / 2) / step * step).min(max), cx);
    }

    /// Ends a knob's drag by writing where it was let go.
    fn release_knob(&mut self, knob: Knob, cx: &mut Context<Self>) {
        if let Some((dragged, value)) = self.knobs.dragging
            && dragged == knob
        {
            self.knobs.dragging = None;
            self.set_knob(knob, value, cx);
        }
    }

    /// The value of `knob` while it is dragged.
    pub(super) fn knobs_dragging(&self, knob: Knob) -> Option<u64> {
        self.knobs
            .dragging
            .filter(|(k, _)| *k == knob)
            .map(|(_, v)| v)
    }

    fn zoom(&self, at: TakeAt, index: usize, cx: &App) -> Option<Zoom> {
        let cut = &self.folder.read(cx).project.cut;
        cut.chapters
            .get(at.chapter)?
            .takes
            .get(at.take)?
            .zooms
            .get(index)
            .copied()
    }

    /// Plays the zoom's ramp in or out with a little around it, then pauses.
    fn replay(&mut self, at: TakeAt, index: usize, end: End, cx: &mut Context<Self>) {
        let Some(zoom) = self.zoom(at, index, cx) else {
            return;
        };
        let Some(frames) = self.map.output_range(at, zoom.range) else {
            return;
        };
        let motion = self.folder.read(cx).project.format.effects().zoom_motion;
        let (ramp_in, ramp_out) = ramps(&zoom, motion);
        let to_frames = |d: Duration| d.as_micros() as u64 * u64::from(FPS) / 1_000_000;
        let lead = to_frames(REPLAY_LEAD);
        let ramp = match end {
            End::In => ramp_in,
            End::Out => ramp_out,
        };
        let from = match end {
            End::In => frames.start.saturating_sub(lead),
            End::Out => frames
                .end
                .saturating_sub(to_frames(Duration::from_micros(ramp.0)) + lead),
        };
        // The replay stays inside the zoom's chapter, so Text keeps showing it.
        let chapter = self
            .map
            .chapters
            .get(at.chapter)
            .map_or(0..self.map.frames, |c| c.frames.clone());
        let from = from.max(chapter.start);
        let played = REPLAY_LEAD + Duration::from_micros(ramp.0) + REPLAY_TAIL;
        let until = (from + to_frames(played)).min(chapter.end.saturating_sub(1));
        let length = Duration::from_micros(until.saturating_sub(from) * 1_000_000 / u64::from(FPS));
        self.seek(from);
        self.set_playing(true);
        self.knobs.replay += 1;
        let replay = self.knobs.replay;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(length).await;
            this.update(cx, |this, cx| {
                if this.knobs.replay == replay && this.playing {
                    this.set_playing(false);
                    cx.notify();
                }
            })
        })
        .detach();
        cx.notify();
    }
}

/// The framing picker and the ramp sliders of the picked zoom.
pub(super) fn controls(
    this: &EditScreen,
    at: TakeAt,
    index: usize,
    zoom: Zoom,
    cx: &mut Context<EditScreen>,
) -> gpui::Div {
    let framing = framing_picker(at, index, zoom.shape.framing, cx);
    let ramp_in = ramp_row(this, at, index, zoom, End::In, cx);
    let ramp_out = ramp_row(this, at, index, zoom, End::Out, cx);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(framing)
        .child(ramp_in)
        .child(ramp_out)
        .child(hint())
}

/// One ramp's slider with its value, and Reset when the zoom has a ramp of its own.
fn ramp_row(
    this: &EditScreen,
    at: TakeAt,
    index: usize,
    zoom: Zoom,
    end: End,
    cx: &mut Context<EditScreen>,
) -> gpui::Div {
    let motion = this.folder.read(cx).project.format.effects().zoom_motion;
    {
        let (knob, own, id) = match end {
            End::In => (Knob::RampIn(at, index), zoom.shape.ramp_in, "zoom-ramp-in"),
            End::Out => (
                Knob::RampOut(at, index),
                zoom.shape.ramp_out,
                "zoom-ramp-out",
            ),
        };
        let shown = match this.knobs.dragging {
            Some((k, value)) if k == knob => value,
            _ => this.knob_value(knob, cx),
        };
        let text = if shown == 0 {
            "Cut".to_owned()
        } else {
            format!("{:.2} s", shown as f64 / 1e6)
        };
        knob_row(
            match end {
                End::In => "Ramp in",
                End::Out => "Ramp out",
            },
            slider(this, knob, id, shown, cx),
            text,
            own.map(|_| {
                div()
                    .id((id, 1usize))
                    .px(px(6.))
                    .rounded(px(6.))
                    .text_size(TEXT_XS)
                    .text_color(MUTED)
                    .cursor_pointer()
                    .hover(|s| s.bg(FLOAT).text_color(TEXT))
                    .map(ui::keyboard_reachable)
                    .child("Reset")
                    .tooltip(move |window, cx| {
                        Tooltip::new(match motion {
                            shot_core::ZoomMotion::Instant => "Back to the format's instant zoom",
                            shot_core::ZoomMotion::Smooth => "Back to the format's smooth ramp",
                        })
                        .build(window, cx)
                    })
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_ramp(at, index, end, None, cx)),
                    )
            }),
        )
    }
}

/// The name of a framing in the inspector.
fn framing_name(framing: Framing) -> &'static str {
    match framing {
        Framing::Auto => "Auto",
        Framing::Center => "Center",
        Framing::TopLeft => "Top left",
        Framing::TopRight => "Top right",
        Framing::BottomLeft => "Bottom left",
        Framing::BottomRight => "Bottom right",
    }
}

/// A 16:9 frame split into its quarters with the center over them, and Auto beside it. A
/// quarter fills the frame at 200 percent or more.
fn framing_picker(
    at: TakeAt,
    index: usize,
    current: Framing,
    cx: &mut Context<EditScreen>,
) -> gpui::Div {
    let cell = |framing: Framing, id: usize| {
        let on = framing == current;
        div()
            .id(("zoom-framing", id))
            .absolute()
            .rounded(px(5.))
            .cursor_pointer()
            .map(ui::keyboard_reachable)
            .when(on, |c| c.bg(ZOOM_TINT).border_color(ZOOM))
            .when(!on, |c| c.hover(|s| s.bg(FLOAT_ON)))
            .tooltip(move |window, cx| Tooltip::new(framing_name(framing)).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| this.set_framing(at, index, framing, cx)))
    };
    let quarter = |framing: Framing, id: usize| {
        let [x, y] = framing.quarter().expect("a quarter");
        cell(framing, id)
            .left(relative(x - 0.25))
            .top(relative(y - 0.25))
            .w(relative(0.5))
            .h(relative(0.5))
    };
    let frame = div()
        .relative()
        .w(px(168.))
        .h(px(94.5))
        .flex_none()
        .rounded(px(7.))
        .bg(WELL)
        .border_1()
        .border_color(KBD)
        .p(px(3.))
        .child(
            div()
                .relative()
                .size_full()
                .child(quarter(Framing::TopLeft, 2))
                .child(quarter(Framing::TopRight, 3))
                .child(quarter(Framing::BottomLeft, 4))
                .child(quarter(Framing::BottomRight, 5))
                .child(
                    div()
                        .absolute()
                        .left(relative(0.5))
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .bg(KBD),
                )
                .child(
                    div()
                        .absolute()
                        .top(relative(0.5))
                        .left_0()
                        .right_0()
                        .h(px(1.))
                        .bg(KBD),
                )
                .child(
                    // Over the quarters, so it takes the click alone.
                    cell(Framing::Center, 1)
                        .occlude()
                        .left(relative(0.32))
                        .top(relative(0.3))
                        .w(relative(0.36))
                        .h(relative(0.4))
                        .when(current != Framing::Center, |c| c.bg(RAISED))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(5.)).rounded_full().bg(MUTED)),
                ),
        );
    let auto_on = current == Framing::Auto;
    let auto = div()
        .id(("zoom-framing", 0usize))
        .flex()
        .flex_col()
        .gap(px(4.))
        .p(px(8.))
        .rounded(RADIUS_SM)
        .cursor_pointer()
        .map(ui::keyboard_reachable)
        .bg(if auto_on { ZOOM_TINT } else { FLOAT })
        .when(auto_on, |a| a.border_color(ZOOM))
        .when(!auto_on, |a| a.hover(|s| s.bg(FLOAT_ON)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(TEXT_SM)
                .font_weight(FontWeight::MEDIUM)
                .text_color(if auto_on { STRONG } else { TEXT })
                .child(ui::icon(
                    "cursor",
                    ICON_SM,
                    if auto_on { ZOOM_TEXT } else { MUTED },
                ))
                .child("Auto"),
        )
        .child(
            div()
                .text_size(TEXT_XS)
                .text_color(MUTED)
                .child("The point, or the layout's center"),
        )
        .on_click(
            cx.listener(move |this, _, _, cx| this.set_framing(at, index, Framing::Auto, cx)),
        );
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .text_size(px(14.))
                .child(div().text_color(MUTED).child("Framing"))
                .child(div().flex_1())
                .child(div().text_color(TEXT).child(framing_name(current))),
        )
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(frame)
                .child(div().flex_1().min_w_0().child(auto)),
        )
}

/// A knob's name, slider, value and an optional action after it.
pub(super) fn knob_row(
    name: &'static str,
    slider: Stateful<gpui::Div>,
    value: String,
    after: Option<Stateful<gpui::Div>>,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(14.))
                .child(div().text_color(MUTED).child(name))
                .child(div().flex_1())
                .children(after)
                .child(
                    div()
                        .min_w(px(44.))
                        .flex()
                        .justify_end()
                        .text_color(TEXT)
                        .child(value),
                ),
        )
        .child(slider)
}

/// A horizontal slider for `knob` showing `value`: click or drag the track, or focus it and
/// step it with ←→.
pub(super) fn slider(
    this: &EditScreen,
    knob: Knob,
    id: &'static str,
    value: u64,
    cx: &mut Context<EditScreen>,
) -> Stateful<gpui::Div> {
    let (max, step) = knob.range();
    let at = value.min(max) as f32 / max as f32;
    let active = this.knobs.dragging.is_some_and(|(k, _)| k == knob);
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
    let value_at = move |bounds: Bounds<Pixels>, position: Point<Pixels>| {
        let width = f32::from(bounds.size.width).max(1.);
        let at = (f32::from(position.x - bounds.origin.x) / width).clamp(0., 1.);
        ((at * max as f32 / step as f32).round() as u64 * step).min(max)
    };
    let measured = bounds.clone();
    div()
        .id(id)
        .key_context(KNOB_CONTEXT)
        .relative()
        .h(px(22.))
        .rounded(px(6.))
        .cursor_pointer()
        .map(ui::keyboard_reachable)
        .child(ui::measure(bounds).absolute().size_full())
        .child(
            div()
                .absolute()
                .left(px(4.))
                .right(px(4.))
                .top(px(8.))
                .h(px(4.))
                .rounded(px(2.))
                .bg(KBD)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(relative(at))
                        .rounded(px(2.))
                        .bg(ZOOM),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(-5.))
                        .left(relative(at))
                        .ml(px(-7.))
                        .size(px(14.))
                        .rounded_full()
                        .bg(if active { STRONG } else { TEXT })
                        .shadow_sm(),
                ),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.knobs.dragging = Some((knob, value_at(measured.get(), event.position)));
                cx.notify();
            }),
        )
        .on_drag(KnobDrag(knob), |drag, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| KnobDrag(drag.0))
        })
        .on_drag_move(
            cx.listener(move |this, event: &DragMoveEvent<KnobDrag>, _, cx| {
                if event.drag(cx).0 == knob {
                    this.knobs.dragging =
                        Some((knob, value_at(event.bounds, event.event.position)));
                    cx.notify();
                }
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| this.release_knob(knob, cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| this.release_knob(knob, cx)),
        )
        .on_action(cx.listener(move |this, _: &StepDown, _, cx| this.step_knob(knob, -1, cx)))
        .on_action(cx.listener(move |this, _: &StepUp, _, cx| this.step_knob(knob, 1, cx)))
        .on_action(cx.listener(move |this, _: &StepDownMore, _, cx| this.step_knob(knob, -5, cx)))
        .on_action(cx.listener(move |this, _: &StepUpMore, _, cx| this.step_knob(knob, 5, cx)))
        .tooltip(move |window, cx| {
            Tooltip::new(SharedString::from("Drag, or ←→ to step")).build(window, cx)
        })
}

/// The hint under the zoom's controls.
fn hint() -> gpui::Div {
    label("Pick a part of the frame to zoom on. A ramp of 0 cuts in or out")
}
