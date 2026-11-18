//! The Timeline workspace: preview, change list and overlays on top, a split to drag between
//! them and the lanes, the whole-video overview, then one lane per kind: Layout (with the intro
//! and outro), Zoom, Overlays, Highlight, Words, Voice, Sound and Music, with export's
//! transitions as marks on their edges.
//!
//! The lanes are the editing surface. Click or drag the ruler or an empty lane to move the
//! playhead, drag across the lanes to select words, click a block to pick it: a zoom or overlay
//! (drag its edges to trim it, its middle to move it), a layout stretch, a cut, the music bed.
//! The toolbar under the lanes and the right-click menu offer what the selection or the pick
//! can do, each with its key; a right-click selects or picks what is under it first. `=` and
//! `-` zoom the lanes around the playhead, ⌥Z to the selection and ⇧Z to the whole video, as do
//! the slider and the overview's frame (its middle pans, its edges zoom). Scrolling pans,
//! ⌘-scroll and pinch zoom around the pointer.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, Div, ElementId, FontWeight, Hsla, MouseButton, MouseDownEvent,
    PinchEvent, Pixels, Point, ScrollEvent, SharedString, Stateful, Window, canvas, div, fill,
    point, prelude::*, px, size,
};
use gpui_component::menu::ContextMenuExt;
use gpui_component::tooltip::Tooltip;
use shot_core::{Applied, Id, Layout, Micros, Placed, RemovedBy, TakeAt, TimeRange, WordState};
use shot_render::join::BookendKind;
use shot_render::overlay::highlight;
use shot_render::plan::frame_time;

use super::parts::{self, layout_name};
use super::{EditScreen, music};
use crate::model::{self, FPS, LayoutRun, Selection, changes, seconds, timecode};
use crate::peaks::Peaks;
use crate::theme::*;
use crate::ui::{self, hint, icon, kbd, label, mono};

const NAME_WIDTH: f32 = 112.;
/// Average width of a character at 13 px, for packing words into rows, and at 11 px mono, for
/// packing cut lengths.
const WORD_CHAR: f32 = 7.2;
const CHIP_CHAR: f32 = 6.8;
/// A row of words on the Words lane, and the strip under the rows that holds cut lengths.
const WORD_ROW: f32 = 28.;
const WORD_HEIGHT: f32 = 22.;
const CHIP_STRIP: f32 = 16.;
/// The lanes pane at its natural height and at its smallest, where the Words lane keeps one
/// row and the Voice lane a thin waveform. Both include the margin under the pane.
const LANES_HEIGHT: f32 = 510.;
const LANES_MIN: f32 = 456.;
/// The split between the top row and the lanes.
const SPLIT: f32 = 8.;
/// The top row's height: its least, the most it opens at, and the width the change list keeps.
const TOP_MIN: f32 = 220.;
const TOP_OPENS: f32 = 420.;
/// The least the top row opens at while the lanes can give way to their smallest, so the
/// preview stays usable on short windows.
const TOP_USABLE: f32 = 300.;
const CHANGES_MIN: f32 = 260.;
/// The title bar above the screen.
const TITLE_BAR: f32 = 46.;
/// The fewest and most output frames the lanes show: 2 s up to the whole video.
const MIN_SPAN: u64 = 2 * FPS as u64;
const DEFAULT_SPAN: u64 = 15 * FPS as u64;
/// How far a dragged edge jumps to the playhead or a word edge, in pixels.
const SNAP: f32 = 8.;
/// The shortest zoom or overlay a drag leaves, in frames.
const MIN_LENGTH: u64 = 6;
/// The scale of a zoom a person adds, and the scales the toolbar and menu offer.
const DEFAULT_ZOOM: u16 = 130;
pub(super) const ZOOM_SCALES: [u16; 4] = [115, 130, 150, 200];

/// What the Timeline lanes show and what the mouse does in them.
pub struct Lanes {
    /// Output frames the lanes show, and the first of them.
    span: u64,
    start: u64,
    /// The playhead the view last followed. A pan holds until the playhead moves.
    followed: Option<u64>,
    /// Where the tracks, the Words lane and the zoom slider were last painted, for mouse
    /// mapping and the number of word rows.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    words: Rc<Cell<Bounds<Pixels>>>,
    slider: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    /// The block a click picked, which the toolbar, the menu and the keys act on.
    pub picked: Option<Pick>,
    /// What the right-click menu is about.
    pub(super) menu: Option<MenuOn>,
    /// The layout switch whose popover is open.
    pub(super) switch: Option<super::switch::OpenSwitch>,
    /// The top row's height the person dragged the split to, and the range it may take.
    top: Option<f32>,
    top_range: (f32, f32),
}

impl Default for Lanes {
    fn default() -> Self {
        Self {
            span: DEFAULT_SPAN,
            start: 0,
            followed: None,
            bounds: Rc::default(),
            words: Rc::default(),
            slider: Rc::default(),
            drag: None,
            picked: None,
            menu: None,
            switch: None,
            top: None,
            top_range: (TOP_MIN, TOP_MIN),
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
    /// A stretch of one layout, by its media range, as [`model::LayoutRun`] has it.
    Layout(TakeAt, TimeRange),
    /// The music bed, or the place for one.
    Music,
}

/// What a right-click opened the menu on. A right-click selects or picks first, so the
/// selection or the pick says the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MenuOn {
    Selection,
    Pick,
    /// A zoom, overlay or sound of the lock, which the lanes cannot move, with its name and
    /// what a remove takes out, if it can.
    Lock(String, Option<(Id, shot_core::Instance)>),
    /// An empty spot, where the playhead now is.
    Empty,
}

enum Drag {
    /// The ruler moves the playhead.
    Scrub,
    /// The overview moves the playhead.
    Overview,
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
    /// The overview's frame around the lanes' window: the body pans, an edge zooms.
    Frame {
        grip: Grip,
        grab: Pixels,
        was: Range<u64>,
    },
    /// The zoom slider's thumb.
    Slider,
    /// The split between the top row and the lanes, from the height it had.
    Split { grab: Pixels, was: f32 },
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
    /// Its index in `cut.kdl`, `None` for the lock's, which the lanes cannot move.
    pub placed: Option<Placed>,
    /// For the lock's zooms and overlays: the take and the instance a remove turns off.
    pub lock: Option<(Id, shot_core::Instance)>,
    pub frames: Range<u64>,
    pub text: String,
}

impl Lanes {
    fn window(&self, total: u64) -> Range<u64> {
        let span = self.span.min(total).max(1);
        let start = self.start.min(total.saturating_sub(span));
        start..start + span
    }

    /// Shows `span` frames from `start`, within the video.
    fn set_window(&mut self, start: u64, span: u64, total: u64) {
        self.span = span.clamp(MIN_SPAN, total.max(MIN_SPAN));
        self.start = start.min(total.saturating_sub(self.span));
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

    /// The rows of words the Words lane has room for.
    fn word_rows(&self) -> usize {
        let height = f32::from(self.words.get().size.height);
        if height <= 0. {
            return 2;
        }
        let rows = (height - 4. - CHIP_STRIP + WORD_ROW - WORD_HEIGHT) / WORD_ROW;
        (rows.floor() as usize).max(1)
    }
}

/// The shortest zoom Z makes. A shorter one punches in and out before the eye catches up.
const ZOOM_HOLD: shot_core::Micros = shot_core::Micros(1_500_000);

/// `range` grown evenly on both sides to at least `hold`, inside `take`. Where one side runs
/// into the take's edge the other side grows instead.
fn at_least(range: TimeRange, hold: shot_core::Micros, take: TimeRange) -> TimeRange {
    let hold = hold.0.min(take.duration().0);
    let short = hold.saturating_sub(range.duration().0);
    if short == 0 {
        return range;
    }
    let from = range
        .from()
        .0
        .saturating_sub(short / 2)
        .max(take.from().0)
        .min(take.to().0 - hold);
    TimeRange::new(
        shot_core::Micros(from),
        shot_core::Micros(from + hold.max(range.duration().0)),
    )
    .unwrap_or(range)
}

/// Where the zoom slider sits for a window of `span` frames: 0 shows the whole video, 1 the
/// closest view. The scale is logarithmic, so each step of the slider zooms by the same factor.
fn slider_at(span: u64, total: u64) -> f32 {
    if total <= MIN_SPAN {
        return 0.;
    }
    let range = (total as f64 / MIN_SPAN as f64).ln();
    ((total as f64 / span.max(1) as f64).ln() / range).clamp(0., 1.) as f32
}

/// The window [`slider_at`] puts at `at`.
fn slider_span(at: f32, total: u64) -> u64 {
    let closest = MIN_SPAN as f64 / total.max(1) as f64;
    (total as f64 * closest.powf(f64::from(at.clamp(0., 1.)))).round() as u64
}

impl EditScreen {
    fn lane_window(&self) -> Range<u64> {
        self.lanes.window(self.map.frames)
    }

    /// Shows `span` frames, keeping `anchor` (else the playhead when shown, else the middle)
    /// where it is on screen.
    fn zoom_lanes_to(&mut self, span: u64, anchor: Option<u64>, cx: &mut Context<Self>) {
        let window = self.lane_window();
        let length = (window.end - window.start).max(1);
        let anchor = anchor
            .or_else(|| window.contains(&self.playhead).then_some(self.playhead))
            .unwrap_or(window.start + length / 2);
        let before = anchor.saturating_sub(window.start) as f64 / length as f64;
        let span = span.clamp(MIN_SPAN, self.map.frames.max(MIN_SPAN));
        let start = anchor.saturating_sub((before * span as f64).round() as u64);
        self.lanes.set_window(start, span, self.map.frames);
        cx.notify();
    }

    /// Shows `factor` times as many frames, see [`Self::zoom_lanes_to`].
    pub(super) fn zoom_lanes(&mut self, factor: f32, anchor: Option<u64>, cx: &mut Context<Self>) {
        let window = self.lane_window();
        let span = ((window.end - window.start) as f64 * f64::from(factor)).round() as u64;
        self.zoom_lanes_to(span, anchor, cx);
    }

    pub(super) fn fit_lanes(&mut self, cx: &mut Context<Self>) {
        self.lanes.set_window(0, self.map.frames, self.map.frames);
        cx.notify();
    }

    /// Fits the lanes to the selection, else the picked block, else the playhead's chapter,
    /// with a little room on either side.
    pub(super) fn zoom_to_selection(&mut self, cx: &mut Context<Self>) {
        let frames = self
            .selection_frames()
            .or_else(|| self.picked_frames(cx))
            .or_else(|| {
                let chapter = self.map.chapter_at(self.playhead)?;
                Some(self.map.chapters.get(chapter)?.frames.clone())
            });
        let Some(frames) = frames else {
            return;
        };
        let length = (frames.end - frames.start).max(MIN_SPAN);
        let room = length / 10;
        let middle = (frames.start + frames.end) / 2;
        self.lanes.set_window(
            middle.saturating_sub(length / 2 + room),
            length + 2 * room,
            self.map.frames,
        );
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

    /// The top row's height for a viewport, within what leaves the lanes and the change list
    /// their room.
    fn top_height(&mut self, width: f32, height: f32) -> f32 {
        let room = height - TITLE_BAR - SPLIT;
        let stage_max = width - 32. - super::overlays::PANE_WIDTH - CHANGES_MIN;
        let by_width = (stage_max - 24.) * 9. / 16. + 66.;
        let most = (room - LANES_MIN).min(by_width).max(TOP_MIN);
        let opens = (room - LANES_HEIGHT).clamp(TOP_USABLE, TOP_OPENS).min(most);
        self.lanes.top_range = (TOP_MIN, most);
        self.lanes.top.unwrap_or(opens).clamp(TOP_MIN, most)
    }

    /// Follows the mouse while a drag in the Timeline runs.
    pub(super) fn drag_lanes(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let total = self.map.frames;
        let frame = self.lanes.frame_at(position.x, total);
        let threshold = (SNAP * self.lanes.per_pixel(total)) as u64;
        match self.lanes.drag {
            None => return,
            Some(Drag::Scrub) => self.seek(frame),
            Some(Drag::Overview) => self.seek_overview(position.x, cx),
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
            Some(Drag::Frame {
                grip,
                grab,
                ref was,
            }) => {
                let width = f32::from(self.overview_bounds.get().size.width).max(1.);
                let delta = (f32::from(position.x - grab) / width * total as f32) as i64;
                let (start, end) = match grip {
                    Grip::Body => (was.start.saturating_add_signed(delta), None),
                    Grip::Start => (
                        was.start
                            .saturating_add_signed(delta)
                            .min(was.end.saturating_sub(MIN_SPAN)),
                        Some(was.end),
                    ),
                    Grip::End => (
                        was.start,
                        Some(
                            was.end
                                .saturating_add_signed(delta)
                                .clamp(was.start + MIN_SPAN, total),
                        ),
                    ),
                };
                let span = end.map_or(was.end - was.start, |end| end - start);
                self.lanes.set_window(start, span, total);
            }
            Some(Drag::Slider) => {
                let bounds = self.lanes.slider.get();
                let at =
                    f32::from(position.x - bounds.origin.x) / f32::from(bounds.size.width).max(1.);
                self.zoom_lanes_to(slider_span(at, total), None, cx);
            }
            Some(Drag::Split { grab, was }) => {
                let (least, most) = self.lanes.top_range;
                self.lanes.top = Some((was + f32::from(position.y - grab)).clamp(least, most));
            }
        }
        cx.notify();
    }

    /// Writes a moved or trimmed zoom or overlay when its drag ends.
    pub(super) fn end_lane_drag(&mut self, cx: &mut Context<Self>) {
        let Some(Drag::Shift {
            at,
            placed,
            grip,
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
        let Some(mut range) = self.map.source_range(at, now) else {
            return;
        };
        // An overlay's copy on the lane is clipped to its block: move the overlay, not the copy.
        if let Placed::Overlay(index) = placed {
            let stored = {
                let cut = &self.folder.read(cx).project.cut;
                let edit = cut
                    .chapters
                    .get(at.chapter)
                    .and_then(|c| c.takes.get(at.take));
                edit.and_then(|edit| {
                    let (first, last) = (edit.ranges.first()?, edit.ranges.last()?);
                    let block = TimeRange::new(first.range().from(), last.range().to())?;
                    Some((edit.overlays.get(index)?.range, block))
                })
            };
            let moved =
                stored
                    .zip(self.map.source_range(at, was))
                    .and_then(|((stored, block), shown)| {
                        dragged_overlay(stored, block, shown, range, grip)
                    });
            match moved {
                Some(moved) => range = moved,
                None => return,
            }
        }
        self.edit(cx, |cut| cut.set_range(at, placed, range));
        // Zooms are kept in time order, so the moved one may have a new index.
        if let Placed::Zoom(_) = placed {
            self.lanes.picked = self
                .zoom_index(at, range, cx)
                .map(|index| Pick::Placed(at, Placed::Zoom(index)));
        }
    }

    fn zoom_index(&self, at: TakeAt, range: TimeRange, cx: &App) -> Option<usize> {
        let cut = &self.folder.read(cx).project.cut;
        cut.chapters
            .get(at.chapter)?
            .takes
            .get(at.take)?
            .zooms
            .iter()
            .position(|z| z.range == range)
    }

    /// Picks a block in the lanes, dropping the selection.
    fn pick(&mut self, pick: Pick) {
        self.selection = None;
        self.proposal = None;
        self.lanes.picked = Some(pick);
    }

    /// Removes the picked zoom, overlay or music bed. False when none is picked.
    pub(super) fn remove_picked(&mut self, cx: &mut Context<Self>) -> bool {
        match self.lanes.picked {
            Some(Pick::Placed(at, placed)) => {
                self.lanes.picked = None;
                self.edit(cx, |cut| cut.remove_placed(at, placed));
                true
            }
            Some(Pick::Music) if self.folder.read(cx).project.cut.music.is_some() => {
                self.remove_music(cx);
                true
            }
            _ => false,
        }
    }

    /// Adds a zoom over the selected words, at [`DEFAULT_ZOOM`].
    pub(super) fn add_zoom(&mut self, cx: &mut Context<Self>) {
        let Some((at, range, ..)) = self.selected() else {
            return;
        };
        let folder = self.folder.read(cx);
        let (scale, hold) = (DEFAULT_ZOOM, ZOOM_HOLD);
        let take = folder
            .project
            .cut
            .chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .and_then(|t| {
                TimeRange::new(
                    t.ranges.first()?.range().from(),
                    t.ranges.last()?.range().to(),
                )
            });
        let range = take.map_or(range, |take| at_least(range, hold, take));
        self.selection = None;
        self.edit(cx, |cut| cut.add_zoom(at, range, scale));
        self.lanes.picked = self
            .zoom_index(at, range, cx)
            .map(|index| Pick::Placed(at, Placed::Zoom(index)));
    }

    /// Sets a zoom's scale. When the playhead is outside the zoom, seeks to its middle, inside
    /// its hold, so the preview shows the new scale.
    pub(super) fn set_zoom_scale(
        &mut self,
        at: TakeAt,
        index: usize,
        scale: u16,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, |cut| cut.set_zoom_scale(at, index, scale));
        if let Some(frames) = self.zoom_frames(at, index, cx)
            && !frames.contains(&self.playhead)
        {
            self.seek((frames.start + frames.end) / 2);
            cx.notify();
        }
    }

    /// Seeks to where a zoom starts in the output.
    pub(super) fn seek_to_zoom(&mut self, at: TakeAt, index: usize, cx: &mut Context<Self>) {
        if let Some(frames) = self.zoom_frames(at, index, cx) {
            self.seek(frames.start);
            cx.notify();
        }
    }

    /// A zoom's output frames, `None` when it was removed.
    fn zoom_frames(&self, at: TakeAt, index: usize, cx: &App) -> Option<Range<u64>> {
        let cut = &self.folder.read(cx).project.cut;
        let zoom = cut
            .chapters
            .get(at.chapter)?
            .takes
            .get(at.take)?
            .zooms
            .get(index)?;
        self.map.output_range(at, zoom.range)
    }

    /// The pick after the map changed: a layout stretch becomes the stretch that now holds
    /// its middle, so the pick always matches a block on the Layout lane.
    pub(super) fn repick_layout(&mut self) {
        if let Some(Pick::Layout(at, range)) = self.lanes.picked {
            let middle = shot_core::Micros((range.from().0 + range.to().0) / 2);
            self.lanes.picked = self
                .map
                .layout_runs()
                .into_iter()
                .find(|r| r.at == at && r.range.from() <= middle && middle < r.range.to())
                .map(|r| Pick::Layout(r.at, r.range));
        }
    }

    /// Plays from the selection, else the picked block, else the playhead.
    pub(super) fn play_from_here(&mut self, cx: &mut Context<Self>) {
        let frame = self
            .selection_frames()
            .or_else(|| self.picked_frames(cx))
            .map_or(self.playhead, |f| f.start);
        self.seek(frame);
        self.set_playing(true);
        cx.notify();
    }

    /// Opens the sound picker, which places at the selection or the playhead.
    pub(super) fn add_sound_here(&mut self, cx: &mut Context<Self>) {
        if self.place.picker != Some(super::place::Picker::Sound) {
            self.toggle_picker(super::place::Picker::Sound, cx);
        }
    }

    /// The output frames of the selected words.
    pub(super) fn selection_frames(&self) -> Option<Range<u64>> {
        let selection = self.selection?;
        let words = self.block(selection.at)?.words.get(selection.words())?;
        let from = words.iter().find_map(|w| w.output)?;
        let to = words
            .iter()
            .rev()
            .find_map(|w| w.output.map(|start| start + word_frames(w.range)))?;
        Some(from..to.max(from + 1))
    }

    /// The output frames of the picked block.
    fn picked_frames(&self, cx: &App) -> Option<Range<u64>> {
        let cut = &self.folder.read(cx).project.cut;
        let block = |at: TakeAt| cut.chapters.get(at.chapter)?.takes.get(at.take);
        match self.lanes.picked? {
            Pick::Placed(at, Placed::Zoom(index)) => self
                .map
                .output_range(at, block(at)?.zooms.get(index)?.range),
            Pick::Placed(at, Placed::Overlay(index)) => self
                .map
                .output_range(at, block(at)?.overlays.get(index)?.range),
            Pick::Cut(at, range) => {
                let frame = self.map.seam(at, range)?;
                Some(frame.saturating_sub(MIN_SPAN / 2)..frame + MIN_SPAN / 2)
            }
            Pick::Layout(at, range) => self.map.output_range(at, range),
            Pick::Music => None,
        }
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
        self.pick(Pick::Placed(at, placed));
        self.lanes.drag = Some(Drag::Shift {
            at,
            placed,
            grip,
            grab: self.lanes.frame_at(x, self.map.frames),
            was: frames.clone(),
            now: frames,
        });
    }

    /// A press on the overview outside its frame: seeks there, shows that moment in the middle
    /// of the lanes and scrubs while the button is down.
    pub(super) fn press_overview(&mut self, x: Pixels, cx: &mut Context<Self>) {
        self.seek_overview(x, cx);
        let window = self.lane_window();
        let span = window.end - window.start;
        self.lanes.set_window(
            self.playhead.saturating_sub(span / 2),
            span,
            self.map.frames,
        );
        self.lanes.followed = Some(self.playhead);
        self.lanes.drag = Some(Drag::Overview);
    }
}

pub(super) fn words(count: usize) -> &'static str {
    if count == 1 { "word" } else { "words" }
}

/// The picked zoom's or overlay's line for the toolbar and menu, with a zoom's scale.
pub(super) fn picked_text(
    this: &EditScreen,
    at: TakeAt,
    placed: Placed,
    cx: &App,
) -> Option<(String, u16)> {
    let cut = &this.folder.read(cx).project.cut;
    let block = cut.chapters.get(at.chapter)?.takes.get(at.take)?;
    match placed {
        Placed::Zoom(index) => block.zooms.get(index).map(|z| {
            (
                format!("Zoom {}%, {}", z.scale, seconds(z.range.duration())),
                z.scale,
            )
        }),
        Placed::Overlay(index) => block.overlays.get(index).map(|o| {
            (
                format!(
                    "{}, {}",
                    super::overlays::summary(o),
                    seconds(o.range.duration())
                ),
                0,
            )
        }),
    }
}

/// The words a cut run removed, in order.
pub(super) fn removed_words(this: &EditScreen, at: TakeAt, range: TimeRange) -> String {
    this.block(at)
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
        .unwrap_or_default()
}

fn word_frames(range: TimeRange) -> u64 {
    range.duration().0 * u64::from(FPS) / 1_000_000
}

pub fn render(this: &mut EditScreen, win: &mut Window, cx: &mut Context<EditScreen>) -> Div {
    this.lanes.follow(this.playhead, this.map.frames);
    let window = this.lane_window();
    let viewport = win.viewport_size();
    // The top row gives way first on short windows; the preview keeps 16:9 inside it.
    let top = this.top_height(f32::from(viewport.width), f32::from(viewport.height));
    let stage = (top - 24. - 42.) * 16. / 9. + 24.;
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(parts::error_strip(this, cx))
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
        .child(split(top, cx))
        .child(lanes(this, window, cx))
}

/// The edge between the top row and the lanes, dragged to give one of them more room.
fn split(top: f32, cx: &mut Context<EditScreen>) -> impl IntoElement {
    div()
        .id("split")
        .group("split")
        .h(px(SPLIT))
        .flex_none()
        .mx(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_row_resize()
        .child(
            div()
                .w(px(40.))
                .h(px(3.))
                .rounded(px(2.))
                .group_hover("split", |s| s.bg(FAINT)),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if event.click_count == 2 {
                    this.lanes.top = None;
                } else {
                    this.lanes.drag = Some(Drag::Split {
                        grab: event.position.y,
                        was: top,
                    });
                }
                cx.notify();
            }),
        )
}

fn change_list(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let folder = this.folder.read(cx);
    let changes = changes(&folder.project.cut, folder.lock());
    let raw: u64 = model::blocks(&folder.project.cut)
        .filter_map(|(_, edit)| {
            Some(edit.ranges.last()?.range().to().0 - edit.ranges.first()?.range().from().0)
        })
        .sum();
    let line =
        |name: &'static str, color: gpui::Rgba, text: String, count: usize, delta: String| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(12.))
                .px(px(10.))
                .py(px(8.))
                .rounded(RADIUS_SM)
                .child(icon(name, ICON, color))
                .child(div().flex_1().min_w_0().truncate().child(text))
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
                .child(div().min_w_0().truncate().text_color(MUTED).child(format!(
                    "against the raw takes, {}",
                    timecode(raw * u64::from(FPS) / 1_000_000)
                ))),
        )
        .child(ui::scrolling_list(
            "change-lines",
            &this.changes_scroll,
            div()
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
        ))
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

/// How a lane takes its height: fixed, or growing with the room the split gives the lanes.
enum Height {
    Fixed(f32),
    Grow {
        natural: f32,
        least: f32,
        weight: f32,
    },
}

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
    let runs = this.map.layout_runs();
    let bounds = this.lanes.bounds.clone();
    let screen = cx.entity();
    ui::pane()
        .flex_1()
        .min_h(px(LANES_MIN - 8.))
        .mx(px(8.))
        .mb(px(8.))
        .pt(px(12.))
        .pb(px(10.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(14.))
                .px(px(16.))
                .h(px(24.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(chapter),
                )
                .child(
                    mono(format!(
                        "{} to {}",
                        timecode(window.start),
                        timecode(window.end)
                    ))
                    .flex_none()
                    .text_size(TEXT_SM)
                    .text_color(MUTED),
                )
                .child(div().flex_1())
                .child(zoom_controls(this, length, cx)),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .ml(px(NAME_WIDTH))
                .mr(px(16.))
                .child(parts::overview(this, window.clone(), px(20.), cx)),
        )
        .child(
            div()
                .id("ruler")
                .relative()
                .flex_none()
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
                        this.drag_lanes(event.position, cx);
                    }),
                ),
        )
        .child(
            div()
                .id("lanes")
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                // A right-click is on an empty spot unless a block or word under it says
                // otherwise as the event bubbles back up.
                .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                    if event.button == MouseButton::Right {
                        this.lanes.menu = Some(MenuOn::Empty);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if this.lanes.menu != Some(MenuOn::Empty) {
                            return;
                        }
                        this.selection = None;
                        this.lanes.picked = None;
                        this.proposal = None;
                        if event.position.x >= this.lanes.bounds.get().origin.x {
                            this.seek(this.lanes.frame_at(event.position.x, this.map.frames));
                        }
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        if this.lanes.switch.take().is_some() {
                            window.focus(&this.focus, cx);
                        }
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
                    crate::ui::measure(bounds)
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(NAME_WIDTH))
                        .right(px(16.)),
                )
                .child(lane(
                    "layout",
                    "Layout",
                    Height::Fixed(36.),
                    layout_lane(this, &runs, &window, &x, cx),
                ))
                .child(lane(
                    "zoom",
                    "Zoom",
                    Height::Fixed(36.),
                    item_lane(
                        this,
                        ("zoom-item", "Zoom"),
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
                    Height::Fixed(36.),
                    item_lane(
                        this,
                        ("overlay-item", "Overlay"),
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
                    Height::Fixed(36.),
                    item_lane(
                        this,
                        ("highlight-item", "Highlight"),
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
                    Height::Grow {
                        natural: 4. + 2. * WORD_ROW + CHIP_STRIP - (WORD_ROW - WORD_HEIGHT),
                        least: 4. + WORD_ROW + CHIP_STRIP - (WORD_ROW - WORD_HEIGHT),
                        weight: 2.,
                    },
                    words_lane(this, &window, &x, track, cx),
                ))
                .child(lane(
                    "mic",
                    "Voice",
                    Height::Grow {
                        natural: 56.,
                        least: 32.,
                        weight: 1.,
                    },
                    voice_lane(this, &window, cx),
                ))
                .child(lane(
                    "system-audio",
                    "Sound",
                    Height::Fixed(28.),
                    sound_lane(this, &window, &x, cx),
                ))
                .child(lane(
                    "music",
                    "Music",
                    Height::Fixed(28.),
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
                                .w(px(520.))
                                .on_any_mouse_down(|_, _, cx| cx.stop_propagation()),
                        )
                    },
                )
                .children(super::switch::popover(this, &window, &x, cx))
                .context_menu(move |menu, window, cx| {
                    super::lane_menu::context_menu(
                        menu,
                        &screen,
                        super::lane_menu::Place::Timeline,
                        window,
                        cx,
                    )
                }),
        )
        .child(toolbar(this, &runs, cx))
}

/// A small square button of the zoom controls.
fn zoom_button(id: &'static str, text: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .min_w(px(26.))
        .px(px(8.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(RAISED)
        .text_size(TEXT_SM)
        .text_color(TEXT)
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT))
        .child(text)
}

fn zoom_controls(this: &EditScreen, length: u64, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let span = length as f64 / f64::from(FPS);
    let shown = if span < 60. {
        format!("{span:.0} s")
    } else {
        timecode(length)
    };
    let at = slider_at(length, this.map.frames);
    let bounds = this.lanes.slider.clone();
    let focused =
        this.selection.is_some() || this.lanes.picked.is_some_and(|pick| pick != Pick::Music);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .child(
            zoom_button("lanes-out", "−")
                .tooltip(|window, cx| Tooltip::new("Show more  −").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.zoom_lanes(2.0, None, cx))),
        )
        .child(
            div()
                .id("lanes-zoom")
                .relative()
                .w(px(96.))
                .h(px(24.))
                .mx(px(8.))
                .cursor_pointer()
                .child(crate::ui::measure(bounds).absolute().size_full())
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(10.))
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(KBD),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(10.))
                        .h(px(4.))
                        .w(gpui::relative(at))
                        .rounded(px(2.))
                        .bg(MUTED),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(5.))
                        .left(gpui::relative(at))
                        .ml(px(-7.))
                        .size(px(14.))
                        .rounded_full()
                        .bg(TEXT)
                        .shadow_sm(),
                )
                .tooltip(|window, cx| Tooltip::new("Zoom the lanes").build(window, cx))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.lanes.drag = Some(Drag::Slider);
                        this.drag_lanes(event.position, cx);
                    }),
                ),
        )
        .child(
            zoom_button("lanes-in", "+")
                .tooltip(|window, cx| Tooltip::new("Show less  =").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.zoom_lanes(0.5, None, cx))),
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
            zoom_button("lanes-fit", "Fit")
                .tooltip(|window, cx| Tooltip::new("Whole video  ⇧Z").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.fit_lanes(cx))),
        )
        .child(
            zoom_button(
                "lanes-selection",
                if focused { "Selection" } else { "Chapter" },
            )
            .tooltip(move |window, cx| {
                let what = if focused {
                    "Zoom to the selection  ⌥Z"
                } else {
                    "Zoom to the chapter  ⌥Z"
                };
                Tooltip::new(what).build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| this.zoom_to_selection(cx))),
        )
}

/// The overview's frame around the lanes' window in the Timeline: drag its middle to pan, an
/// edge to show more or less.
pub(super) fn overview_frame(view: (f32, f32), cx: &mut Context<EditScreen>) -> impl IntoElement {
    let edge = |id: &'static str, grip: Grip, cx: &mut Context<EditScreen>| {
        div()
            .id(id)
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(9.))
            .flex()
            .justify_center()
            .cursor_ew_resize()
            .group("frame-edge")
            .child(
                div()
                    .w(px(3.))
                    .my(px(4.))
                    .rounded(px(2.))
                    .group_hover("frame-edge", |s| s.bg(TEXT)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.grab_frame(grip, event.position.x);
                    cx.notify();
                }),
            )
    };
    div()
        .id("overview-frame")
        .absolute()
        .top_0()
        .bottom_0()
        .left(gpui::relative(view.0))
        .w(gpui::relative((view.1 - view.0).max(0.)))
        .min_w(px(10.))
        .cursor_grab()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.grab_frame(Grip::Body, event.position.x);
                cx.notify();
            }),
        )
        .child(edge("frame-start", Grip::Start, cx).left(px(-5.)))
        .child(edge("frame-end", Grip::End, cx).right(px(-5.)))
}

impl EditScreen {
    fn grab_frame(&mut self, grip: Grip, x: Pixels) {
        self.lanes.drag = Some(Drag::Frame {
            grip,
            grab: x,
            was: self.lane_window(),
        });
    }
}

/// What the selection or the picked block can do, under the lanes.
fn toolbar(
    this: &EditScreen,
    runs: &[LayoutRun],
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let bar = div()
        .h(px(32.))
        .flex_none()
        .ml(px(NAME_WIDTH))
        .mr(px(16.))
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(TEXT_SM);
    let what = |text: String| {
        div()
            .flex_none()
            .pr(px(10.))
            .text_color(TEXT)
            .whitespace_nowrap()
            .child(text)
    };
    if let Some((_, range, kept, removed)) = this.selected() {
        let count = this.selection.map_or(0, |s| s.words().count());
        let sound_open = this.place.picker == Some(super::place::Picker::Sound);
        return bar
            .child(what(format!(
                "{count} {}, {}",
                words(count),
                seconds(range.duration())
            )))
            .when(kept, |bar| {
                bar.child(
                    tool("tool-cut", "cut", "Cut", super::DELETE_KEYS).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.delete_selection(&super::DeleteSelection, window, cx)
                        },
                    )),
                )
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
            .child(layout_choices(this, None, cx));
    }
    match this.lanes.picked {
        Some(Pick::Placed(at, placed)) => {
            let text = picked_text(this, at, placed, cx);
            let scale = text.as_ref().map_or(0, |(_, scale)| *scale);
            let mut bar = bar.children(text.map(|(text, _)| what(text)));
            if let Placed::Zoom(index) = placed {
                bar = bar
                    .children(zoom_scales(at, index, scale, cx))
                    .child(div().w(px(8.)));
            }
            bar.child(
                tool("tool-remove", "close", "Remove", super::DELETE_KEYS).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.remove_picked(cx);
                    },
                )),
            )
            .child(div().flex_1())
            .child(label("Drag the edges to trim, the middle to move").truncate())
        }
        Some(Pick::Cut(at, range)) => {
            let removed = removed_words(this, at, range);
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
        Some(Pick::Layout(at, range)) => {
            let run = runs.iter().find(|r| r.at == at && r.range == range);
            bar.children(run.map(|run| {
                what(format!(
                    "{}, {}",
                    layout_name(run.layout),
                    seconds(frame_time(run.frames.end - run.frames.start, FPS))
                ))
            }))
            .child(layout_choices(this, run.map(|r| r.layout), cx))
            .child(div().w(px(8.)))
            .child(tool("tool-switch", "layout", "Switch", "T").on_click(
                cx.listener(|this, _, window, cx| this.open_switch_near_playhead(window, cx)),
            ))
            .child(
                tool("tool-build", "edit", "Layout builder", "⇧L").on_click(
                    cx.listener(|this, _, window, cx| this.open_builder(None, window, cx)),
                ),
            )
        }
        Some(Pick::Music) => music_tools(this, bar, cx),
        None => bar
            .gap(px(18.))
            .child(label("Drag across the lanes to select words, right-click for more").truncate())
            .child(hint(super::DELETE_KEYS, "Cut"))
            .child(hint("Z", "Zoom"))
            .child(hint("S", "Sound"))
            .child(hint("1–4", "Layout"))
            .child(hint("T", "Switch"))
            .child(hint("⌘Z", "Undo")),
    }
}

/// The scales a zoom can take, `scale` lit, for the lanes' toolbar and the Text inspector.
pub(super) fn zoom_scales(
    at: TakeAt,
    index: usize,
    scale: u16,
    cx: &mut Context<EditScreen>,
) -> Vec<Stateful<Div>> {
    ZOOM_SCALES
        .into_iter()
        .map(|option| {
            choice(("zoom-scale", option as usize), format!("{option}%"), None)
                .on(option == scale)
                .on_click(
                    cx.listener(move |this, _, _, cx| this.set_zoom_scale(at, index, option, cx)),
                )
        })
        .collect()
}

/// The layouts on 1 to 4 with their keys, `current` lit; a click does what the key does.
fn layout_choices(this: &EditScreen, current: Option<Layout>, cx: &mut Context<EditScreen>) -> Div {
    let keys = this.layout_choices(cx).0;
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .ml(px(8.))
        .child(label("Layout").pr(px(4.)))
        .children(keys.iter().enumerate().map(|(key, layout)| {
            const KEYS: [&str; 4] = ["1", "2", "3", "4"];
            choice(
                ("layout-choice", key),
                layout_name(*layout),
                Some(KEYS[key]),
            )
            .on(current == Some(*layout))
            .on_click(cx.listener(move |this, _, _, cx| this.set_layout(key, cx)))
        }))
}

/// The music bed's file, gain and the beds to swap in, or the beds to add.
fn music_tools(this: &EditScreen, bar: Div, cx: &mut Context<EditScreen>) -> Div {
    let music = this.folder.read(cx).project.cut.music.clone();
    let current = music.as_ref().map(|m| m.file.clone());
    let beds = music::beds().enumerate().map(|(key, name)| {
        choice(("music-bed", key), name, None)
            .on(current.as_deref() == Some(music::bed_file(name).as_str()))
            .on_click(cx.listener(move |this, _, _, cx| this.add_bed(name, cx)))
    });
    let choose = choice("music-file", "Choose a file…", None)
        .on_click(cx.listener(|this, _, _, cx| this.choose_music(cx)));
    let Some(music) = music else {
        return bar
            .child(
                div()
                    .flex_none()
                    .pr(px(10.))
                    .text_color(MUTED)
                    .child("No music. Add a bed"),
            )
            .children(beds)
            .child(choose);
    };
    let gain = music.gain;
    bar.child(
        div()
            .flex_none()
            .pr(px(10.))
            .text_color(TEXT)
            .child(format!("Music, {}", music::bed_name(&music.file))),
    )
    .child(
        choice("music-quieter", "−", None).on_click(
            cx.listener(move |this, _, _, cx| this.set_music_gain(gain - music::STEP, cx)),
        ),
    )
    .child(
        div()
            .child(music::db(gain))
            .w(px(64.))
            .flex()
            .justify_center(),
    )
    .child(
        choice("music-louder", "+", None).on_click(
            cx.listener(move |this, _, _, cx| this.set_music_gain(gain + music::STEP, cx)),
        ),
    )
    .child(
        label(format!("{} under voice", music::db(music.duck)))
            .flex_none()
            .px(px(8.)),
    )
    .children(beds)
    .child(choose)
    .child(
        tool("tool-remove-music", "close", "Remove", super::DELETE_KEYS).on_click(cx.listener(
            |this, _, _, cx| {
                this.remove_picked(cx);
            },
        )),
    )
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
        .flex_none()
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

/// A toolbar option, lit with [`Lit::on`] when it is the current one.
pub(super) fn choice(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    keys: Option<&'static str>,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(28.))
        .px(px(10.))
        .rounded(RADIUS_SM)
        .bg(RAISED)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT))
        .child(text.into())
        .children(keys.map(kbd))
}

/// Lights the current option of a row of [`choice`]s.
pub(super) trait Lit {
    fn on(self, on: bool) -> Self;
}

impl Lit for Stateful<Div> {
    fn on(self, on: bool) -> Self {
        self.when(on, |s| s.bg(FLOAT_ON).text_color(STRONG))
    }
}

/// A lime band down every lane over the selected words.
fn selection_band(this: &EditScreen, window: &Range<u64>, x: &impl Fn(u64) -> f32) -> Option<Div> {
    let frames = this.selection_frames()?;
    if frames.end <= window.start || frames.start >= window.end {
        return None;
    }
    let (left, right) = (
        x(frames.start.max(window.start)),
        x(frames.end.min(window.end)),
    );
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
    pick: impl Fn(&Id, &shot_core::Instance) -> Option<T>,
) -> Vec<(u64, T)> {
    let lock = this.folder.read(cx).lock();
    let mut out = Vec::new();
    for take in &lock.takes {
        for instance in &take.instances {
            if let Some(value) = pick(&take.take, instance) {
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
/// the playhead there and opens the sound picker above it; the menu of a sound a person placed
/// removes it.
fn sound_lane(
    this: &EditScreen,
    window: &Range<u64>,
    x: &impl Fn(u64) -> f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let sounds = lock_moments(this, window, cx, |take, i| match &i.applied {
        Applied::Sfx { sound, .. } => {
            let placed = i.provenance.layer == shot_core::Layer::Override;
            Some((sound.to_string(), placed.then(|| (take.clone(), i.clone()))))
        }
        _ => None,
    });
    div()
        .id("sound-lane")
        .size_full()
        .cursor_pointer()
        .children(
            sounds
                .into_iter()
                .enumerate()
                .map(|(key, (frame, (name, lock)))| {
                    let title = format!("Sound: {name}");
                    div()
                        .id(("sound", key))
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
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, _, _| {
                                this.lanes.menu = Some(MenuOn::Lock(title.clone(), lock.clone()));
                            }),
                        )
                }),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let frame = this.lanes.frame_at(event.position.x, this.map.frames);
                this.selection = None;
                this.lanes.picked = None;
                this.seek(frame);
                this.add_sound_here(cx);
                cx.notify();
            }),
        )
}

/// The music bed under the whole cut and the lock's music cues, each until the next. A click
/// picks the bed, or the place for one.
fn music_lane(
    this: &EditScreen,
    window: &Range<u64>,
    x: &impl Fn(u64) -> f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let picked = this.lanes.picked == Some(Pick::Music);
    let mut blocks: Vec<(u64, u64, String, bool)> = Vec::new();
    if let Some(bed) = &this.folder.read(cx).project.cut.music {
        blocks.push((0, this.map.frames, music::summary(bed), true));
    }
    let all = lock_moments(this, &(0..this.map.frames), cx, |_, i| match &i.applied {
        Applied::Music { file, .. } => Some(file.clone()),
        _ => None,
    });
    for (index, (frame, file)) in all.iter().enumerate() {
        let end = all
            .get(index + 1)
            .map_or(this.map.frames, |(next, _)| *next);
        blocks.push((*frame, end, format!("{file}, from the style rules"), false));
    }
    let empty = blocks.is_empty();
    let pick = |this: &mut EditScreen| this.pick(Pick::Music);
    div()
        .id("music-lane")
        .size_full()
        .cursor_pointer()
        .when(empty, |lane| {
            lane.child(
                div()
                    .px(px(8.))
                    .text_size(TEXT_XS)
                    .line_height(px(28.))
                    .text_color(if picked { MUTED } else { FAINT })
                    .child("No music, click to add a bed"),
            )
        })
        .children(
            blocks
                .into_iter()
                .filter(|(from, to, ..)| *to > window.start && *from < window.end)
                .map(|(from, to, text, bed)| {
                    block(
                        x(from.max(window.start)),
                        x(to.min(window.end)) - 0.002,
                        if picked && bed { BLOCK_ON } else { RAISED },
                        if picked && bed { TEXT } else { MUTED },
                        text,
                    )
                    .line_height(px(28.))
                    .when(picked && bed, |b| b.border_1().border_color(MUTED))
                }),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                pick(this);
                cx.notify();
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                pick(this);
                this.lanes.menu = Some(MenuOn::Pick);
                cx.notify();
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

fn lane(icon_name: &str, name: &'static str, height: Height, track: impl IntoElement) -> Div {
    let row = div().flex();
    let row = match height {
        Height::Fixed(height) => row.flex_none().h(px(height)),
        Height::Grow {
            natural,
            least,
            weight,
        } => {
            let mut row = row.h(px(natural)).min_h(px(least));
            row.style().flex_grow = Some(weight);
            row.style().flex_shrink = Some(1.);
            row
        }
    };
    row.child(
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
            .h_full()
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
        .truncate()
        .child(text)
}

/// The layout of each stretch, a click picks it for 1 to 4, and the intro and outro.
fn layout_lane(
    this: &EditScreen,
    runs: &[LayoutRun],
    window: &Range<u64>,
    x: &impl Fn(u64) -> f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let picked = match this.lanes.picked {
        Some(Pick::Layout(at, range)) => Some((at, range)),
        _ => None,
    };
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
        .children(
            runs.iter()
                .enumerate()
                .filter(|(_, run)| run.frames.end > window.start && run.frames.start < window.end)
                .map(|(key, run)| {
                    let (at, range) = (run.at, run.range);
                    let on = picked == Some((at, range));
                    block(
                        x(run.frames.start.max(window.start)),
                        x(run.frames.end.min(window.end)) - 0.002,
                        if on { BLOCK_ON } else { BLOCK },
                        if on { TEXT } else { BODY },
                        layout_name(run.layout).to_owned(),
                    )
                    .id(("layout-run", key))
                    .cursor_pointer()
                    .hover(|s| s.bg(BLOCK_ON))
                    .when(on, |b| b.border_1().border_color(MUTED))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.pick(Pick::Layout(at, range));
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.pick(Pick::Layout(at, range));
                            this.lanes.menu = Some(MenuOn::Pick);
                            cx.notify();
                        }),
                    )
                }),
        )
        .children(bookends.map(|(from, to, name)| {
            block(
                x(from.max(window.start)),
                x(to.min(window.end)) - 0.002,
                BLOCK_ON,
                TEXT,
                name.to_owned(),
            )
        }))
        .children(super::switch::marks(this, window, x, cx))
}

/// Every zoom the edit shows, a person's with its index in `cut.kdl`, the lock's with its
/// instance.
fn zoom_items(this: &EditScreen, cx: &Context<EditScreen>) -> Vec<Item> {
    let folder = this.folder.read(cx);
    let mut items = Vec::new();
    for (at, block) in model::blocks(&folder.project.cut) {
        for zoom in &model::zooms(block, folder.lock()) {
            let Some(frames) = this.map.output_range(at, zoom.range) else {
                continue;
            };
            let placed = block.zooms.iter().position(|z| z == zoom).map(Placed::Zoom);
            let lock = match placed {
                Some(_) => None,
                None => model::zoom_instance(folder.lock(), &block.take, zoom)
                    .map(|i| (block.take.clone(), i.clone())),
            };
            items.push(Item {
                at,
                placed,
                lock,
                frames,
                text: format!("{}%", zoom.scale),
            });
        }
    }
    items
}

/// Zooms or overlays on a lane, in two rows where they overlap. A person's can be picked,
/// trimmed by their edges and moved by their middle; the lock's say where they come from.
/// `(id, what)` names the elements and the kind in the menu of a lock's item.
fn item_lane(
    this: &EditScreen,
    (id, what): (&'static str, &'static str),
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
                .min_w(px(6.))
                .rounded(px(6.))
                .px(px(8.))
                .bg(fill)
                .text_color(color)
                .text_size(TEXT_XS)
                .font_weight(FontWeight::MEDIUM)
                .line_height(px(height))
                .truncate()
                .when(picked, |b| b.border_1().border_color(color));
            match item.placed {
                None => {
                    let title = format!("{what}: {}", item.text);
                    let lock = item.lock;
                    element
                        .child(item.text)
                        .tooltip(|window, cx| {
                            Tooltip::new("From the style rules").build(window, cx)
                        })
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, _, _| {
                                this.lanes.menu = Some(MenuOn::Lock(title.clone(), lock.clone()));
                            }),
                        )
                }
                Some(placed) => {
                    let grabbed = (item.at, placed, item.frames.clone());
                    let at = item.at;
                    element
                        .child(item.text)
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
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.pick(Pick::Placed(at, placed));
                                this.lanes.menu = Some(MenuOn::Pick);
                                cx.notify();
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

/// Spoken words placed by time in as many rows as the lane has room for, and the cuts between
/// them as red seams with their length in a strip under the words.
fn words_lane(
    this: &EditScreen,
    window: &Range<u64>,
    x: impl Fn(u64) -> f32,
    track: f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let mut words = Vec::new();
    // Where each word row is free again, in pixels. A word that fits no row shows only
    // through the waveform.
    let mut rows = vec![f32::MIN; this.lanes.word_rows()];
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
                    .top(px(4. + WORD_ROW * row as f32))
                    .left(gpui::relative(x(frame)))
                    .h(px(WORD_HEIGHT))
                    .px(px(3.))
                    .rounded(px(3.))
                    .text_size(TEXT_SM)
                    .line_height(px(WORD_HEIGHT))
                    .whitespace_nowrap()
                    .text_color(BODY)
                    .cursor_pointer()
                    .hover(|w| w.bg(RAISED))
                    .when(word.state != WordState::Kept, |w| {
                        w.text_color(CUT_TEXT).line_through()
                    })
                    .when(selected, |w| {
                        w.bg(super::word_selection()).text_color(STRONG)
                    })
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
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            let inside = this
                                .selection
                                .is_some_and(|s| s.at == at && s.words().contains(&index));
                            if !inside {
                                this.select_word(at, index, false, cx);
                            }
                            this.lanes.menu = Some(MenuOn::Selection);
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
    // Lengths that would run into the one before stay in the seam's tooltip.
    let mut chip_free = f32::MIN;
    let bounds = this.lanes.words.clone();
    div()
        .size_full()
        .child(crate::ui::measure(bounds).absolute().size_full())
        .children(seams.into_iter().enumerate().map(|(key, seam)| {
            let picked = this.lanes.picked == Some(Pick::Cut(seam.at, seam.range));
            let (at, range, frame) = (seam.at, seam.range, seam.frame);
            let tip = seam.removed.clone();
            let chip = seam.length.filter(|text| {
                let left = seam.left * track + 6.;
                let fits = left >= chip_free;
                if fits {
                    chip_free = left + text.chars().count() as f32 * CHIP_CHAR + 8.;
                }
                fits
            });
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
                        this.pick(Pick::Cut(at, range));
                        this.seek(frame);
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.pick(Pick::Cut(at, range));
                        this.seek(frame);
                        this.lanes.menu = Some(MenuOn::Pick);
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .my(px(4.))
                        .w(px(if picked { 4. } else { 2. }))
                        .rounded(px(1.))
                        .bg(if seam.long || picked {
                            Hsla::from(CUT)
                        } else {
                            Hsla::from(FAINT)
                        }),
                )
                .children(chip.map(|text| {
                    div()
                        .absolute()
                        .bottom(px(2.))
                        .left(px(10.))
                        .font_family(MONO)
                        .text_size(px(11.))
                        .line_height(px(CHIP_STRIP - 4.))
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
    /// A long or a person's cut, red with its length.
    long: bool,
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
                let frame = this.map.seam(block.at, run.range)?;
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
                        long: !run.is_seam(),
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
    let folder = this.folder.read(cx);
    let (cut, lock) = (&folder.project.cut, folder.lock());
    let removed_in = |at: shot_core::TakeAt| -> Vec<shot_core::TimeRange> {
        if this.view == model::View::Edited {
            return Vec::new();
        }
        cut.chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .map(|edit| {
                model::removals(edit, lock)
                    .into_iter()
                    .map(|(r, _)| r)
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

/// The range of an overlay after a drag of the copy one block of its take shows. `stored` is the
/// overlay's range, `block` the media that block spans, and the drag moved what the lane showed
/// from `was` to `now`, all media time of the take. The copy is clipped to its block and to the
/// block's cuts. An edge the overlay has in the block goes where the drag put it, so an edge
/// that sat in a cut is stored where it was dropped. An edge outside the block, where the take's
/// other blocks have their copies, moves by as much as the copy's edge did, with a body drag
/// only: a trim of the block's edge changes nothing. Each edge gets its own change, since a cut
/// between the two makes them differ and everything the lane showed covered must be. `None`
/// when the drag changes nothing.
fn dragged_overlay(
    stored: TimeRange,
    block: TimeRange,
    was: TimeRange,
    now: TimeRange,
    grip: Grip,
) -> Option<TimeRange> {
    let by = |stored: Micros, now: Micros, was: Micros| {
        Micros(stored.0.saturating_add_signed(now.0 as i64 - was.0 as i64))
    };
    let from = match (grip, stored.from() >= block.from()) {
        (Grip::End, _) => stored.from(),
        (_, true) => now.from(),
        (Grip::Body, false) => by(stored.from(), now.from(), was.from()),
        (Grip::Start, false) => return None,
    };
    let to = match (grip, stored.to() <= block.to()) {
        (Grip::Start, _) => stored.to(),
        (_, true) => now.to(),
        (Grip::Body, false) => by(stored.to(), now.to(), was.to()),
        (Grip::End, false) => return None,
    };
    let moved = TimeRange::new(from, to)?;
    (moved != stored).then_some(moved)
}

/// Whether `template` belongs on the Highlight lane.
pub fn is_highlight(template: &str) -> bool {
    template == highlight::TEMPLATE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_zoom_grows_evenly_inside_its_take() {
        let range =
            |from, to| TimeRange::new(shot_core::Micros(from), shot_core::Micros(to)).unwrap();
        let take = range(0, 10_000_000);
        let hold = shot_core::Micros(1_500_000);
        assert_eq!(
            at_least(range(4_000_000, 4_600_000), hold, take),
            range(3_550_000, 5_050_000)
        );
        assert_eq!(
            at_least(range(100_000, 700_000), hold, take),
            range(0, 1_500_000)
        );
        assert_eq!(
            at_least(range(9_500_000, 9_900_000), hold, take),
            range(8_500_000, 10_000_000)
        );
        assert_eq!(
            at_least(range(1_000_000, 3_000_000), hold, take),
            range(1_000_000, 3_000_000)
        );
    }

    #[test]
    fn dragging_a_blurs_copy_after_a_splice_keeps_its_copy_before_it() {
        let secs = |from: f64, to: f64| {
            TimeRange::new(Micros((from * 1e6) as u64), Micros((to * 1e6) as u64)).unwrap()
        };
        // The take plays 0 to 4 s and 8 to 12 s around a retake, blurred from 3 s to 11 s.
        let cut = shot_core::Cut::parse(
            "chapter \"Intro\" {\n    take t01 {\n        keep from=0 to=4_000_000\n        overlay blur from=3_000_000 to=11_000_000\n    }\n    take t02 {\n        keep from=4_000_000 to=8_000_000\n    }\n    take t01 {\n        keep from=8_000_000 to=12_000_000\n        overlay blur from=3_000_000 to=11_000_000\n    }\n}\n",
        )
        .unwrap();
        let (stored, block) = (secs(3., 11.), secs(8., 12.));
        // The last block shows 8 to 11 s of it. Its end dragged to 10 s trims the overlay's end.
        let trimmed = dragged_overlay(stored, block, secs(8., 11.), secs(8., 10.), Grip::End);
        assert_eq!(trimmed, Some(secs(3., 10.)));
        let last = TakeAt {
            chapter: 0,
            take: 2,
        };
        let written = cut
            .set_range(last, Placed::Overlay(0), trimmed.unwrap())
            .unwrap();
        let copies: Vec<TimeRange> = written.chapters[0]
            .takes
            .iter()
            .flat_map(|edit| &edit.overlays)
            .map(|o| o.range)
            .collect();
        assert_eq!(copies, [secs(3., 10.), secs(3., 10.)]);
        // The copy's start there is the block's edge, not the overlay's: it does not trim.
        let start = dragged_overlay(stored, block, secs(8., 11.), secs(9., 11.), Grip::Start);
        assert_eq!(start, None);
        // The body shifts both ends, also past the block.
        let shifted = dragged_overlay(stored, block, secs(8., 11.), secs(9., 12.), Grip::Body);
        assert_eq!(shifted, Some(secs(4., 12.)));
        // In the first block the start is the overlay's own.
        let first = dragged_overlay(
            stored,
            secs(0., 4.),
            secs(3., 4.),
            secs(2., 4.),
            Grip::Start,
        );
        assert_eq!(first, Some(secs(2., 11.)));
    }

    #[test]
    fn a_blur_dragged_over_a_cut_covers_what_plays_and_an_edge_in_a_cut_goes_where_dropped() {
        let secs = |from: f64, to: f64| {
            TimeRange::new(Micros((from * 1e6) as u64), Micros((to * 1e6) as u64)).unwrap()
        };
        let block = secs(0., 12.);
        // The block cuts 4 to 8 s. A blur over output 1 to 3 s is dragged to 3 to 5 s, which
        // shows media 3 to 4 s and 8 to 9 s: all of it is blurred.
        let moved = dragged_overlay(secs(1., 3.), block, secs(1., 3.), secs(3., 9.), Grip::Body);
        assert_eq!(moved, Some(secs(3., 9.)));
        // The block cuts 4 to 6 s, so a blur from 5 s shows from 6 s. Its start dragged to 7 s
        // is at 7 s, not a second after where it was stored.
        let stored = secs(5., 9.);
        let trimmed = dragged_overlay(stored, block, secs(6., 9.), secs(7., 9.), Grip::Start);
        assert_eq!(trimmed, Some(secs(7., 9.)));
        let shifted = dragged_overlay(stored, block, secs(6., 9.), secs(7., 10.), Grip::Body);
        assert_eq!(shifted, Some(secs(7., 10.)));
    }

    #[test]
    fn the_zoom_slider_maps_spans_on_a_log_scale_both_ways() {
        let total = 60 * 60 * u64::from(FPS);
        assert_eq!(slider_at(total, total), 0.);
        assert_eq!(slider_at(MIN_SPAN, total), 1.);
        assert_eq!(slider_span(0., total), total);
        assert_eq!(slider_span(1., total), MIN_SPAN);
        // Halfway is the geometric middle, and every span comes back from its position.
        let middle = slider_span(0.5, total);
        assert!(middle.abs_diff(((total * MIN_SPAN) as f64).sqrt() as u64) <= 1);
        for span in [MIN_SPAN, 900, 54_000, total] {
            assert!(slider_span(slider_at(span, total), total).abs_diff(span) <= 1);
        }
        // A video shorter than the closest view has nothing to zoom.
        assert_eq!(slider_at(60, 60), 0.);
    }
}
