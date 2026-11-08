//! The thumbnail studio, opened from the Ship screen's thumbnail card: variants of the video's
//! thumbnail on a 1280x720 canvas, each beside itself at YouTube's small sizes, the real test.
//!
//! A variant takes a frame of the cut (an expression candidate or any moment scrubbed to), the
//! face cut out of it by Vision or framed, a headline in the brand's heading font auto-fitted
//! to its preset's box, and stickers. Every picture comes from `shot-render`'s vector pass on a
//! worker thread, the same drawing the exported `main.png` is, so the canvas is the upload.
//!
//! The design is `thumbnail.kdl` in the video folder, written on every change. ⌘Z and ⇧⌘Z step
//! through the changes made since the studio opened.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use gpui::{
    App, Bounds, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, RenderImage,
    SharedString, Stateful, Subscription, Window, actions, div, img, prelude::*, px, relative,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use shot_core::thumbnail::{
    Backdrop, FaceStyle, FramePick, Headline, HeadlineColor, HeadlineStyle, STICKER_SIZE, Sticker,
    StickerShape, ThumbnailDesign, ThumbnailPreset, Variant,
};
use shot_core::{Id, Micros, TimeRange};
use shot_render::theme::Theme;
use shot_render::thumbnail::{Painter, Picture, Sources};
use shot_ship::thumbnails;

use crate::folder::VideoFolder;
use crate::theme::*;
use crate::ui::{self, ActionLook, Segment, icon, kbd, label, proposal_action, segmented};

pub(crate) const CONTEXT: &str = "ThumbnailStudio";
const FILE: &str = "thumbnail.kdl";
/// The canvas size every variant is laid out on.
const CANVAS: (f32, f32) = (1280., 720.);
/// YouTube's small sizes the compare column shows, in points: search results and up next.
const SMALL: [(&str, f32, f32); 2] = [("Search", 246., 138.), ("Up next", 168., 94.)];
/// Small sizes are rendered at twice their points, for the Retina display they are looked at on.
const DENSITY: f32 = 2.;
/// The emoji the sticker row offers: the bundled animated set.
const EMOJI: [&str; 11] = [
    "🔥", "👀", "😂", "🎉", "👍", "👏", "😍", "🥳", "✅", "✨", "❤",
];
const PRESETS: [(ThumbnailPreset, &str); 4] = [
    (ThumbnailPreset::FaceRight, "Face right"),
    (ThumbnailPreset::FaceLeft, "Face left"),
    (ThumbnailPreset::BigFace, "Big face"),
    (ThumbnailPreset::Screen, "Screen"),
];

actions!(
    thumbnail_studio,
    [
        CloseStudio,
        UndoDesign,
        RedoDesign,
        RemoveSticker,
        PresetOne,
        PresetTwo,
        PresetThree,
        PresetFour,
    ]
);

/// The studio closed; the thumbnail may have changed.
pub struct Closed;

/// A frame's pictures: loading, loaded, or why they could not be.
#[derive(Clone)]
enum Frame {
    Loading,
    Ready(Arc<Sources>),
    Failed(String),
}

/// A variant's renders: the canvas, then [`SMALL`] in order.
#[derive(Default)]
struct Stills {
    canvas: Option<Arc<RenderImage>>,
    small: [Option<Arc<RenderImage>>; 2],
}

enum Job {
    Load(FramePick, PathBuf),
    Render {
        variant: Variant,
        sources: Arc<Sources>,
        theme: Arc<Theme>,
        canvas: bool,
        /// The video folder to write `main.png` into, for the upload variant.
        upload: Option<PathBuf>,
    },
}

enum Done {
    Loaded(FramePick, Result<Sources, String>),
    Rendered {
        name: Id,
        canvas: Option<Arc<RenderImage>>,
        small: [Arc<RenderImage>; 2],
        uploaded: Option<Result<(), String>>,
    },
    /// A render a newer one of the same variant replaced.
    Skipped,
    Failed(String),
}

/// A sticker being dragged on the canvas: where the pointer took it, in canvas pixels.
#[derive(Clone, Copy)]
struct Drag {
    index: usize,
    grab: (f32, f32),
}

pub struct ThumbnailStudio {
    folder: Entity<VideoFolder>,
    dir: PathBuf,
    design: ThumbnailDesign,
    undo: Vec<ThumbnailDesign>,
    redo: Vec<ThumbnailDesign>,
    /// The headline is being typed: one undo step for the whole word run.
    typing: bool,
    selected: usize,
    sticker: Option<usize>,
    drag: Option<Drag>,
    frames: HashMap<FramePick, Frame>,
    stills: HashMap<Id, Stills>,
    /// The upload variant as last written to `main.png`. Opening the studio writes nothing
    /// until it changes, so a thumbnail picked from the candidates stays until then.
    uploaded: Option<Variant>,
    segments: Vec<(Id, TimeRange)>,
    candidates: Vec<(PathBuf, FramePick)>,
    lines: [Entity<InputState>; 2],
    face_size: Entity<SliderState>,
    dim: Entity<SliderState>,
    sticker_size: Entity<SliderState>,
    turn: Entity<SliderState>,
    scrub: Entity<SliderState>,
    /// Where the scrub slider is while it moves, in the cut.
    scrubbing: Option<Micros>,
    /// A slider is moving: one undo step for the whole move.
    sliding: bool,
    canvas: Rc<Cell<Bounds<Pixels>>>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    pending: usize,
    polling: bool,
    note: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Closed> for ThumbnailStudio {}

impl Focusable for ThumbnailStudio {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ThumbnailStudio {
    pub fn new(folder: Entity<VideoFolder>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dir, project) = {
            let folder = folder.read(cx);
            (folder.dir.clone(), folder.project.clone())
        };
        let candidates = candidates(&dir);
        let mut note = None;
        let segments = thumbnails::main_segments(&dir, &project).unwrap_or_else(|e| {
            note = Some(format!("The cut could not be read: {e}"));
            Vec::new()
        });
        let design = match std::fs::read_to_string(dir.join(FILE)) {
            Ok(text) => ThumbnailDesign::parse(&text).unwrap_or_else(|e| {
                note = Some(format!(
                    "{FILE} did not parse ({e}), so the studio starts over."
                ));
                ThumbnailDesign::default()
            }),
            Err(_) => ThumbnailDesign::default(),
        };
        let design = if design.variants.is_empty() {
            first_design(&project.video.title, &candidates, &segments)
        } else {
            design
        };
        let mut input = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let lines = [input("Headline", cx), input("Second line", cx)];
        let slider = |min: f32, max: f32, step: f32, cx: &mut Context<Self>| {
            cx.new(|_| SliderState::new().min(min).max(max).step(step))
        };
        let face_size = slider(40., 200., 5., cx);
        let dim = slider(0., f32::from(shot_core::thumbnail::MAX_DIM), 5., cx);
        let sticker_size = slider(
            f32::from(*STICKER_SIZE.start()),
            f32::from(*STICKER_SIZE.end()),
            4.,
            cx,
        );
        let turn = slider(-180., 180., 5., cx);
        let length = segments.iter().map(|(_, r)| r.duration().0).sum::<u64>();
        let scrub = slider(0., (length as f32 / 1e6).max(1.), 0.1, cx);

        let mut subscriptions = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                line,
                window,
                move |this, state, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        let text = state.read(cx).value().to_string();
                        this.set_line(index, text, cx);
                    }
                    InputEvent::Blur => this.typing = false,
                    _ => {}
                },
            ));
        }
        let mut slide =
            |slider: &Entity<SliderState>, apply: fn(&mut Self, f32, &mut Context<Self>)| {
                subscriptions.push(cx.subscribe(
                    slider,
                    move |this, _, event: &SliderEvent, cx| match *event {
                        SliderEvent::Change(value) => {
                            if !this.sliding {
                                this.sliding = true;
                                this.checkpoint();
                            }
                            apply(this, value.start(), cx);
                        }
                        SliderEvent::Release(value) => {
                            apply(this, value.start(), cx);
                            this.sliding = false;
                            this.save();
                        }
                    },
                ));
            };
        slide(&face_size, |this, v, cx| {
            this.live(cx, |variant| variant.face_size = v.round() as u16)
        });
        slide(&dim, |this, v, cx| {
            this.live(cx, |variant| variant.dim = v.round() as u8)
        });
        slide(&sticker_size, |this, v, cx| {
            this.live_sticker(cx, |sticker| sticker.size = v.round() as u16)
        });
        slide(&turn, |this, v, cx| {
            this.live_sticker(cx, |sticker| sticker.turn = v.round() as i16)
        });
        subscriptions.push(
            cx.subscribe(&scrub, |this, _, event: &SliderEvent, cx| match *event {
                SliderEvent::Change(value) => {
                    this.scrubbing = Some(Micros((value.start() * 1e6) as u64));
                    cx.notify();
                }
                SliderEvent::Release(value) => {
                    this.scrubbing = None;
                    let at = Micros((value.start() * 1e6) as u64);
                    if let Some((take, at)) = thumbnails::moment(&this.segments, at) {
                        this.set_frame(FramePick { take, at }, cx);
                    }
                }
            }),
        );
        subscriptions.push(cx.observe(&folder, |this, _, cx| this.render_all(cx)));

        let (jobs, rx) = mpsc::channel();
        let (send, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("thumbnail studio".into())
            .spawn(move || work(&rx, &send))
            .expect("spawning the thumbnail worker");
        let mut studio = Self {
            folder,
            dir,
            design,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            selected: 0,
            sticker: None,
            drag: None,
            frames: HashMap::new(),
            stills: HashMap::new(),
            uploaded: None,
            segments,
            candidates,
            lines,
            face_size,
            dim,
            sticker_size,
            turn,
            scrub,
            scrubbing: None,
            sliding: false,
            canvas: Rc::default(),
            jobs,
            done,
            pending: 0,
            polling: false,
            note,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        if shot_ship::ship::thumbnail(&studio.dir).is_some() {
            studio.uploaded = studio.design.uploaded().cloned();
        }
        studio.selected = studio
            .design
            .upload
            .as_ref()
            .and_then(|name| studio.design.variants.iter().position(|v| &v.name == name))
            .unwrap_or(0);
        studio.save();
        studio.sync_controls(window, cx);
        studio.render_all(cx);
        studio.focus.focus(window, cx);
        studio
    }

    fn variant(&self) -> &Variant {
        &self.design.variants[self.selected]
    }

    fn variant_mut(&mut self) -> &mut Variant {
        &mut self.design.variants[self.selected]
    }

    /// Remembers the design before a change, for ⌘Z.
    fn checkpoint(&mut self) {
        self.undo.push(self.design.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// One undoable change to the selected variant, saved and drawn.
    fn change(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Variant)) {
        self.typing = false;
        self.checkpoint();
        edit(self.variant_mut());
        self.save();
        self.render_all(cx);
    }

    /// A step of a slider or a drag: drawn now, saved and undone with the whole move.
    fn live(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Variant)) {
        edit(self.variant_mut());
        self.render_all(cx);
    }

    fn live_sticker(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Sticker)) {
        if let Some(index) = self.sticker {
            self.live(cx, |variant| {
                if let Some(sticker) = variant.stickers.get_mut(index) {
                    edit(sticker);
                }
            });
        }
    }

    /// Writes `thumbnail.kdl`.
    fn save(&mut self) {
        let path = self.dir.join(FILE);
        if let Err(e) = std::fs::write(&path, self.design.to_kdl()) {
            self.note = Some(format!("The design could not be saved: {e}"));
        }
    }

    fn set_line(&mut self, index: usize, text: String, cx: &mut Context<Self>) {
        if !self.typing {
            self.typing = true;
            self.checkpoint();
        }
        let variant = self.variant_mut();
        let mut lines = variant
            .headline
            .as_ref()
            .map(|h| h.lines.clone())
            .unwrap_or_default();
        lines.resize(2, String::new());
        lines[index] = text;
        let lines: Vec<String> = lines
            .into_iter()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect();
        let (color, style) = variant
            .headline
            .as_ref()
            .map_or((HeadlineColor::Both, HeadlineStyle::Outline), |h| {
                (h.color, h.style)
            });
        variant.headline = (!lines.is_empty()).then_some(Headline {
            lines,
            color,
            style,
        });
        self.save();
        self.render_all(cx);
    }

    fn set_frame(&mut self, frame: FramePick, cx: &mut Context<Self>) {
        self.change(cx, |variant| variant.frame = Some(frame));
    }

    fn set_preset(&mut self, preset: ThumbnailPreset, window: &mut Window, cx: &mut Context<Self>) {
        self.change(cx, |variant| variant.apply(preset));
        self.sync_controls(window, cx);
    }

    fn add_sticker(
        &mut self,
        shape: StickerShape,
        emoji: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Where each sticker starts: the arrow pointing at the face, the ring around it, an
        // emoji in a corner the face leaves free.
        let (x, y, turn) = match (shape, self.variant().preset) {
            (StickerShape::Arrow, ThumbnailPreset::FaceRight) => (600, 560, -10),
            (StickerShape::Arrow, ThumbnailPreset::FaceLeft) => (680, 560, 190),
            (StickerShape::Arrow, _) => (640, 360, 0),
            (StickerShape::Ring, ThumbnailPreset::FaceRight) => (950, 300, 0),
            (StickerShape::Ring, ThumbnailPreset::FaceLeft) => (330, 300, 0),
            (StickerShape::Ring, _) => (640, 300, 0),
            (StickerShape::Emoji, ThumbnailPreset::FaceLeft | ThumbnailPreset::Screen) => {
                (110, 110, 0)
            }
            (StickerShape::Emoji, _) => (1170, 110, 0),
        };
        let size = match shape {
            StickerShape::Arrow => 140,
            StickerShape::Ring => 220,
            StickerShape::Emoji => 150,
        };
        let sticker = Sticker {
            shape,
            emoji: emoji.map(str::to_owned),
            x,
            y,
            size,
            turn,
        };
        self.change(cx, |variant| variant.stickers.push(sticker));
        self.sticker = Some(self.variant().stickers.len() - 1);
        self.sync_controls(window, cx);
    }

    fn remove_sticker(&mut self, _: &RemoveSticker, window: &mut Window, cx: &mut Context<Self>) {
        // Backspace in a headline field edits the text.
        if !self.focus.is_focused(window) {
            return;
        }
        if let Some(index) = self.sticker.take() {
            self.change(cx, |variant| {
                if index < variant.stickers.len() {
                    variant.stickers.remove(index);
                }
            });
        }
    }

    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.selected {
            return;
        }
        self.selected = index;
        self.sticker = None;
        self.typing = false;
        self.sync_controls(window, cx);
        self.render_all(cx);
    }

    fn new_variant(&mut self, duplicate: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.typing = false;
        self.checkpoint();
        let name = self.design.free_name();
        let current = self.variant().clone();
        let variant = if duplicate {
            Variant { name, ..current }
        } else {
            // The next preset, so a new variant is a different idea from the start.
            let next = PRESETS
                .iter()
                .position(|(p, _)| *p == current.preset)
                .map_or(PRESETS[0].0, |i| PRESETS[(i + 1) % PRESETS.len()].0);
            Variant {
                frame: current.frame.clone(),
                headline: current.headline.clone(),
                ..Variant::new(name, next)
            }
        };
        self.design.variants.push(variant);
        self.selected = self.design.variants.len() - 1;
        self.sticker = None;
        self.save();
        self.sync_controls(window, cx);
        self.render_all(cx);
    }

    fn remove_variant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.design.variants.len() < 2 {
            return;
        }
        self.typing = false;
        self.checkpoint();
        let removed = self.design.variants.remove(self.selected);
        if self.design.upload.as_ref() == Some(&removed.name) {
            self.design.upload = Some(self.design.variants[0].name.clone());
        }
        self.selected = self.selected.min(self.design.variants.len() - 1);
        self.sticker = None;
        self.save();
        self.sync_controls(window, cx);
        self.render_all(cx);
    }

    fn set_upload(&mut self, name: Id, cx: &mut Context<Self>) {
        self.typing = false;
        self.checkpoint();
        self.design.upload = Some(name);
        self.save();
        self.render_all(cx);
    }

    fn undo(&mut self, _: &UndoDesign, window: &mut Window, cx: &mut Context<Self>) {
        // The headline fields keep their own undo while one has focus.
        if !self.focus.is_focused(window) && self.typing {
            return;
        }
        if let Some(previous) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.design, previous));
            self.restored(window, cx);
        }
    }

    fn redo(&mut self, _: &RedoDesign, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.design, next));
            self.restored(window, cx);
        }
    }

    fn restored(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.typing = false;
        self.selected = self.selected.min(self.design.variants.len() - 1);
        self.sticker = self.sticker.filter(|&i| i < self.variant().stickers.len());
        self.save();
        self.sync_controls(window, cx);
        self.render_all(cx);
    }

    /// Puts the selected variant's values into the fields and sliders.
    fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let variant = self.variant().clone();
        let lines = variant
            .headline
            .as_ref()
            .map(|h| h.lines.clone())
            .unwrap_or_default();
        for (index, input) in self.lines.iter().enumerate() {
            let text = lines.get(index).cloned().unwrap_or_default();
            input.update(cx, |input, cx| {
                if input.value().as_ref() != text {
                    input.set_value(text, window, cx);
                }
            });
        }
        let set = |slider: &Entity<SliderState>, value: f32, window: &mut Window, cx: &mut App| {
            slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
        };
        set(&self.face_size, f32::from(variant.face_size), window, cx);
        set(&self.dim, f32::from(variant.dim), window, cx);
        if let Some(sticker) = self.sticker.and_then(|i| variant.stickers.get(i)) {
            set(&self.sticker_size, f32::from(sticker.size), window, cx);
            set(&self.turn, f32::from(sticker.turn), window, cx);
        }
        if let Some(at) = variant
            .frame
            .as_ref()
            .and_then(|f| thumbnails::cut_time(&self.segments, &f.take, f.at))
        {
            set(&self.scrub, at.0 as f32 / 1e6, window, cx);
        }
        cx.notify();
    }

    /// Asks the worker for every variant's pictures, and for the frames they need.
    fn render_all(&mut self, cx: &mut Context<Self>) {
        let dir = self.dir.clone();
        let theme = self
            .folder
            .read(cx)
            .theme
            .clone()
            .unwrap_or_else(|_| Arc::default());
        for index in 0..self.design.variants.len() {
            let variant = self.design.variants[index].clone();
            let frame = match &variant.frame {
                None => Some(Arc::new(Sources::default())),
                Some(pick) => match self.frames.get(pick) {
                    Some(Frame::Ready(sources)) => Some(sources.clone()),
                    Some(Frame::Failed(_)) => Some(Arc::new(Sources::default())),
                    Some(Frame::Loading) => None,
                    None => {
                        self.frames.insert(pick.clone(), Frame::Loading);
                        self.send(Job::Load(pick.clone(), dir.clone()), cx);
                        None
                    }
                },
            };
            let Some(sources) = frame else {
                continue;
            };
            // The frame is loaded here, so the written thumbnail has its pictures.
            let upload = (self.design.upload.as_ref() == Some(&variant.name)
                && self.uploaded.as_ref() != Some(&variant))
            .then(|| {
                self.uploaded = Some(variant.clone());
                dir.clone()
            });
            self.send(
                Job::Render {
                    canvas: index == self.selected,
                    variant,
                    sources,
                    theme: theme.clone(),
                    upload,
                },
                cx,
            );
        }
        cx.notify();
    }

    fn send(&mut self, job: Job, cx: &mut Context<Self>) {
        if self.jobs.send(job).is_ok() {
            self.pending += 1;
            self.poll(cx);
        }
    }

    /// Takes the worker's results while it has work.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let Ok(busy) = this.update(cx, |this, cx| {
                    while let Ok(done) = this.done.try_recv() {
                        this.pending = this.pending.saturating_sub(1);
                        this.finished(done, cx);
                    }
                    this.polling = this.pending > 0;
                    this.polling
                }) else {
                    break;
                };
                if !busy {
                    break;
                }
            }
        })
        .detach();
    }

    fn finished(&mut self, done: Done, cx: &mut Context<Self>) {
        match done {
            Done::Loaded(pick, result) => {
                let frame = match result {
                    Ok(sources) => Frame::Ready(Arc::new(sources)),
                    Err(e) => Frame::Failed(e),
                };
                self.frames.insert(pick, frame);
                self.render_all(cx);
            }
            Done::Rendered {
                name,
                canvas,
                small,
                uploaded,
            } => {
                let stills = self.stills.entry(name).or_default();
                if canvas.is_some() {
                    stills.canvas = canvas;
                }
                stills.small = small.map(Some);
                if let Some(Err(e)) = uploaded {
                    self.note = Some(format!("The thumbnail could not be written: {e}"));
                }
            }
            Done::Skipped => {}
            Done::Failed(e) => self.note = Some(e),
        }
        cx.notify();
    }

    /// Where `position` falls on the canvas, in canvas pixels.
    fn canvas_point(&self, position: gpui::Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.canvas.get();
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if w <= 0. || h <= 0. {
            return None;
        }
        let x = (f32::from(position.x) - f32::from(bounds.origin.x)) / w * CANVAS.0;
        let y = (f32::from(position.y) - f32::from(bounds.origin.y)) / h * CANVAS.1;
        Some((x, y))
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let Some((x, y)) = self.canvas_point(event.position) else {
            return;
        };
        // The topmost sticker under the pointer.
        let hit = self
            .variant()
            .stickers
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| {
                let (w, h) = sticker_box(s);
                (x - f32::from(s.x)).abs() <= w / 2. && (y - f32::from(s.y)).abs() <= h / 2.
            })
            .map(|(i, s)| (i, (x - f32::from(s.x), y - f32::from(s.y))));
        self.sticker = hit.map(|(i, _)| i);
        self.drag = hit.map(|(index, grab)| Drag { index, grab });
        if self.drag.is_some() {
            self.checkpoint();
        }
        self.sync_controls(window, cx);
    }

    fn moved(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            self.save();
            return;
        }
        let Some((x, y)) = self.canvas_point(event.position) else {
            return;
        };
        self.live(cx, |variant| {
            if let Some(sticker) = variant.stickers.get_mut(drag.index) {
                sticker.x = (x - drag.grab.0).round().clamp(-200., 1480.) as i16;
                sticker.y = (y - drag.grab.1).round().clamp(-200., 920.) as i16;
            }
        });
    }

    fn released(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.save();
            cx.notify();
        }
    }
}

/// The variant's sticker under the pointer: how much of the canvas it covers.
fn sticker_box(sticker: &Sticker) -> (f32, f32) {
    let size = f32::from(sticker.size);
    match sticker.shape {
        StickerShape::Arrow => (size * 2., size),
        StickerShape::Ring | StickerShape::Emoji => (size, size),
    }
}

/// The expression candidates of the last search, best first, with the moment each shows.
fn candidates(dir: &Path) -> Vec<(PathBuf, FramePick)> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir.join(".shot/cache/thumbnails"))
        .into_iter()
        .flatten()
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    found.sort();
    found
        .into_iter()
        .filter_map(|path| {
            let (take, at) = thumbnails::candidate_moment(&path)?;
            Some((path, FramePick { take, at }))
        })
        .take(6)
        .collect()
}

/// A first design: the face on the right of the best candidate, the title as the headline.
fn first_design(
    title: &str,
    candidates: &[(PathBuf, FramePick)],
    segments: &[(Id, TimeRange)],
) -> ThumbnailDesign {
    let mut design = ThumbnailDesign::default();
    let name = design.free_name();
    let mut variant = Variant::new(name.clone(), ThumbnailPreset::FaceRight);
    variant.frame = candidates
        .first()
        .map(|(_, pick)| pick.clone())
        .or_else(|| {
            let length: u64 = segments.iter().map(|(_, r)| r.duration().0).sum();
            thumbnails::moment(segments, Micros(length / 3))
                .map(|(take, at)| FramePick { take, at })
        });
    let lines = headline_lines(title);
    variant.headline = (!lines.is_empty()).then_some(Headline {
        lines,
        color: HeadlineColor::Both,
        style: HeadlineStyle::Outline,
    });
    design.variants.push(variant);
    design.upload = Some(name);
    design
}

/// `title` as one line, or two of about equal length once it is longer than four words.
fn headline_lines(title: &str) -> Vec<String> {
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    if words.len() <= 4 {
        return vec![words.join(" ")];
    }
    let total: usize = words.iter().map(|w| w.len() + 1).sum();
    let mut run = 0;
    let split = words
        .iter()
        .position(|w| {
            run += w.len() + 1;
            run * 2 >= total
        })
        .map_or(words.len() / 2, |i| i + 1)
        .min(words.len() - 1);
    vec![words[..split].join(" "), words[split..].join(" ")]
}

/// The worker: loads frames and renders variants, the newest render of each variant only.
fn work(jobs: &Receiver<Job>, done: &Sender<Done>) {
    let mut renderer: Option<shot_render::graph::Renderer> = None;
    let mut painter = Painter::default();
    while let Ok(first) = jobs.recv() {
        // Everything queued meanwhile: frames first, then the last render of each variant.
        let mut queued = vec![first];
        queued.extend(jobs.try_iter());
        let mut renders: Vec<Job> = Vec::new();
        for job in queued {
            match job {
                Job::Load(pick, dir) => {
                    let result =
                        thumbnails::sources(&dir, &pick.take, pick.at).map_err(|e| e.to_string());
                    let _ = done.send(Done::Loaded(pick, result));
                }
                Job::Render { .. } => {
                    let name = |job: &Job| match job {
                        Job::Render { variant, .. } => Some(variant.name.clone()),
                        Job::Load(..) => None,
                    };
                    let mine = name(&job);
                    match renders.iter().position(|r| name(r) == mine) {
                        Some(at) => {
                            let earlier = std::mem::replace(&mut renders[at], job);
                            if let (
                                Job::Render {
                                    canvas: drew,
                                    upload: wrote,
                                    ..
                                },
                                Job::Render { canvas, upload, .. },
                            ) = (earlier, &mut renders[at])
                            {
                                *canvas |= drew;
                                if upload.is_none() {
                                    *upload = wrote;
                                }
                            }
                            let _ = done.send(Done::Skipped);
                        }
                        None => renders.push(job),
                    }
                }
            }
        }
        for job in renders {
            let Job::Render {
                variant,
                sources,
                theme,
                canvas,
                upload,
            } = job
            else {
                continue;
            };
            if renderer.is_none() {
                match shot_render::graph::Renderer::new() {
                    Ok(made) => renderer = Some(made),
                    Err(e) => {
                        let _ = done.send(Done::Failed(format!("No GPU to draw with: {e}")));
                        continue;
                    }
                }
            }
            let renderer = renderer.as_ref().expect("made above");
            let mut draw =
                |size: (u32, u32)| painter.render(renderer, &variant, &theme, &sources, size);
            let full = if canvas || upload.is_some() {
                draw((CANVAS.0 as u32, CANVAS.1 as u32)).map(Some)
            } else {
                Ok(None)
            };
            let small: Result<Vec<Picture>, _> = SMALL
                .iter()
                .map(|(_, w, h)| draw(((w * DENSITY) as u32, (h * DENSITY) as u32)))
                .collect();
            let (full, small) = match (full, small) {
                (Ok(full), Ok(small)) => (full, small),
                (Err(e), _) | (_, Err(e)) => {
                    let _ = done.send(Done::Failed(format!("Drawing the thumbnail failed: {e}")));
                    continue;
                }
            };
            let uploaded = match (&upload, &full) {
                (Some(dir), Some(still)) => Some(
                    thumbnails::write_upload(dir, still)
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                ),
                _ => None,
            };
            let _ = done.send(Done::Rendered {
                name: variant.name.clone(),
                canvas: full.filter(|_| canvas).map(|p| render_image(&p)),
                small: [render_image(&small[0]), render_image(&small[1])],
                uploaded,
            });
        }
    }
}

/// A picture as gpui draws it: BGRA.
fn render_image(picture: &Picture) -> Arc<RenderImage> {
    let mut bgra = picture.rgba.to_vec();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(picture.width, picture.height, bgra)
        .expect("width * height pixels");
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

impl Render for ThumbnailStudio {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("thumbnail-studio")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &CloseStudio, _, cx| cx.emit(Closed)))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::remove_sticker))
            .on_action(
                cx.listener(|this, _: &PresetOne, w, cx| this.set_preset(PRESETS[0].0, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PresetTwo, w, cx| this.set_preset(PRESETS[1].0, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PresetThree, w, cx| this.set_preset(PRESETS[2].0, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PresetFour, w, cx| this.set_preset(PRESETS[3].0, w, cx)),
            )
            .size_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(8.))
            .pt(px(0.))
            .child(self.header(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(8.))
                    .child(self.inspector(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(self.stage(cx))
                            .child(self.frames_strip(cx)),
                    )
                    .child(self.compare(cx)),
            )
    }
}

impl ThumbnailStudio {
    fn header(&self, cx: &mut Context<Self>) -> Div {
        ui::pane()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(16.))
            .py(px(10.))
            .child(icon("picture", ICON, TEXT))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Thumbnail studio"),
                    )
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(
                        "Saved with the video. The variant marked Upload is the thumbnail Ship \
                         uploads.",
                    )),
            )
            .child(div().flex_1())
            .when_some(self.note.clone().filter(|n| !n.is_empty()), |row, note| {
                row.child(
                    div()
                        .max_w(px(420.))
                        .text_size(TEXT_SM)
                        .text_color(WARN)
                        .child(note),
                )
            })
            .when(self.pending > 0, |row| {
                row.child(div().text_size(TEXT_SM).text_color(MUTED).child("Drawing"))
            })
            .child(ui::hint("⌘Z", "Undo"))
            .child(
                proposal_action("studio-done", "Done", None, Some("Esc"), ActionLook::Accent)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Closed))),
            )
    }

    fn inspector(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let variant = self.variant().clone();
        let headline = variant.headline.clone();
        let frame = variant
            .frame
            .as_ref()
            .and_then(|f| self.frames.get(f))
            .cloned();
        let sources = match &frame {
            Some(Frame::Ready(sources)) => Some(sources.clone()),
            _ => None,
        };
        let face_note = match (&sources, variant.face) {
            (_, FaceStyle::Off) => None,
            (Some(s), _) if s.camera.is_none() => {
                Some("This take has no camera, so there is no face to show.")
            }
            (Some(s), FaceStyle::Cutout) if s.cutout.is_none() => {
                Some("Vision found no person in this frame, so the face shows framed.")
            }
            _ => None,
        };
        let backdrop_note = match (&sources, variant.backdrop) {
            (Some(s), Backdrop::Screen) if s.screen.is_none() => {
                Some("This take has no screen, so the brand color shows.")
            }
            (Some(s), Backdrop::Camera) if s.camera.is_none() => {
                Some("This take has no camera, so the brand color shows.")
            }
            _ => None,
        };
        let presets =
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(PRESETS.iter().enumerate().map(|(index, &(preset, name))| {
                    let on = variant.preset == preset;
                    ui::keyboard_reachable(
                        div()
                            .id(("studio-preset", index))
                            .w(px(126.))
                            .h(px(54.))
                            .p(px(8.))
                            .flex()
                            .flex_col()
                            .justify_between()
                            .rounded(RADIUS_MD)
                            .bg(if on { FLOAT } else { RAISED })
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT)),
                    )
                    .when(on, |tile| tile.border_color(ACCENT))
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if on { STRONG } else { TEXT })
                            .child(name),
                    )
                    .child(kbd(format!("⌥{}", index + 1)))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.set_preset(preset, window, cx)),
                    )
                }));
        let color = headline.as_ref().map(|h| h.color);
        let style = headline.as_ref().map(|h| h.style);
        let headline_color = segmented(
            "studio-headline-color",
            WELL,
            [
                (HeadlineColor::Text, "Text"),
                (HeadlineColor::Accent, "Accent"),
                (HeadlineColor::Both, "Both"),
            ]
            .into_iter()
            .map(|(value, name)| {
                self.segment(name, color == Some(value), cx, move |this, cx| {
                    this.change(cx, |v| {
                        if let Some(h) = &mut v.headline {
                            h.color = value;
                        }
                    })
                })
            })
            .collect(),
        );
        let headline_style = segmented(
            "studio-headline-style",
            WELL,
            [
                (HeadlineStyle::Outline, "Outline"),
                (HeadlineStyle::Shadow, "Shadow"),
            ]
            .into_iter()
            .map(|(value, name)| {
                self.segment(name, style == Some(value), cx, move |this, cx| {
                    this.change(cx, |v| {
                        if let Some(h) = &mut v.headline {
                            h.style = value;
                        }
                    })
                })
            })
            .collect(),
        );
        let face = segmented(
            "studio-face",
            WELL,
            [
                (FaceStyle::Cutout, "Cut out"),
                (FaceStyle::Framed, "Framed"),
                (FaceStyle::Off, "Off"),
            ]
            .into_iter()
            .map(|(value, name)| {
                self.segment(name, variant.face == value, cx, move |this, cx| {
                    this.change(cx, |v| v.face = value)
                })
            })
            .collect(),
        );
        let backdrop = segmented(
            "studio-backdrop",
            WELL,
            [
                (Backdrop::Screen, "Screen"),
                (Backdrop::Camera, "Camera"),
                (Backdrop::Brand, "Brand"),
            ]
            .into_iter()
            .map(|(value, name)| {
                self.segment(name, variant.backdrop == value, cx, move |this, cx| {
                    this.change(cx, |v| v.backdrop = value)
                })
            })
            .collect(),
        );
        let add = |id: &'static str, text: &'static str, shape: StickerShape| {
            proposal_action(id, text, None, None, ActionLook::Plain).on_click(
                cx.listener(move |this, _, window, cx| this.add_sticker(shape, None, window, cx)),
            )
        };
        let stickers = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(add("studio-arrow", "Arrow", StickerShape::Arrow))
                    .child(add("studio-ring", "Ring", StickerShape::Ring)),
            )
            .child(
                div().flex().flex_wrap().gap(px(4.)).children(
                    EMOJI
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| shot_render::overlay::pop::animated(e).is_some())
                        .map(|(index, &emoji)| {
                            ui::keyboard_reachable(
                                div()
                                    .id(("studio-emoji", index))
                                    .size(px(34.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(RADIUS_SM)
                                    .bg(RAISED)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(FLOAT))
                                    .text_size(px(18.)),
                            )
                            .child(emoji)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.add_sticker(StickerShape::Emoji, Some(emoji), window, cx)
                                },
                            ))
                        }),
                ),
            )
            .when_some(
                self.sticker.and_then(|i| variant.stickers.get(i)).cloned(),
                |col, sticker| {
                    col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .p(px(8.))
                            .rounded(RADIUS_MD)
                            .bg(CARD)
                            .child(slider_row(
                                "Size",
                                &self.sticker_size,
                                format!("{} px", sticker.size),
                            ))
                            .child(slider_row("Turn", &self.turn, format!("{}°", sticker.turn)))
                            .child(
                                div().flex().justify_end().pt(px(4.)).child(
                                    proposal_action(
                                        "studio-remove-sticker",
                                        "Remove",
                                        None,
                                        Some("⌫"),
                                        ActionLook::Quiet,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.focus.focus(window, cx);
                                            this.remove_sticker(&RemoveSticker, window, cx)
                                        },
                                    )),
                                ),
                            ),
                    )
                },
            )
            .when(
                self.sticker.is_none() && !variant.stickers.is_empty(),
                |col| {
                    col.child(note(
                        "Click a sticker on the canvas to move, size or turn it.",
                    ))
                },
            );
        ui::pane()
            .id("studio-inspector")
            .w(px(300.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(14.))
            .overflow_y_scroll()
            .child(section("Layout"))
            .child(presets)
            .child(section("Headline"))
            .child(field(&self.lines[0]))
            .child(field(&self.lines[1]))
            .when(headline.is_some(), |col| {
                col.child(headline_color.w_full())
                    .child(headline_style.w_full())
            })
            .child(section("Face"))
            .child(face.w_full())
            .when(variant.face != FaceStyle::Off, |col| {
                col.child(slider_row(
                    "Size",
                    &self.face_size,
                    format!("{}%", variant.face_size),
                ))
            })
            .children(face_note.map(note))
            .child(section("Background"))
            .child(backdrop.w_full())
            .child(slider_row("Dim", &self.dim, format!("{}%", variant.dim)))
            .children(backdrop_note.map(note))
            .child(section("Stickers"))
            .child(stickers)
    }

    fn segment(
        &self,
        text: &'static str,
        on: bool,
        cx: &mut Context<Self>,
        apply: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Segment {
        let this = cx.entity().downgrade();
        Segment::new(text, on, move |_: &ClickEvent, _, cx: &mut App| {
            let _ = this.update(cx, |studio, cx| apply(studio, cx));
        })
    }

    /// The canvas: the selected variant at its full size, fitted into the space, with the
    /// selected sticker outlined. Stickers drag with the mouse.
    fn stage(&self, cx: &mut Context<Self>) -> Div {
        let still = self
            .stills
            .get(&self.variant().name)
            .and_then(|s| s.canvas.clone());
        let selected = self
            .sticker
            .and_then(|i| self.variant().stickers.get(i))
            .map(|s| {
                let (w, h) = sticker_box(s);
                (
                    (f32::from(s.x) - w / 2.) / CANVAS.0,
                    (f32::from(s.y) - h / 2.) / CANVAS.1,
                    w / CANVAS.0,
                    h / CANVAS.1,
                )
            });
        ui::pane()
            .flex_1()
            .min_h_0()
            .p(px(14.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("studio-canvas")
                    .relative()
                    .w_full()
                    .max_w(px(1280.))
                    .aspect_ratio(CANVAS.0 / CANVAS.1)
                    .rounded(RADIUS_MD)
                    .overflow_hidden()
                    .bg(WELL)
                    .cursor_pointer()
                    .child(ui::measure(self.canvas.clone()).absolute().size_full())
                    .when_some(still, |canvas, still| {
                        canvas.child(img(still).size_full().object_fit(ObjectFit::Fill))
                    })
                    .when_some(selected, |canvas, (left, top, width, height)| {
                        canvas.child(
                            div()
                                .absolute()
                                .left(relative(left))
                                .top(relative(top))
                                .w(relative(width))
                                .h(relative(height))
                                .rounded(RADIUS_SM)
                                .border_2()
                                .border_color(Hsla::from(ACCENT).opacity(0.9)),
                        )
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
                    .on_mouse_move(cx.listener(Self::moved))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.released(cx)),
                    ),
            )
    }

    /// The frame strip: the expression candidates, and the whole cut to scrub through.
    fn frames_strip(&self, cx: &mut Context<Self>) -> Div {
        let current = self.variant().frame.clone();
        let length = Micros(self.segments.iter().map(|(_, r)| r.duration().0).sum());
        let at = self.scrubbing.or_else(|| {
            current
                .as_ref()
                .and_then(|f| thumbnails::cut_time(&self.segments, &f.take, f.at))
        });
        let loading = matches!(
            current.as_ref().and_then(|f| self.frames.get(f)),
            Some(Frame::Loading)
        );
        let failed = match current.as_ref().and_then(|f| self.frames.get(f)) {
            Some(Frame::Failed(e)) => Some(e.clone()),
            _ => None,
        };
        let where_ = match (&current, at) {
            (Some(f), _) if let Some(e) = &failed => {
                format!("Take {} could not be read: {e}", f.take)
            }
            (None, _) => "No frame yet. Pick a candidate or scrub to a moment.".to_owned(),
            (Some(f), Some(at)) => format!(
                "Take {} at {} of the cut{}",
                f.take,
                clock(at),
                if loading {
                    ", cutting out the face"
                } else {
                    ""
                }
            ),
            (Some(f), None) => format!("Take {}, a moment the cut leaves out", f.take),
        };
        ui::pane()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(label("Frame"))
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(where_)),
            )
            .when(!self.candidates.is_empty(), |strip| {
                strip.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .children(self.candidates.iter().enumerate().map(
                            |(index, (path, pick))| {
                                let on = current.as_ref() == Some(pick);
                                let pick = pick.clone();
                                ui::keyboard_reachable(
                                    div()
                                        .id(("studio-candidate", index))
                                        .w(px(96.))
                                        .h(px(54.))
                                        .rounded(px(7.))
                                        .overflow_hidden()
                                        .bg(BLOCK)
                                        .cursor_pointer(),
                                )
                                .border_2()
                                .border_color(if on {
                                    Hsla::from(ACCENT)
                                } else {
                                    gpui::transparent_black()
                                })
                                .hover(|s| s.border_color(Hsla::from(ACCENT).opacity(0.4)))
                                .child(img(path.clone()).size_full().object_fit(ObjectFit::Cover))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.set_frame(pick.clone(), cx),
                                ))
                            },
                        )),
                )
            })
            .when(self.candidates.is_empty(), |strip| {
                strip.child(note(
                    "Find candidates on the Ship screen to see the best expressions here.",
                ))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        Slider::new(&self.scrub)
                            .accessibility_label("Moment of the cut")
                            .flex_1(),
                    )
                    .child(
                        ui::mono(format!(
                            "{} / {}",
                            clock(at.unwrap_or_default()),
                            clock(length)
                        ))
                        .text_size(TEXT_SM)
                        .text_color(MUTED)
                        .flex_none(),
                    ),
            )
    }

    /// Every variant at YouTube's small sizes, side by side down the column, and which one
    /// uploads.
    fn compare(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let cards =
            self.design
                .variants
                .iter()
                .enumerate()
                .map(|(index, variant)| {
                    let on = index == self.selected;
                    let upload = self.design.upload.as_ref() == Some(&variant.name);
                    let stills = self.stills.get(&variant.name);
                    let name = variant.name.clone();
                    div()
                        .id(("studio-variant", index))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .p(px(10.))
                        .rounded(RADIUS_MD)
                        .bg(if on { FLOAT } else { CARD })
                        .border_1()
                        .border_color(if on {
                            Hsla::from(ACCENT).opacity(0.6)
                        } else {
                            gpui::transparent_black()
                        })
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.select(index, window, cx)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                                    "Variant {}",
                                    variant.name.as_str().to_uppercase()
                                )))
                                .child(div().flex_1())
                                .map(|row| {
                                    if upload {
                                        row.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(4.))
                                                .text_size(TEXT_SM)
                                                .text_color(ACCENT)
                                                .child(icon("check", ICON_SM, ACCENT))
                                                .child("Upload"),
                                        )
                                    } else {
                                        row.child(
                                            proposal_action(
                                                format!("studio-upload-{index}"),
                                                "Use for upload",
                                                None,
                                                None,
                                                ActionLook::Quiet,
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.set_upload(name.clone(), cx)
                                                }),
                                            ),
                                        )
                                    }
                                }),
                        )
                        .children((0..SMALL.len()).map(|size| {
                            small_still(stills.and_then(|s| s.small[size].clone()), SMALL[size])
                        }))
                });
        ui::pane()
            .id("studio-compare")
            .w(px(300.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(14.))
            .overflow_y_scroll()
            .child(section("At YouTube sizes"))
            .children(cards)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .pt(px(4.))
                    .child(
                        proposal_action("studio-new", "New variant", None, None, ActionLook::Plain)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_variant(false, window, cx)
                            })),
                    )
                    .child(
                        proposal_action(
                            "studio-duplicate",
                            "Duplicate",
                            None,
                            None,
                            ActionLook::Plain,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.new_variant(true, window, cx)),
                        ),
                    )
                    .when(self.design.variants.len() > 1, |row| {
                        row.child(
                            proposal_action(
                                "studio-remove",
                                "Remove",
                                None,
                                None,
                                ActionLook::Quiet,
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.remove_variant(window, cx)),
                            ),
                        )
                    }),
            )
    }
}

/// A variant at one of YouTube's small sizes, with where YouTube shows it that size.
fn small_still(still: Option<Arc<RenderImage>>, (place, w, h): (&str, f32, f32)) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .w(px(w))
                .h(px(h))
                .flex_none()
                .rounded(px(if w > 200. { 8. } else { 6. }))
                .overflow_hidden()
                .bg(WELL)
                .when_some(still, |frame, still| {
                    frame.child(img(still).size_full().object_fit(ObjectFit::Fill))
                }),
        )
        .child(
            div()
                .text_size(TEXT_XS)
                .text_color(MUTED)
                .child(format!("{place}, {} px", w as u32)),
        )
}

fn section(title: &'static str) -> Div {
    label(title).pt(px(6.))
}

fn note(text: &'static str) -> Div {
    div().text_size(TEXT_SM).text_color(MUTED).child(text)
}

fn field(input: &Entity<InputState>) -> Div {
    div()
        .h(px(34.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(RADIUS_SM)
        .bg(WELL)
        .child(Input::new(input).appearance(false))
}

fn slider_row(name: &'static str, slider: &Entity<SliderState>, value: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(4.))
        .child(div().w(px(44.)).flex_none().text_color(MUTED).child(name))
        .child(Slider::new(slider).accessibility_label(name).flex_1())
        .child(
            div()
                .w(px(56.))
                .flex_none()
                .flex()
                .justify_end()
                .text_size(TEXT_SM)
                .child(value),
        )
}

/// A cut time as m:ss.
fn clock(at: Micros) -> SharedString {
    let seconds = at.0 / 1_000_000;
    format!("{}:{:02}", seconds / 60, seconds % 60).into()
}
