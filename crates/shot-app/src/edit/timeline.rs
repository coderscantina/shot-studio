//! The Timeline workspace: preview, change list and overlays on top, the whole-video overview,
//! then one lane per kind: Layout (with the intro and outro), Zoom, Overlays, Highlight, Words,
//! Voice, Sound and Music, with export's transitions as marks on their edges.
//!
//! The lanes are the editing surface. Click or drag the ruler or an empty lane to move the
//! playhead, drag across the lanes to select words, click a zoom or overlay to pick it, drag its
//! edges to trim it or its body to move it, click a cut to pick it for restoring. `=` and `-`
//! zoom the lanes, scrolling pans them, ⌘-scroll and pinch zoom around the pointer.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Bounds, Context, Div, FontWeight, Hsla, MouseButton, MouseDownEvent, PinchEvent, Pixels,
    ScrollEvent, Stateful, Window, canvas, div, fill, point, prelude::*, px, size,
};
use gpui_component::tooltip::Tooltip;
use shot_core::{Applied, Placed, RemovedBy, TakeAt, TimeRange, WordState};
use shot_render::join::BookendKind;
use shot_render::overlay::highlight;
use shot_render::plan::frame_time;

use super::EditScreen;
use super::parts::{self, layout_name};
use crate::model::{self, FPS, Selection, changes, seconds, timecode};
use crate::peaks::Peaks;
use crate::theme::*;
use crate::ui::{self, hint, icon, kbd, label, mono};

const NAME_WIDTH: f32 = 112.;
/// Average width of a character at 13 px, for packing words into rows.
const WORD_CHAR: f32 = 7.2;
/// The lanes pane at its natural height, so the top row gives way on short windows.
const LANES_HEIGHT: f32 = 516.;
/// The title bar above the screen.
const TITLE_BAR: f32 = 46.;
/// The fewest and most output frames the lanes show: 2 s up to the whole video.
const MIN_SPAN: u64 = 2 * FPS as u64;
const DEFAULT_SPAN: u64 = 15 * FPS as u64;
/// How far a dragged edge jumps to the playhead or a word edge, in pixels.
const SNAP: f32 = 8.;
/// The shortest zoom or overlay a drag leaves, in frames.
const MIN_LENGTH: u64 = 6;
/// A zoom's scale when the format has no auto-zoom rule to take it from.
const DEFAULT_ZOOM: u16 = 130;

/// What the Timeline lanes show and what the mouse does in them.
pub struct Lanes {
    /// Output frames the lanes show, and the first of them.
    span: u64,
    start: u64,
    /// The playhead the view last followed. A pan holds until the playhead moves.
    followed: Option<u64>,
    /// Where the tracks were last painted, for mouse mapping.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    /// The block a click picked, which Backspace removes or R restores.
    pub picked: Option<Pick>,
}

impl Default for Lanes {
    fn default() -> Self {
        Self {
            span: DEFAULT_SPAN,
            start: 0,
            followed: None,
            bounds: Rc::default(),
            drag: None,
            picked: None,
        }
    }
}

/// A block picked in the lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// A person's zoom or overlay in `cut.kdl`.
    Placed(TakeAt, Placed),
    /// A run of removed media.
    Cut(TakeAt, TimeRange),
}

enum Drag {
    /// The ruler moves the playhead.
    Scrub,
    /// Words from the frame the drag started at.
    Select { from: u64 },
    /// A zoom's or overlay's edge or body, with its output frames as they were and as dragged.
    Shift {
        at: TakeAt,
        placed: Placed,
        grip: Grip,
        grab: u64,
        was: Range<u64>,
        now: Range<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grip {
    Start,
    End,
    Body,
}

/// A zoom or an overlay on its lane.
pub struct Item {
    pub at: TakeAt,
    /// Its index in `cut.kdl`, `None` for the lock's, which the lanes only show.
    pub placed: Option<Placed>,
    pub frames: Range<u64>,
    pub text: String,
}

impl Lanes {
    fn window(&self, total: u64) -> Range<u64> {
        let span = self.span.min(total).max(1);
        let start = self.start.min(total.saturating_sub(span));
        start..start + span
    }

    /// Pages the view to the playhead when the playhead moved out of it.
    fn follow(&mut self, playhead: u64, total: u64) {
        self.span = self.span.clamp(MIN_SPAN, total.max(MIN_SPAN));
        if self.followed == Some(playhead) {
            return;
        }
        self.followed = Some(playhead);
        let window = self.window(total);
        if !window.contains(&playhead) {
            self.start = playhead.saturating_sub(self.span / 5);
        }
    }

    /// Output frames per pixel of track.
    fn per_pixel(&self, total: u64) -> f32 {
        let width = f32::from(self.bounds.get().size.width).max(1.);
        let window = self.window(total);
        (window.end - window.start) as f32 / width
    }

    /// The output frame under `x`.
    fn frame_at(&self, x: Pixels, total: u64) -> u64 {
        let bounds = self.bounds.get();
        let window = self.window(total);
        let along = f32::from(x - bounds.origin.x).max(0.);
        (window.start + (along * self.per_pixel(total)) as u64).min(total.saturating_sub(1))
    }
}

impl EditScreen {
    fn lane_window(&self) -> Range<u64> {
        self.lanes.window(self.map.frames)
    }

    /// Shows `factor` times as many frames, keeping `anchor` (or the playhead) where it is.
    pub(super) fn zoom_lanes(&mut self, factor: f32, anchor: Option<u64>, cx: &mut Context<Self>) {
        let window = self.lane_window();
        let length = (window.end - window.start).max(1);
        let anchor = anchor
            .or_else(|| window.contains(&self.playhead).then_some(self.playhead))
            .unwrap_or(window.start + length / 2);
        let span = ((self.lanes.span as f32 * factor) as u64)
            .clamp(MIN_SPAN, self.map.frames.max(MIN_SPAN));
        let before = (anchor - window.start) as f32 / length as f32;
        self.lanes.start = anchor.saturating_sub((before * span as f32) as u64);
        self.lanes.span = span;
        cx.notify();
    }

    pub(super) fn fit_lanes(&mut self, cx: &mut Context<Self>) {
        self.lanes.start = 0;
        self.lanes.span = self.map.frames.max(MIN_SPAN);
        cx.notify();
    }

    fn pan_lanes(&mut self, pixels: f32, cx: &mut Context<Self>) {
        let total = self.map.frames;
        let frames = (pixels * self.lanes.per_pixel(total)) as i64;
        let window = self.lane_window();
        let last = total.saturating_sub(window.end - window.start);
        self.lanes.start = window.start.saturating_add_signed(-frames).min(last);
        cx.notify();
    }

    fn scroll_lanes(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(16.));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.platform || event.modifiers.control {
            let anchor = self.lanes.frame_at(event.position.x, self.map.frames);
            self.zoom_lanes((-dy / 200.).exp(), Some(anchor), cx);
        } else {
            self.pan_lanes(if dx.abs() >= dy.abs() { dx } else { dy }, cx);
        }
    }

    fn pinch_lanes(&mut self, event: &PinchEvent, cx: &mut Context<Self>) {
        let anchor = self.lanes.frame_at(event.position.x, self.map.frames);
        self.zoom_lanes(1. / (1. + event.delta).max(0.1), Some(anchor), cx);
    }

    /// Follows the mouse while a lane drag runs.
    pub(super) fn drag_lanes(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let total = self.map.frames;
        let frame = self.lanes.frame_at(x, total);
        let threshold = (SNAP * self.lanes.per_pixel(total)) as u64;
        match self.lanes.drag {
            None => return,
            Some(Drag::Scrub) => self.seek(frame),
            Some(Drag::Select { from }) => {
                if frame.abs_diff(from) > threshold / 2 {
                    self.select_frames(from, frame);
                }
            }
            Some(Drag::Shift {
                at,
                grip,
                grab,
                ref was,
                ..
            }) => {
                let Some(block) = self.block_frames(at) else {
                    return;
                };
                let delta = frame as i64 - grab as i64;
                let moved = |f: u64| f.saturating_add_signed(delta);
                let snap = |f: u64| self.snap(at, f, threshold);
                let now = match grip {
                    Grip::Start => {
                        let start = snap(moved(was.start))
                            .clamp(block.start, was.end.saturating_sub(MIN_LENGTH));
                        start..was.end
                    }
                    Grip::End => {
                        let end = snap(moved(was.end)).clamp(was.start + MIN_LENGTH, block.end);
                        was.start..end
                    }
                    Grip::Body => {
                        let length = was.end - was.start;
                        let start = snap(moved(was.start))
                            .clamp(block.start, block.end.saturating_sub(length));
                        start..start + length
                    }
                };
                if let Some(Drag::Shift { now: shown, .. }) = &mut self.lanes.drag {
                    *shown = now;
                }
            }
        }
        cx.notify();
    }

    /// Writes a moved or trimmed zoom or overlay when its drag ends.
    pub(super) fn end_lane_drag(&mut self, cx: &mut Context<Self>) {
        let Some(Drag::Shift {
            at,
            placed,
            was,
            now,
            ..
        }) = self.lanes.drag.take()
        else {
            return;
        };
        if now == was {
            return;
        }
        let Some(range) = self.map.source_range(at, now) else {
            return;
        };
        self.edit(cx, |cut| cut.set_range(at, placed, range));
        // Zooms are kept in time order, so the moved one may have a new index.
        if let Placed::Zoom(_) = placed {
            let cut = &self.folder.read(cx).project.cut;
            self.lanes.picked = cut
                .chapters
                .get(at.chapter)
                .and_then(|c| c.takes.get(at.take))
                .and_then(|b| b.zooms.iter().position(|z| z.range == range))
                .map(|index| Pick::Placed(at, Placed::Zoom(index)));
        }
    }

    /// Removes the picked zoom or overlay. False when none is picked.
    pub(super) fn remove_picked(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(Pick::Placed(at, placed)) = self.lanes.picked else {
            return false;
        };
        self.lanes.picked = None;
        self.edit(cx, |cut| cut.remove_placed(at, placed));
        true
    }

    /// Adds a zoom over the selected words, at the format's auto-zoom scale.
    pub(super) fn add_zoom(&mut self, cx: &mut Context<Self>) {
        let Some((at, range, ..)) = self.selected() else {
            return;
        };
        let scale = self
            .folder
            .read(cx)
            .project
            .format
            .auto_zoom
            .as_ref()
            .map_or(DEFAULT_ZOOM, |z| z.scale);
        self.selection = None;
        self.edit(cx, |cut| cut.add_zoom(at, range, scale));
        let cut = &self.folder.read(cx).project.cut;
        self.lanes.picked = cut
            .chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .and_then(|b| b.zooms.iter().position(|z| z.range == range))
            .map(|index| Pick::Placed(at, Placed::Zoom(index)));
    }

    /// Selects the words of the block under `from` that start or end between the two frames.
    fn select_frames(&mut self, from: u64, to: u64) {
        let (low, high) = (from.min(to), from.max(to));
        self.selection = self.map.source_at(from).and_then(|(at, _)| {
            let block = self.block(at)?;
            let mut inside = block.words.iter().enumerate().filter(|(_, w)| {
                w.output
                    .is_some_and(|start| start + word_frames(w.range) > low && start < high)
            });
            let first = inside.next()?.0;
            let last = inside.next_back().map_or(first, |(index, _)| index);
            Some(if to >= from {
                Selection {
                    at,
                    anchor: first,
                    head: last,
                }
            } else {
                Selection {
                    at,
                    anchor: last,
                    head: first,
                }
            })
        });
    }

    /// The output frames of every span of the block at `at`.
    fn block_frames(&self, at: TakeAt) -> Option<Range<u64>> {
        self.map
            .spans
            .iter()
            .filter(|s| s.at == at)
            .map(|s| s.output_frames())
            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
    }

    /// `frame`, or the playhead or a word edge of the block at `at` within `threshold`.
    fn snap(&self, at: TakeAt, frame: u64, threshold: u64) -> u64 {
        let edges = self
            .block(at)
            .into_iter()
            .flat_map(|b| &b.words)
            .filter_map(|w| w.output.map(|start| [start, start + word_frames(w.range)]))
            .flatten();
        std::iter::once(self.playhead)
            .chain(edges)
            .filter(|edge| edge.abs_diff(frame) <= threshold)
            .min_by_key(|edge| edge.abs_diff(frame))
            .unwrap_or(frame)
    }

    /// Starts a drag of the item's `grip` and picks the item.
    fn grab(&mut self, item: (TakeAt, Placed, Range<u64>), grip: Grip, x: Pixels) {
        let (at, placed, frames) = item;
        self.selection = None;
        self.lanes.picked = Some(Pick::Placed(at, placed));
        self.lanes.drag = Some(Drag::Shift {
            at,
            placed,
            grip,
            grab: self.lanes.frame_at(x, self.map.frames),
            was: frames.clone(),
            now: frames,
        });
    }
}

fn word_frames(range: TimeRange) -> u64 {
    range.duration().0 * u64::from(FPS) / 1_000_000
}

pub fn render(this: &mut EditScreen, win: &mut Window, cx: &mut Context<EditScreen>) -> Div {
    this.lanes.follow(this.playhead, this.map.frames);
    let window = this.lane_window();
    // The top row gives way first on short windows; the preview keeps 16:9 inside it.
    let top = (f32::from(win.viewport_size().height) - TITLE_BAR - LANES_HEIGHT).clamp(220., 420.);
    let stage = (top - 24. - 42.) * 16. / 9. + 24.;
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(parts::error_strip(this))
        .child(
            div()
                .h(px(top))
                .flex_none()
                .flex()
                .gap(px(8.))
                .mx(px(8.))
                .child(
                    ui::pane()
                        .w(px(stage))
                        .flex_none()
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(parts::preview(this))
                        .child(parts::transport(this, stage >= 440., cx)),
                )
                .child(change_list(this, cx))
                .child(super::overlays::pane(this, cx)),
        )
        .child(lanes(this, window, cx))
}

fn change_list(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let folder = this.folder.read(cx);
    let changes = changes(&folder.shown.cut);
    let raw: u64 = model::blocks(&folder.shown.cut)
        .filter_map(|(_, edit)| {
            Some(edit.ranges.last()?.range().to().0 - edit.ranges.first()?.range().from().0)
        })
        .sum();
    let line =
        |name: &'static str, color: gpui::Rgba, text: String, count: usize, delta: String| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(10.))
                .py(px(8.))
                .rounded(RADIUS_SM)
                .child(icon(name, ICON, color))
                .child(div().flex_1().child(text))
                .child(mono(count.to_string()).text_size(px(14.)).text_color(MUTED))
                .child(
                    mono(delta)
                        .w(px(64.))
                        .text_size(px(14.))
                        .flex()
                        .justify_end(),
                )
        };
    ui::pane()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(12.))
        .py(px(16.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .px(px(10.))
                .pb(px(10.))
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Changes"),
                )
                .child(div().text_color(MUTED).child(format!(
                    "against the raw takes, {}",
                    timecode(raw * u64::from(FPS) / 1_000_000)
                ))),
        )
        .child(
            div()
                .id("change-lines")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(changes.removed.iter().map(|(by, count, length)| {
                    let name = match by {
                        RemovedBy::Manual => "Your cuts".to_owned(),
                        RemovedBy::Rule(rule) => format!("{} cuts", capitalized(rule.as_str())),
                    };
                    line(
                        "cut",
                        CUT,
                        name,
                        *count,
                        format!("−{}", timecode(length.0 * u64::from(FPS) / 1_000_000)),
                    )
                }))
                .child(line(
                    "zoom",
                    ZOOM,
                    "Punch-in zooms".into(),
                    changes.zooms,
                    String::new(),
                ))
                .child(line(
                    "layout",
                    TEXT,
                    "Layout changes".into(),
                    changes.layouts,
                    String::new(),
                )),
        )
        .children(this.active_card().map(|card| {
            div()
                .flex_none()
                .pt(px(8.))
                .child(super::proposals::card_element(this, card, true, cx))
        }))
}

fn capitalized(text: &str) -> String {
    let text = text.replace('-', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Ruler steps in seconds, the first that leaves 90 px between ticks wins.
const STEPS: [u64; 10] = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600];

fn lanes(this: &EditScreen, window: Range<u64>, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let chapter = this
        .map
        .chapter_at(this.playhead)
        .and_then(|c| this.map.chapters.get(c))
        .map(|c| c.title.clone())
        .unwrap_or_default();
    let length = (window.end - window.start).max(1);
    let x = {
        let window = window.clone();
        move |frame: u64| model::fraction(frame.saturating_sub(window.start), length)
    };
    let fps = u64::from(FPS);
    let track = f32::from(this.lanes.bounds.get().size.width).max(1.);
    let step = STEPS
        .into_iter()
        .find(|s| (s * fps) as f32 / length as f32 * track >= 90.)
        .unwrap_or(600)
        * fps;
    let transitions: Vec<(u64, shot_core::TransitionKind)> = this
        .map
        .plan
        .transitions
        .iter()
        .filter(|t| window.contains(&t.edge))
        .map(|t| (t.edge, t.kind))
        .collect();
    // A timecode gives way to a transition's name where the two would overlap.
    let ruler: Vec<(f32, String)> = (window.start.div_ceil(step)..)
        .map(|i| i * step)
        .take_while(|f| *f < window.end)
        .map(|f| (x(f), timecode(f)))
        .filter(|(at, _)| {
            transitions
                .iter()
                .all(|(f, _)| ((x(*f) - at) * track).abs() > 64.)
        })
        .collect();
    let bounds = this.lanes.bounds.clone();
    ui::pane()
        .flex_none()
        .h(px(LANES_HEIGHT - 16.))
        .m(px(8.))
        .pt(px(12.))
        .pb(px(10.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(16.))
                .h(px(24.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(chapter))
                .child(
                    mono(format!(
                        "{} to {}",
                        timecode(window.start),
                        timecode(window.end)
                    ))
                    .text_size(TEXT_SM)
                    .text_color(MUTED),
                )
                .child(div().flex_1())
                .child(zoom_controls(length, cx)),
        )
        .child(
            div()
                .flex()
                .ml(px(NAME_WIDTH))
                .mr(px(16.))
                .child(parts::overview(this, window.clone(), px(20.), cx)),
        )
        .child(
            div()
                .id("ruler")
                .relative()
                .h(px(18.))
                .ml(px(NAME_WIDTH))
                .mr(px(16.))
                .cursor_pointer()
                .font_family(MONO)
                .text_size(TEXT_XS)
                .text_color(MUTED)
                .children(ruler.into_iter().map(|(at, text)| {
                    div()
                        .absolute()
                        .left(gpui::relative(at))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .child(div().w(px(1.)).h(px(8.)).bg(FAINT))
                        .child(text)
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.set_playing(false);
                        this.lanes.drag = Some(Drag::Scrub);
                        this.drag_lanes(event.position.x, cx);
                    }),
                ),
        )
        .child(
            div()
                .id("lanes")
                .relative()
                .flex()
                .flex_col()
                .gap(px(4.))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        let bounds = this.lanes.bounds.get();
                        if event.position.x < bounds.origin.x {
                            return;
                        }
                        let frame = this.lanes.frame_at(event.position.x, this.map.frames);
                        this.selection = None;
                        this.lanes.picked = None;
                        this.proposal = None;
                        this.seek(frame);
                        this.lanes.drag = Some(Drag::Select { from: frame });
                        cx.notify();
                    }),
                )
                .on_scroll(
                    cx.listener(|this, event: &ScrollEvent, _, cx| this.scroll_lanes(event, cx)),
                )
                .on_pinch(
                    cx.listener(|this, event: &PinchEvent, _, cx| this.pinch_lanes(event, cx)),
                )
                .child(
                    canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(NAME_WIDTH))
                        .right(px(16.)),
                )
                .child(lane(
                    "layout",
                    "Layout",
                    px(36.),
                    layout_lane(this, &window, &x),
                ))
                .child(lane(
                    "zoom",
                    "Zoom",
                    px(36.),
                    item_lane(
                        this,
                        "zoom-item",
                        zoom_items(this, cx),
                        &window,
                        &x,
                        (ZOOM_TINT.into(), ZOOM_TEXT.into()),
                        cx,
                    ),
                ))
                .child(lane(
                    "picture",
                    "Overlays",
                    px(36.),
                    item_lane(
                        this,
                        "overlay-item",
                        super::overlays::lane_items(this, false, cx),
                        &window,
                        &x,
                        super::overlays::lane_colors(false),
                        cx,
                    ),
                ))
                .child(lane(
                    "edit",
                    "Highlight",
                    px(36.),
                    item_lane(
                        this,
                        "highlight-item",
                        super::overlays::lane_items(this, true, cx),
                        &window,
                        &x,
                        super::overlays::lane_colors(true),
                        cx,
                    ),
                ))
                .child(lane(
                    "text",
                    "Words",
                    px(66.),
                    words_lane(this, &window, &x, track, cx),
                ))
                .child(lane("mic", "Voice", px(56.), voice_lane(this, &window, cx)))
                .child(lane(
                    "system-audio",
                    "Sound",
                    px(28.),
                    sound_lane(this, &window, &x, cx),
                ))
                .child(lane(
                    "music",
                    "Music",
                    px(28.),
                    music_lane(this, &window, &x, cx),
                ))
                .children(selection_band(this, &window, &x))
                .children(transition_marks(transitions, &x))
                .children(playhead_line(this, &window))
                .when(
                    this.place.picker == Some(super::place::Picker::Sound),
                    |stack| {
                        // Above the Sound lane, over the Music lane and the gaps below it.
                        stack.child(
                            super::place::sound_picker(this, cx)
                                .absolute()
                                .bottom(px(28. + 4. + 28. + 6.))
                                .left(px(NAME_WIDTH))
                                .w(px(520.)),
                        )
                    },
                ),
        )
        .child(toolbar(this, cx))
}

fn zoom_controls(length: u64, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let button = |id: &'static str, text: &'static str| {
        div()
            .id(id)
            .h(px(24.))
            .min_w(px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .bg(RAISED)
            .text_size(TEXT_SM)
            .text_color(TEXT)
            .cursor_pointer()
            .hover(|s| s.bg(FLOAT))
            .child(text)
    };
    let span = length as f64 / f64::from(FPS);
    let shown = if span < 60. {
        format!("{span:.0} s")
    } else {
        timecode(length)
    };
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(
            button("lanes-out", "−")
                .tooltip(|window, cx| Tooltip::new("Show more  −").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.zoom_lanes(2.0, None, cx))),
        )
        .child(
            mono(shown)
                .w(px(52.))
                .flex()
                .justify_center()
                .text_size(TEXT_SM)
                .text_color(MUTED),
        )
        .child(
            button("lanes-in", "+")
                .tooltip(|window, cx| Tooltip::new("Show less  =").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.zoom_lanes(0.5, None, cx))),
        )
        .child(
            button("lanes-fit", "Fit")
                .ml(px(4.))
                .tooltip(|window, cx| Tooltip::new("Whole video  ⇧Z").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.fit_lanes(cx))),
        )
}

/// What the selection or the picked block can do, under the lanes.
fn toolbar(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let bar = div()
        .h(px(32.))
        .mx(px(16.))
        .ml(px(NAME_WIDTH))
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(TEXT_SM);
    let what = |text: String| div().pr(px(10.)).text_color(TEXT).child(text);
    if let Some((_, range, kept, removed)) = this.selected() {
        let count = this.selection.map_or(0, |s| s.words().count());
        let words = if count == 1 { "word" } else { "words" };
        let sound_open = this.place.picker == Some(super::place::Picker::Sound);
        return bar
            .child(what(format!(
                "{count} {words}, {}",
                seconds(range.duration())
            )))
            .when(kept, |bar| {
                bar.child(tool("tool-cut", "cut", "Cut", "⌫").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.delete_selection(&super::DeleteSelection, window, cx)
                    },
                )))
            })
            .when(removed, |bar| {
                bar.child(
                    tool("tool-restore", "restore", "Restore", "R").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.restore_selection(&super::RestoreSelection, window, cx)
                        },
                    )),
                )
            })
            .when(kept, |bar| {
                bar.child(
                    tool("tool-zoom", "zoom", "Zoom", "Z")
                        .on_click(cx.listener(|this, _, _, cx| this.add_zoom(cx))),
                )
            })
            .child(
                tool("tool-sound", "system-audio", "Sound", "S")
                    .when(sound_open, |t| t.bg(FLOAT_ON))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_picker(super::place::Picker::Sound, cx)
                    })),
            )
            .child(hint("1–4", "Layout").ml(px(8.)));
    }
    match this.lanes.picked {
        Some(Pick::Placed(at, placed)) => {
            let cut = &this.folder.read(cx).project.cut;
            let block = cut
                .chapters
                .get(at.chapter)
                .and_then(|c| c.takes.get(at.take));
            let text = match placed {
                Placed::Zoom(index) => block
                    .and_then(|b| b.zooms.get(index))
                    .map(|z| format!("Zoom {}%, {}", z.scale, seconds(z.range.duration()))),
                Placed::Overlay(index) => block
                    .and_then(|b| b.overlays.get(index))
                    .map(|o| format!("{}, {}", o.template.as_str(), seconds(o.range.duration()))),
            };
            bar.children(text.map(what))
                .child(
                    tool("tool-remove", "close", "Remove", "⌫").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.remove_picked(cx);
                        },
                    )),
                )
                .child(div().flex_1())
                .child(label("Drag the edges to trim, the middle to move"))
        }
        Some(Pick::Cut(at, range)) => {
            let removed = this
                .block(at)
                .map(|b| {
                    b.words
                        .iter()
                        .filter(|w| w.state != WordState::Kept)
                        .filter(|w| {
                            let middle = (w.range.from().0 + w.range.to().0) / 2;
                            range.from().0 <= middle && middle < range.to().0
                        })
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            bar.child(what(format!("Cut, {}", seconds(range.duration()))))
                .when(!removed.is_empty(), |bar| {
                    bar.child(
                        div()
                            .min_w_0()
                            .max_w(px(520.))
                            .pr(px(10.))
                            .truncate()
                            .text_color(CUT_TEXT)
                            .line_through()
                            .child(removed),
                    )
                })
                .child(
                    tool("tool-restore", "restore", "Restore", "R").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.restore_selection(&super::RestoreSelection, window, cx)
                        },
                    )),
                )
        }
        None => bar
            .gap(px(18.))
            .child(label("Drag across the lanes to select words"))
            .child(hint("⌫", "Cut"))
            .child(hint("Z", "Zoom"))
            .child(hint("S", "Sound"))
            .child(hint("⌘Z", "Undo"))
            .child(hint("= −", "Zoom lanes")),
    }
}

fn tool(
    id: &'static str,
    name: &'static str,
    text: &'static str,
    keys: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(28.))
        .px(px(10.))
        .rounded(RADIUS_SM)
        .bg(RAISED)
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT))
        .child(icon(name, ICON_SM, TEXT))
        .child(text)
        .child(kbd(keys))
}

/// A lime band down every lane over the selected words.
fn selection_band(this: &EditScreen, window: &Range<u64>, x: &impl Fn(u64) -> f32) -> Option<Div> {
    let selection = this.selection?;
    let block = this.block(selection.at)?;
    let words = block.words.get(selection.words())?;
    let from = words.iter().find_map(|w| w.output)?;
    let to = words
        .iter()
        .rev()
        .find_map(|w| w.output.map(|start| start + word_frames(w.range)))?;
    if to <= window.start || from >= window.end {
        return None;
    }
    let (left, right) = (x(from.max(window.start)), x(to.min(window.end)));
    Some(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(NAME_WIDTH))
            .right(px(16.))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(left))
                    .w(gpui::relative((right - left).max(0.001)))
                    .bg(Hsla::from(ACCENT).opacity(0.08))
                    .border_x_1()
                    .border_color(Hsla::from(ACCENT).opacity(0.5)),
            ),
    )
}

/// The moments the lock places in take time, as output frames inside `window`, with what
/// `pick` makes of each instance.
fn lock_moments<T: Clone>(
    this: &EditScreen,
    window: &Range<u64>,
    cx: &Context<EditScreen>,
    pick: impl Fn(&shot_core::Instance) -> Option<T>,
) -> Vec<(u64, T)> {
    let Some(lock) = this.folder.read(cx).lock() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for take in &lock.takes {
        for instance in &take.instances {
            if let Some(value) = pick(instance) {
                let frames = this.map.outputs_of(&take.take, instance.applied.start());
                out.extend(
                    frames
                        .filter(|f| window.contains(f))
                        .map(|f| (f, value.clone())),
                );
            }
        }
    }
    out.sort_by_key(|(frame, _)| *frame);
    out
}

/// Neutral marks for the lock's sound effects, named by their sound. Clicking the lane moves
/// the playhead there and opens the sound picker above it.
fn sound_lane(
    this: &EditScreen,
    window: &Range<u64>,
    x: &impl Fn(u64) -> f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let sounds = lock_moments(this, window, cx, |i| match &i.applied {
        Applied::Sfx { sound, .. } => Some(sound.to_string()),
        _ => None,
    });
    div()
        .id("sound-lane")
        .size_full()
        .cursor_pointer()
        .children(sounds.into_iter().map(|(frame, name)| {
            div()
                .absolute()
                .top(px(4.))
                .bottom(px(4.))
                .left(gpui::relative(x(frame)))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(2.)).h_full().rounded(px(1.)).bg(BODY))
                .child(
                    div()
                        .px(px(6.))
                        .rounded(px(4.))
                        .bg(BLOCK)
                        .text_color(BODY)
                        .text_size(TEXT_XS)
                        .font_weight(FontWeight::MEDIUM)
                        .line_height(px(20.))
                        .whitespace_nowrap()
                        .child(name),
                )
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let frame = this.lanes.frame_at(event.position.x, this.map.frames);
                this.selection = None;
                this.lanes.picked = None;
                this.seek(frame);
                if this.place.picker != Some(super::place::Picker::Sound) {
                    this.toggle_picker(super::place::Picker::Sound, cx);
                }
                cx.notify();
            }),
        )
}

/// The music bed under the whole cut and the lock's music cues, each until the next.
fn music_lane(
    this: &EditScreen,
    window: &Range<u64>,
    x: &impl Fn(u64) -> f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let mut blocks: Vec<(u64, u64, String)> = Vec::new();
    if let Some(bed) = &this.folder.read(cx).project.cut.music {
        blocks.push((
            0,
            this.map.frames,
            format!("{}, ducked under voice", bed.file),
        ));
    }
    let all = lock_moments(this, &(0..this.map.frames), cx, |i| match &i.applied {
        Applied::Music { file, .. } => Some(file.clone()),
        _ => None,
    });
    for (index, (frame, file)) in all.iter().enumerate() {
        let end = all
            .get(index + 1)
            .map_or(this.map.frames, |(next, _)| *next);
        blocks.push((*frame, end, format!("{file}, ducked under voice")));
    }
    div().size_full().children(
        blocks
            .into_iter()
            .filter(|(from, to, _)| *to > window.start && *from < window.end)
            .map(|(from, to, text)| {
                block(
                    x(from.max(window.start)),
                    x(to.min(window.end)) - 0.002,
                    RAISED,
                    MUTED,
                    text,
                )
                .line_height(px(28.))
            }),
    )
}

/// A transition as export places it: a mark down the lanes on its edge, named by its kind.
fn transition_marks(
    transitions: Vec<(u64, shot_core::TransitionKind)>,
    x: &impl Fn(u64) -> f32,
) -> Vec<Div> {
    transitions
        .into_iter()
        .map(|(frame, kind)| {
            div()
                .absolute()
                .top(px(-18.))
                .bottom_0()
                .left(px(NAME_WIDTH))
                .right(px(16.))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(gpui::relative(x(frame)))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px(px(5.))
                                .rounded(px(4.))
                                .bg(BLOCK_ON)
                                .text_color(TEXT)
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .whitespace_nowrap()
                                .child(capitalized(kind.as_str())),
                        )
                        .child(div().flex_1().w(px(1.)).bg(Hsla::from(TEXT).opacity(0.35))),
                )
        })
        .collect()
}

fn lane(icon_name: &str, name: &'static str, height: Pixels, track: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .child(
            div()
                .w(px(NAME_WIDTH))
                .flex_none()
                .pl(px(16.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(TEXT_SM)
                .font_weight(FontWeight::MEDIUM)
                .text_color(MUTED)
                .child(icon(icon_name, ICON, MUTED))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .relative()
                .mr(px(16.))
                .h(height)
                .rounded(px(6.))
                .bg(WELL)
                .overflow_hidden()
                .child(track),
        )
}

fn block(
    left: f32,
    right: f32,
    background: impl Into<Hsla>,
    color: impl Into<Hsla>,
    text: String,
) -> Div {
    let background: Hsla = background.into();
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(gpui::relative(left))
        .w(gpui::relative((right - left).max(0.002)))
        .rounded(px(6.))
        .px(px(8.))
        .bg(background)
        .text_color(color)
        .text_size(TEXT_XS)
        .font_weight(FontWeight::MEDIUM)
        .line_height(px(36.))
        .overflow_hidden()
        .whitespace_nowrap()
        .child(text)
}

fn layout_lane(this: &EditScreen, window: &Range<u64>, x: impl Fn(u64) -> f32) -> impl IntoElement {
    let mut runs: Vec<(u64, u64, shot_core::Layout)> = Vec::new();
    for span in &this.map.spans {
        let frames = span.output_frames();
        if frames.end <= window.start || frames.start >= window.end {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.2 == span.layout && last.1 == frames.start => last.1 = frames.end,
            _ => runs.push((frames.start, frames.end, span.layout)),
        }
    }
    let bookends = this.map.plan.bookends.iter().filter_map(|b| {
        let (from, to) = (b.output_frame, b.output_frame + b.frames);
        let name = match b.kind {
            BookendKind::Intro => "Intro",
            BookendKind::Outro => "Outro",
        };
        (to > window.start && from < window.end).then_some((from, to, name))
    });
    div()
        .size_full()
        .children(runs.into_iter().map(|(from, to, layout)| {
            block(
                x(from.max(window.start)),
                x(to.min(window.end)) - 0.002,
                BLOCK,
                BODY,
                layout_name(layout).to_owned(),
            )
        }))
        .children(bookends.map(|(from, to, name)| {
            block(
                x(from.max(window.start)),
                x(to.min(window.end)) - 0.002,
                BLOCK_ON,
                TEXT,
                name.to_owned(),
            )
        }))
}

/// Every zoom the edit shows, a person's with its index in `cut.kdl`.
fn zoom_items(this: &EditScreen, cx: &Context<EditScreen>) -> Vec<Item> {
    let folder = this.folder.read(cx);
    let mut items = Vec::new();
    for (at, block) in model::blocks(&folder.shown.cut) {
        let own = folder
            .project
            .cut
            .chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take));
        for zoom in &block.zooms {
            let Some(frames) = this.map.output_range(at, zoom.range) else {
                continue;
            };
            let placed = own
                .and_then(|b| b.zooms.iter().position(|z| z == zoom))
                .map(Placed::Zoom);
            items.push(Item {
                at,
                placed,
                frames,
                text: format!("{}%", zoom.scale),
            });
        }
    }
    items
}

/// Zooms or overlays on a lane, in two rows where they overlap. A person's can be picked,
/// trimmed by their edges and moved by their middle; the lock's say where they come from.
fn item_lane(
    this: &EditScreen,
    id: &'static str,
    items: Vec<Item>,
    window: &Range<u64>,
    x: &impl Fn(u64) -> f32,
    (fill, color): (Hsla, Hsla),
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let dragged = match &this.lanes.drag {
        Some(Drag::Shift {
            at, placed, now, ..
        }) => Some((*at, *placed, now.clone())),
        _ => None,
    };
    let mut shown: Vec<(Item, usize)> = Vec::new();
    let mut ends = [0u64; 2];
    for mut item in items {
        if let (Some((at, placed, now)), Some(own)) = (&dragged, item.placed)
            && item.at == *at
            && own == *placed
        {
            item.frames = now.clone();
        }
        if item.frames.end <= window.start || item.frames.start >= window.end {
            continue;
        }
        let row = ends
            .iter()
            .position(|end| *end <= item.frames.start)
            .unwrap_or(1);
        ends[row] = ends[row].max(item.frames.end);
        shown.push((item, row));
    }
    let rows = if shown.iter().any(|(_, row)| *row == 1) {
        2.
    } else {
        1.
    };
    let height = (36. - 2. * (rows - 1.)) / rows;
    div()
        .size_full()
        .children(shown.into_iter().enumerate().map(|(key, (item, row))| {
            let left = x(item.frames.start.max(window.start));
            let right = x(item.frames.end.min(window.end));
            let picked = item
                .placed
                .is_some_and(|p| this.lanes.picked == Some(Pick::Placed(item.at, p)));
            let element = div()
                .id((id, key))
                .absolute()
                .top(px((height + 2.) * row as f32))
                .h(px(height))
                .left(gpui::relative(left))
                .w(gpui::relative((right - left).max(0.002)))
                .rounded(px(6.))
                .px(px(8.))
                .bg(fill)
                .text_color(color)
                .text_size(TEXT_XS)
                .font_weight(FontWeight::MEDIUM)
                .line_height(px(height))
                .overflow_hidden()
                .whitespace_nowrap()
                .when(picked, |b| b.border_1().border_color(color))
                .child(item.text);
            match item.placed {
                None => element
                    .tooltip(|window, cx| Tooltip::new("From the style rules").build(window, cx)),
                Some(placed) => {
                    let grabbed = (item.at, placed, item.frames.clone());
                    element
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener({
                                let grabbed = grabbed.clone();
                                move |this, event: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.grab(grabbed.clone(), Grip::Body, event.position.x);
                                    cx.notify();
                                }
                            }),
                        )
                        .child(handle(grabbed.clone(), Grip::Start, picked, color, cx).left_0())
                        .child(handle(grabbed, Grip::End, picked, color, cx).right_0())
                }
            }
        }))
}

/// A trim handle on one edge of a picked or hovered block.
fn handle(
    grabbed: (TakeAt, Placed, Range<u64>),
    grip: Grip,
    picked: bool,
    color: Hsla,
    cx: &mut Context<EditScreen>,
) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(7.))
        .cursor_ew_resize()
        .when(picked, |h| h.bg(color.opacity(0.35)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.grab(grabbed.clone(), grip, event.position.x);
                cx.notify();
            }),
        )
}

fn words_lane(
    this: &EditScreen,
    window: &Range<u64>,
    x: impl Fn(u64) -> f32,
    track: f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let mut words = Vec::new();
    // Where each of the two word rows is free again, in pixels. A word that fits neither row
    // shows only through the waveform.
    let mut rows = [f32::MIN; 2];
    for block in &this.doc {
        for (index, word) in block.words.iter().enumerate() {
            let Some(frame) = word.output.filter(|f| window.contains(f)) else {
                continue;
            };
            if word.state != WordState::Kept && this.view == model::View::Edited {
                continue;
            }
            let selected = this
                .selection
                .is_some_and(|s| s.at == block.at && s.words().contains(&index));
            let at = block.at;
            let left = x(frame) * track;
            let width = word.text.chars().count() as f32 * WORD_CHAR + 6.;
            let Some(row) = rows.iter().position(|free| *free <= left) else {
                continue;
            };
            rows[row] = left + width + 4.;
            words.push(
                div()
                    .id((
                        "lane-word",
                        at.chapter * 1_000_000 + at.take * 100_000 + index,
                    ))
                    .absolute()
                    .top(px(6. + 30. * row as f32))
                    .left(gpui::relative(x(frame)))
                    .h(px(22.))
                    .px(px(3.))
                    .rounded(px(3.))
                    .text_size(TEXT_SM)
                    .line_height(px(22.))
                    .whitespace_nowrap()
                    .text_color(BODY)
                    .cursor_pointer()
                    .hover(|w| w.bg(RAISED))
                    .when(word.state != WordState::Kept, |w| {
                        w.text_color(CUT_TEXT).line_through()
                    })
                    .when(selected, |w| w.bg(SELECTION).text_color(STRONG))
                    .child(word.text.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let extend = event.modifiers.shift;
                            this.select_word(at, index, extend, cx);
                            if !extend {
                                this.lanes.drag = Some(Drag::Select { from: frame });
                            }
                        }),
                    )
                    .into_any_element(),
            );
        }
    }
    let seams = if this.view == model::View::Edited {
        seams(this, window, &x)
    } else {
        Vec::new()
    };
    div()
        .size_full()
        .children(seams.into_iter().enumerate().map(|(key, seam)| {
            let picked = this.lanes.picked == Some(Pick::Cut(seam.at, seam.range));
            let (at, range, frame) = (seam.at, seam.range, seam.frame);
            let tip = seam.removed.clone();
            div()
                .id(("seam", key))
                .absolute()
                .top_0()
                .bottom_0()
                .left(gpui::relative(seam.left))
                .ml(px(-5.))
                .w(px(12.))
                .flex()
                .justify_center()
                .cursor_pointer()
                .when_some(tip, |s, tip| {
                    s.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.selection = None;
                        this.lanes.picked = Some(Pick::Cut(at, range));
                        this.seek(frame);
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .my(px(4.))
                        .w(px(if picked { 4. } else { 2. }))
                        .rounded(px(1.))
                        .bg(if seam.length.is_some() || picked {
                            Hsla::from(CUT)
                        } else {
                            Hsla::from(FAINT)
                        }),
                )
                .children(seam.length.map(|text| {
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .left(px(10.))
                        .font_family(MONO)
                        .text_size(px(11.))
                        .text_color(CUT_TEXT)
                        .whitespace_nowrap()
                        .child(text)
                }))
        }))
        .children(words)
}

/// A cut between kept words, where it sits on the Words lane.
struct Seam {
    at: TakeAt,
    range: TimeRange,
    frame: u64,
    left: f32,
    /// The length chip of a long or a person's cut.
    length: Option<String>,
    /// The removed words, for the tooltip.
    removed: Option<String>,
}

fn seams(this: &EditScreen, window: &Range<u64>, x: &impl Fn(u64) -> f32) -> Vec<Seam> {
    this.doc
        .iter()
        .flat_map(|b| {
            b.paragraphs
                .iter()
                .flat_map(move |p| p.items.iter().map(move |i| (b, i)))
        })
        .filter_map(|(block, item)| match item {
            model::Item::Cut(run) => {
                let frame = this.map.output_frame(block.at, run.range.to())?;
                window.contains(&frame).then(|| {
                    let removed: Vec<&str> = block.words[run.words.clone()]
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect();
                    Seam {
                        at: block.at,
                        range: run.range,
                        frame,
                        left: x(frame),
                        length: (!run.is_seam()).then(|| seconds(run.removed)),
                        removed: (!removed.is_empty())
                            .then(|| format!("{}: {}", seconds(run.removed), removed.join(" "))),
                    }
                })
            }
            model::Item::Word(_) => None,
        })
        .collect()
}

/// Peaks of the mic under each column, through the edit. Removed media in Untrimmed shows red.
fn voice_lane(
    this: &EditScreen,
    window: &Range<u64>,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let cut = &this.folder.read(cx).shown.cut;
    let removed_in = |at: shot_core::TakeAt| -> Vec<shot_core::TimeRange> {
        if this.view == model::View::Edited {
            return Vec::new();
        }
        cut.chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .map(|edit| {
                edit.ranges
                    .iter()
                    .filter_map(|r| match r {
                        shot_core::RangeEdit::Remove { range, .. } => Some(*range),
                        shot_core::RangeEdit::Keep(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let spans: Vec<VoiceSpan> = this
        .map
        .spans
        .iter()
        .filter(|s| s.output_frames().end > window.start && s.output_frame < window.end)
        .map(|s| VoiceSpan {
            output: s.output_frames(),
            source_start: s.source_frames.start,
            peaks: this.peaks.get(&s.take).cloned().flatten(),
            removed: removed_in(s.at),
        })
        .collect();
    let window = window.clone();
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, (), win, _| {
            let bar = px(3.);
            let columns = ((f32::from(bounds.size.width) / f32::from(bar)) as u64).max(1);
            let frames = window.end - window.start;
            for column in 0..columns {
                let from = window.start + column * frames / columns;
                let to = (window.start + (column + 1) * frames / columns).max(from + 1);
                let Some(span) = spans.iter().find(|s| s.output.contains(&from)) else {
                    continue;
                };
                let Some(peaks) = &span.peaks else {
                    continue;
                };
                let source = |f: u64| {
                    frame_time(
                        span.source_start + (f.min(span.output.end) - span.output.start),
                        FPS,
                    )
                };
                let time = source(from);
                let peak = peaks.over(time, source(to));
                let removed = span
                    .removed
                    .iter()
                    .any(|r| r.from() <= time && time < r.to());
                let height = (bounds.size.height - px(8.)) * peak.clamp(0.02, 1.0);
                let left = bounds.origin.x + bar * column as f32;
                let top = bounds.origin.y + (bounds.size.height - height) / 2.;
                win.paint_quad(
                    fill(
                        Bounds::new(point(left, top), size(bar - px(1.), height)),
                        if removed {
                            Hsla::from(CUT).opacity(0.35)
                        } else {
                            Hsla::from(WAVE)
                        },
                    )
                    .corner_radii(px(1.)),
                );
            }
        },
    )
    .size_full()
}

struct VoiceSpan {
    output: Range<u64>,
    source_start: u64,
    peaks: Option<Arc<Peaks>>,
    /// Media the edit removes, shown red in Untrimmed.
    removed: Vec<shot_core::TimeRange>,
}

fn playhead_line(this: &EditScreen, window: &Range<u64>) -> Option<Div> {
    if !window.contains(&this.playhead) {
        return None;
    }
    let at = model::fraction(this.playhead - window.start, window.end - window.start);
    Some(
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(NAME_WIDTH))
            .right(px(16.))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(at))
                    .w(px(2.))
                    .rounded(px(1.))
                    .bg(ACCENT),
            ),
    )
}

/// Whether `template` belongs on the Highlight lane.
pub fn is_highlight(template: &str) -> bool {
    template == highlight::TEMPLATE
}
