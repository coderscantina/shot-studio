//! The layout builder: a window that places the screen and camera slots of a channel layout on
//! a live 16:9 canvas showing a real frame of the take, drawn by the renderer export uses, so
//! corners, crop and the brand's camera frame look as they will.
//!
//! Drag a slot to move it, its edges or corners to resize it (Shift keeps its shape), ⌥-drag
//! inside it to move its crop focus. Edges snap to the frame's edges and center lines and to the
//! other slot, with a guide while they hold. Arrows nudge (Shift for larger steps), ⌥ arrows
//! resize, Tab picks the other slot, `[` and `]` send it back or bring it forward, Backspace
//! removes it. ⌘S saves into the channel's brand, Esc closes without saving.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Div, Entity, EventEmitter, FocusHandle, FontWeight,
    Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels, Point, SharedString,
    Stateful, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, point,
    prelude::*, px, size, surface,
};
use gpui_component::Root;
use gpui_component::input::{Input, InputState};
use gpui_component::switch::Switch;
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use shot_core::{CustomLayout, Id, Layout, LayoutName, LayoutSlot, Micros, Track};
use shot_media::videotoolbox::Decoder;
use shot_render::graph::Renderer;
use shot_render::layout::{Canvas, LayoutSpec, Sources};
use shot_render::surface::{BufferPool, Matrix, Range, size as buffer_size};
use shot_render::theme::Theme;
use shot_render::zoom::Punch;

use crate::edit::layout_name;
use crate::theme::*;
use crate::ui::{self, kbd, label, mono};

actions!(
    layout_builder,
    [
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
        JumpLeft,
        JumpRight,
        JumpUp,
        JumpDown,
        Narrower,
        Wider,
        Shorter,
        Taller,
        MuchNarrower,
        MuchWider,
        MuchShorter,
        MuchTaller,
        NextSlot,
        BringForward,
        SendBack,
        RemoveSlot,
        SaveLayout,
        CloseBuilder,
    ]
);

pub(crate) const CONTEXT: &str = "LayoutBuilder";
/// The design frame slots are placed on.
const W: f32 = 3840.0;
const H: f32 = 2160.0;
/// What an arrow moves a slot by, and with Shift, in design frame pixels.
const NUDGE: f32 = 8.0;
const JUMP: f32 = 96.0;
/// How close an edge comes to a guide before it holds there, in pixels on screen.
const SNAP: f32 = 8.0;
/// How near an edge or corner a press grabs it, in pixels on screen.
const GRIP: f32 = 9.0;
/// The smallest slot, in design frame pixels.
const MIN_SIZE: f32 = 96.0;
/// The canvas render: the preview's size, sharp at the window's canvas.
const RENDER: Canvas = Canvas {
    width: 1920,
    height: 1080,
};
const CORNERS: [u16; 6] = [0, 16, 32, 48, 96, 160];

/// What the builder opens with.
pub struct BuilderInput {
    pub dir: std::path::PathBuf,
    /// The take and media time whose frame the canvas shows.
    pub frame: Option<(Id, Micros)>,
    pub theme: Arc<Theme>,
    /// The channel layout to edit, `None` for a new one.
    pub editing: Option<CustomLayout>,
    /// The layout a new one starts from.
    pub start: Layout,
    /// Whether the layout is on the video's 1 to 4 keys.
    pub on_keys: bool,
}

/// The builder asks to save a layout. The window stays open until the screen that opened it
/// says it was saved.
pub struct SaveRequest {
    pub layout: CustomLayout,
    pub on_keys: bool,
}

/// A decoded frame of one track and its size.
struct Track_ {
    buffer: CFRetained<CVPixelBuffer>,
    size: (u32, u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Body,
    /// The crop focus, ⌥-drag.
    Focus,
    Edges {
        left: bool,
        right: bool,
        top: bool,
        bottom: bool,
    },
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    slot: usize,
    grip: Grip,
    /// Where the press was, in design frame pixels.
    from: [f32; 2],
    was: LayoutSlot,
}

/// A guide an edge holds to: a vertical line at x, or a horizontal one at y.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Guide {
    X(f32),
    Y(f32),
}

pub struct LayoutBuilder {
    editing: Option<LayoutName>,
    /// Names the channel has, which a new layout may not take.
    taken: Vec<LayoutName>,
    slots: Vec<LayoutSlot>,
    selected: Option<usize>,
    name: Entity<InputState>,
    on_keys: bool,
    drag: Option<Drag>,
    guides: Vec<Guide>,
    canvas: Rc<Cell<Bounds<Pixels>>>,
    theme: Arc<Theme>,
    screen: Option<Track_>,
    camera: Option<Track_>,
    renderer: Option<(Renderer, BufferPool)>,
    shown: Option<CFRetained<CVPixelBuffer>>,
    dirty: bool,
    error: Option<String>,
    focus: FocusHandle,
}

impl EventEmitter<SaveRequest> for LayoutBuilder {}

/// Opens the builder in a window of its own.
pub fn open(
    input: BuilderInput,
    cx: &mut App,
) -> Result<(Entity<LayoutBuilder>, AnyWindowHandle), String> {
    let title = match &input.editing {
        Some(layout) => format!("{} layout", layout_name(Layout::Custom(layout.name))),
        None => "New layout".to_owned(),
    };
    let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
    let mut made = None;
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(1280.), px(800.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                ..Default::default()
            },
            |window, cx| {
                let builder = cx.new(|cx| LayoutBuilder::new(input, window, cx));
                made = Some(builder.clone());
                cx.new(|cx| Root::new(builder, window, cx))
            },
        )
        .map_err(|e| e.to_string())?;
    cx.activate(true);
    Ok((made.expect("the window made its builder"), window.into()))
}

/// The frame of the track in `file` of `take` on screen at `at`.
fn decode(dir: &Path, take: &Id, file: &str, at: Micros) -> Option<Track_> {
    let path = dir.join("media").join(take.as_str()).join(file);
    if !path.exists() {
        return None;
    }
    let mut decoder = Decoder::open_native(&path).ok()?;
    let time = Duration::from_micros(at.0) + Duration::from_millis(1);
    let frame = decoder.seek(time, || false).ok()??;
    Some(Track_ {
        size: buffer_size(&frame.buffer),
        buffer: frame.buffer,
    })
}

impl LayoutBuilder {
    fn new(input: BuilderInput, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let taken: Vec<LayoutName> = input.theme.layouts.iter().map(|l| l.name).collect();
        let (slots, name) = match &input.editing {
            Some(layout) => (layout.slots.clone(), layout.name.as_str().to_owned()),
            None => {
                let slots = super::slots_of(input.start, &input.theme.layouts);
                let base = match input.start {
                    Layout::Custom(name) => format!("{} copy", super::display_name(name)),
                    layout => format!("My {}", layout_name(layout).to_lowercase()),
                };
                let mut name = base.clone();
                let mut n = 2;
                while super::name_from(&name).is_some_and(|n| taken.contains(&n)) {
                    name = format!("{base} {n}");
                    n += 1;
                }
                (slots, name)
            }
        };
        let editing = input.editing.as_ref().map(|l| l.name);
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Name, like Talk left")
                .default_value(name)
        });
        cx.observe(&name, |this, _, cx| {
            this.error = None;
            cx.notify();
        })
        .detach();
        let (screen, camera) = match &input.frame {
            Some((take, at)) => (
                decode(&input.dir, take, "screen.mp4", *at),
                decode(&input.dir, take, "camera.mp4", *at),
            ),
            None => (None, None),
        };
        let renderer = if screen.is_some() || camera.is_some() {
            Renderer::new().ok().and_then(|renderer| {
                let pool = BufferPool::new(RENDER.width, RENDER.height, Range::Full, Matrix::Bt601)
                    .ok()?;
                Some((renderer, pool))
            })
        } else {
            None
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            editing,
            taken,
            selected: (!slots.is_empty()).then_some(slots.len() - 1),
            slots,
            name,
            on_keys: input.on_keys,
            drag: None,
            guides: Vec::new(),
            canvas: Rc::default(),
            theme: input.theme,
            screen,
            camera,
            renderer,
            shown: None,
            dirty: true,
            error: None,
            focus,
        }
    }

    /// The layout as it stands, if its name is a layout name.
    fn layout(&self, cx: &App) -> Result<CustomLayout, String> {
        let name = match self.editing {
            Some(name) => name,
            None => {
                let text = self.name.read(cx).value();
                let name = super::name_from(&text).ok_or(
                    "Name the layout with letters and digits, starting with a letter, and \
                     not like a built-in layout",
                )?;
                if self.taken.contains(&name) {
                    return Err(format!(
                        "The channel has a layout called {} already",
                        super::display_name(name)
                    ));
                }
                name
            }
        };
        if self.slots.is_empty() {
            return Err("Add the screen, the camera or both".into());
        }
        Ok(CustomLayout {
            name,
            slots: self.slots.clone(),
        })
    }

    /// Shows `error` from the screen that tried to save.
    pub fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        match self.layout(cx) {
            Ok(layout) => cx.emit(SaveRequest {
                layout,
                on_keys: self.on_keys,
            }),
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.error = None;
        cx.notify();
    }

    /// Draws the canvas frame again after a change, through the renderer export uses.
    fn draw(&mut self) {
        self.dirty = false;
        let Some((renderer, pool)) = &self.renderer else {
            return;
        };
        let draft = CustomLayout {
            name: LayoutName::new("draft").expect("a valid name"),
            slots: self.slots.clone(),
        };
        let mut spec = LayoutSpec::custom(&draft);
        if let Some(camera) = self.theme.camera {
            spec = spec.with_camera(camera);
        }
        let sizes = Sources {
            screen: self.screen.as_ref().map_or((16, 9), |t| t.size),
            camera: self.camera.as_ref().map_or((16, 9), |t| t.size),
        };
        let mut scene = spec.scene(RENDER, sizes, Punch::none(sizes.screen));
        let sources = Sources {
            screen: self.screen.as_ref().map(|t| &*t.buffer),
            camera: self.camera.as_ref().map(|t| &*t.buffer),
        };
        scene.layers.retain(|l| sources.get(l.source).is_some());
        let Ok(target) = pool.buffer() else {
            return;
        };
        if renderer.render(&scene, sources, &target).is_ok() {
            self.shown = Some(target);
        }
    }

    /// Design frame pixels per pixel on screen.
    fn per_pixel(&self) -> f32 {
        W / f32::from(self.canvas.get().size.width).max(1.0)
    }

    fn design_point(&self, at: Point<Pixels>) -> [f32; 2] {
        let b = self.canvas.get();
        let s = self.per_pixel();
        [
            f32::from(at.x - b.origin.x) * s,
            f32::from(at.y - b.origin.y) * s,
        ]
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let at = self.design_point(event.position);
        let reach = GRIP * self.per_pixel();
        let edges_of = |s: &LayoutSlot| {
            let (l, t) = (f32::from(s.x), f32::from(s.y));
            let (r, b) = (l + f32::from(s.width), t + f32::from(s.height));
            let inside_x = at[0] > l - reach && at[0] < r + reach;
            let inside_y = at[1] > t - reach && at[1] < b + reach;
            let edges = Grip::Edges {
                left: inside_y && (at[0] - l).abs() < reach,
                right: inside_y && (at[0] - r).abs() < reach,
                top: inside_x && (at[1] - t).abs() < reach,
                bottom: inside_x && (at[1] - b).abs() < reach,
            };
            (edges, l <= at[0] && at[0] <= r && t <= at[1] && at[1] <= b)
        };
        let any = |g: Grip| matches!(g, Grip::Edges { left, right, top, bottom } if left || right || top || bottom);
        // The selected slot's edges first, then the topmost slot under the press.
        let selected = self
            .selected
            .filter(|&i| any(edges_of(&self.slots[i]).0))
            .map(|i| (i, edges_of(&self.slots[i]).0));
        let hit = selected.or_else(|| {
            (0..self.slots.len()).rev().find_map(|i| {
                let (edges, inside) = edges_of(&self.slots[i]);
                if any(edges) {
                    Some((i, edges))
                } else if inside {
                    let grip = if event.modifiers.alt {
                        Grip::Focus
                    } else {
                        Grip::Body
                    };
                    Some((i, grip))
                } else {
                    None
                }
            })
        });
        match hit {
            Some((slot, grip)) => {
                self.selected = Some(slot);
                self.drag = Some(Drag {
                    slot,
                    grip,
                    from: at,
                    was: self.slots[slot],
                });
            }
            None => self.selected = None,
        }
        cx.notify();
    }

    fn drag_to(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else {
            return;
        };
        let at = self.design_point(event.position);
        let (dx, dy) = (at[0] - drag.from[0], at[1] - drag.from[1]);
        let was = drag.was;
        let (l, t) = (f32::from(was.x), f32::from(was.y));
        let (w, h) = (f32::from(was.width), f32::from(was.height));
        let threshold = SNAP * self.per_pixel();
        let (xs, ys) = self.targets(drag.slot);
        self.guides.clear();
        let mut rect = [l, t, l + w, t + h];
        match drag.grip {
            Grip::Focus => {
                // Dragging right shows more of the right of the track: the focus follows.
                let fx = f32::from(was.focus[0]) + dx / w * 100.0;
                let fy = f32::from(was.focus[1]) + dy / h * 100.0;
                self.slots[drag.slot].focus = [fx, fy].map(|f| f.clamp(0.0, 100.0).round() as u8);
                self.changed(cx);
                return;
            }
            Grip::Body => {
                let x = (l + dx).clamp(0.0, W - w);
                let y = (t + dy).clamp(0.0, H - h);
                let (x, gx) = snap_span(x, w, &xs, threshold);
                let (y, gy) = snap_span(y, h, &ys, threshold);
                self.guides
                    .extend(gx.map(Guide::X).into_iter().chain(gy.map(Guide::Y)));
                rect = [
                    x.clamp(0.0, W - w),
                    y.clamp(0.0, H - h),
                    x.clamp(0.0, W - w) + w,
                    y.clamp(0.0, H - h) + h,
                ];
            }
            Grip::Edges {
                left,
                right,
                top,
                bottom,
            } => {
                let mut edge = |value: f32, targets: &[f32], vertical: bool| {
                    let (snapped, guide) = snap(value, targets, threshold);
                    if let Some(g) = guide {
                        self.guides
                            .push(if vertical { Guide::X(g) } else { Guide::Y(g) });
                    }
                    snapped
                };
                if left {
                    rect[0] = edge((l + dx).clamp(0.0, rect[2] - MIN_SIZE), &xs, true);
                }
                if right {
                    rect[2] = edge((l + w + dx).clamp(rect[0] + MIN_SIZE, W), &xs, true);
                }
                if top {
                    rect[1] = edge((t + dy).clamp(0.0, rect[3] - MIN_SIZE), &ys, false);
                }
                if bottom {
                    rect[3] = edge((t + h + dy).clamp(rect[1] + MIN_SIZE, H), &ys, false);
                }
                // Shift on a corner keeps the slot's shape, led by its width.
                if event.modifiers.shift && (left || right) && (top || bottom) {
                    let height = (rect[2] - rect[0]) * h / w;
                    if top {
                        rect[1] = (rect[3] - height).max(0.0);
                    } else {
                        rect[3] = (rect[1] + height).min(H);
                    }
                    self.guides.retain(|g| matches!(g, Guide::X(_)));
                }
            }
        }
        self.place(drag.slot, rect);
        self.changed(cx);
    }

    /// Where the edges of `slot` may hold: the frame's edges and center lines, and the other
    /// slots' edges and centers.
    fn targets(&self, slot: usize) -> (Vec<f32>, Vec<f32>) {
        let mut xs = vec![0.0, W / 2.0, W];
        let mut ys = vec![0.0, H / 2.0, H];
        for (_, s) in self.slots.iter().enumerate().filter(|(i, _)| *i != slot) {
            let (l, t) = (f32::from(s.x), f32::from(s.y));
            let (w, h) = (f32::from(s.width), f32::from(s.height));
            xs.extend([l, l + w / 2.0, l + w]);
            ys.extend([t, t + h / 2.0, t + h]);
        }
        (xs, ys)
    }

    /// Sets the slot to `rect`, left, top, right and bottom, rounded to whole pixels inside the
    /// frame.
    fn place(&mut self, slot: usize, rect: [f32; 4]) {
        let s = &mut self.slots[slot];
        let left = rect[0].clamp(0.0, W - MIN_SIZE).round();
        let top = rect[1].clamp(0.0, H - MIN_SIZE).round();
        s.x = left as u16;
        s.y = top as u16;
        s.width = (rect[2].min(W) - left).round().max(MIN_SIZE) as u16;
        s.height = (rect[3].min(H) - top).round().max(MIN_SIZE) as u16;
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.guides.clear();
            cx.notify();
        }
    }

    fn nudge(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let Some(i) = self.selected else { return };
        let s = self.slots[i];
        let (w, h) = (f32::from(s.width), f32::from(s.height));
        let x = (f32::from(s.x) + dx).clamp(0.0, W - w);
        let y = (f32::from(s.y) + dy).clamp(0.0, H - h);
        self.place(i, [x, y, x + w, y + h]);
        self.changed(cx);
    }

    fn resize(&mut self, dw: f32, dh: f32, cx: &mut Context<Self>) {
        let Some(i) = self.selected else { return };
        let s = self.slots[i];
        let (x, y) = (f32::from(s.x), f32::from(s.y));
        let right = (x + f32::from(s.width) + dw).clamp(x + MIN_SIZE, W);
        let bottom = (y + f32::from(s.height) + dh).clamp(y + MIN_SIZE, H);
        self.place(i, [x, y, right, bottom]);
        self.changed(cx);
    }

    /// Moves the selected slot `by` places in the drawing order, up towards the top.
    fn reorder(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(i) = self.selected else { return };
        let j = if up { i + 1 } else { i.wrapping_sub(1) };
        if j < self.slots.len() {
            self.slots.swap(i, j);
            self.selected = Some(j);
            self.changed(cx);
        }
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.selected else { return };
        self.slots.remove(i);
        self.selected = (!self.slots.is_empty()).then_some(self.slots.len() - 1);
        self.changed(cx);
    }

    /// Adds `track` where it reads well: the screen large and centered, the camera as an inset
    /// at the bottom right.
    fn add(&mut self, track: Track, cx: &mut Context<Self>) {
        let (x, y, width, height, corners) = match track {
            Track::Screen => (320, 180, 3200, 1800, 32),
            Track::Camera => (2808, 1548, 960, 540, 32),
        };
        let slot = LayoutSlot {
            track,
            x,
            y,
            width,
            height,
            corners,
            focus: [50, 50],
        };
        // The screen goes under the camera.
        let index = match track {
            Track::Screen => 0,
            Track::Camera => self.slots.len(),
        };
        self.slots.insert(index, slot);
        self.selected = Some(index);
        self.changed(cx);
    }

    /// Starts again from the slots of `layout`, none for blank.
    fn start_from(&mut self, layout: Option<Layout>, cx: &mut Context<Self>) {
        self.slots = layout.map_or_else(Vec::new, |l| super::slots_of(l, &self.theme.layouts));
        self.selected = (!self.slots.is_empty()).then_some(self.slots.len() - 1);
        self.changed(cx);
    }

    fn set_selected(&mut self, change: impl FnOnce(&mut LayoutSlot), cx: &mut Context<Self>) {
        if let Some(i) = self.selected {
            change(&mut self.slots[i]);
            self.changed(cx);
        }
    }
}

/// `value` held to the nearest of `targets` within `threshold`, and the target it holds to.
fn snap(value: f32, targets: &[f32], threshold: f32) -> (f32, Option<f32>) {
    targets
        .iter()
        .map(|&t| (t, (t - value).abs()))
        .filter(|(_, d)| *d <= threshold)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or((value, None), |(t, _)| (t, Some(t)))
}

/// A span from `start` of `length` moved so its start, middle or end holds to the nearest of
/// `targets` within `threshold`.
fn snap_span(start: f32, length: f32, targets: &[f32], threshold: f32) -> (f32, Option<f32>) {
    let mut best: Option<(f32, f32, f32)> = None;
    for offset in [0.0, length / 2.0, length] {
        let (snapped, guide) = snap(start + offset, targets, threshold);
        if let Some(guide) = guide {
            let distance = (snapped - (start + offset)).abs();
            if best.is_none_or(|(d, ..)| distance < d) {
                best = Some((distance, snapped - offset, guide));
            }
        }
    }
    best.map_or((start, None), |(_, start, guide)| (start, Some(guide)))
}

fn track_name(track: Track) -> &'static str {
    match track {
        Track::Screen => "Screen",
        Track::Camera => "Camera",
    }
}

impl Render for LayoutBuilder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dirty {
            self.draw();
        }
        div()
            .id("layout-builder")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(FRAME)
            .text_color(TEXT)
            .font_family(SANS)
            .text_size(TEXT_BASE)
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.drag_to(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.release(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.release(cx)),
            )
            .on_action(cx.listener(|this, _: &NudgeLeft, _, cx| this.nudge(-NUDGE, 0.0, cx)))
            .on_action(cx.listener(|this, _: &NudgeRight, _, cx| this.nudge(NUDGE, 0.0, cx)))
            .on_action(cx.listener(|this, _: &NudgeUp, _, cx| this.nudge(0.0, -NUDGE, cx)))
            .on_action(cx.listener(|this, _: &NudgeDown, _, cx| this.nudge(0.0, NUDGE, cx)))
            .on_action(cx.listener(|this, _: &JumpLeft, _, cx| this.nudge(-JUMP, 0.0, cx)))
            .on_action(cx.listener(|this, _: &JumpRight, _, cx| this.nudge(JUMP, 0.0, cx)))
            .on_action(cx.listener(|this, _: &JumpUp, _, cx| this.nudge(0.0, -JUMP, cx)))
            .on_action(cx.listener(|this, _: &JumpDown, _, cx| this.nudge(0.0, JUMP, cx)))
            .on_action(cx.listener(|this, _: &Narrower, _, cx| this.resize(-NUDGE, 0.0, cx)))
            .on_action(cx.listener(|this, _: &Wider, _, cx| this.resize(NUDGE, 0.0, cx)))
            .on_action(cx.listener(|this, _: &Shorter, _, cx| this.resize(0.0, -NUDGE, cx)))
            .on_action(cx.listener(|this, _: &Taller, _, cx| this.resize(0.0, NUDGE, cx)))
            .on_action(cx.listener(|this, _: &MuchNarrower, _, cx| this.resize(-JUMP, 0.0, cx)))
            .on_action(cx.listener(|this, _: &MuchWider, _, cx| this.resize(JUMP, 0.0, cx)))
            .on_action(cx.listener(|this, _: &MuchShorter, _, cx| this.resize(0.0, -JUMP, cx)))
            .on_action(cx.listener(|this, _: &MuchTaller, _, cx| this.resize(0.0, JUMP, cx)))
            .on_action(cx.listener(|this, _: &NextSlot, _, cx| {
                if !this.slots.is_empty() {
                    this.selected = Some(this.selected.map_or(0, |i| (i + 1) % this.slots.len()));
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &BringForward, _, cx| this.reorder(true, cx)))
            .on_action(cx.listener(|this, _: &SendBack, _, cx| this.reorder(false, cx)))
            .on_action(cx.listener(|this, _: &RemoveSlot, _, cx| this.remove(cx)))
            .on_action(cx.listener(|this, _: &SaveLayout, _, cx| this.save(cx)))
            .on_action(cx.listener(|_, _: &CloseBuilder, window, _| window.remove_window()))
            .child(title_bar(self, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(8.))
                    .px(px(8.))
                    .pb(px(8.))
                    .child(stage(self, cx))
                    .child(inspector(self, cx)),
            )
    }
}

fn title_bar(this: &LayoutBuilder, cx: &mut Context<LayoutBuilder>) -> Div {
    let title = match this.editing {
        Some(name) => format!("{} layout", super::display_name(name)),
        None => "New layout".into(),
    };
    div()
        .h(px(46.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.))
        .pl(px(88.))
        .pr(px(12.))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(
            label("Saved in the channel's brand, offered next to the built-in layouts")
                .text_size(TEXT_SM),
        )
        .child(div().flex_1())
        .child(
            ui::big_button("builder-cancel", "close", "Cancel", "esc", false)
                .h(px(34.))
                .on_click(cx.listener(|_, _, window, _| window.remove_window())),
        )
        .child(
            ui::big_button("builder-save", "check", "Save layout", "⌘S", true)
                .h(px(34.))
                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
        )
}

/// The 16:9 canvas with the rendered frame, the slots' outlines, handles and the guides.
fn stage(this: &LayoutBuilder, cx: &mut Context<LayoutBuilder>) -> Div {
    let frac = |v: u16, of: f32| gpui::relative(f32::from(v) / of);
    let slots = this.slots.iter().enumerate().map(|(i, s)| {
        let on = this.selected == Some(i);
        let color: Hsla = if on {
            ACCENT.into()
        } else {
            Hsla::from(TEXT).opacity(0.55)
        };
        let mut outline = div()
            .absolute()
            .left(frac(s.x, W))
            .top(frac(s.y, H))
            .w(frac(s.width, W))
            .h(frac(s.height, H))
            .border(px(if on { 2. } else { 1. }))
            .border_color(color)
            .when(this.shown.is_none(), |d| {
                d.bg(match s.track {
                    Track::Screen => Hsla::from(ZOOM).opacity(0.18),
                    Track::Camera => Hsla::from(ACCENT).opacity(0.14),
                })
            })
            .child(
                div()
                    .absolute()
                    .top(px(6.))
                    .left(px(6.))
                    .px(px(6.))
                    .rounded(px(4.))
                    .bg(if on { ACCENT } else { FLOAT })
                    .text_color(if on { ON_ACCENT } else { TEXT })
                    .text_size(TEXT_XS)
                    .font_weight(FontWeight::MEDIUM)
                    .child(track_name(s.track)),
            );
        if on {
            for (x, y) in [
                (0.0, 0.0),
                (0.5, 0.0),
                (1.0, 0.0),
                (0.0, 0.5),
                (1.0, 0.5),
                (0.0, 1.0),
                (0.5, 1.0),
                (1.0, 1.0),
            ] {
                outline = outline.child(
                    div()
                        .absolute()
                        .left(gpui::relative(x))
                        .top(gpui::relative(y))
                        .ml(px(-5.))
                        .mt(px(-5.))
                        .size(px(10.))
                        .rounded(px(2.))
                        .bg(ACCENT)
                        .border_1()
                        .border_color(ON_ACCENT),
                );
            }
        }
        outline
    });
    let guides = this.guides.iter().map(|g| match *g {
        Guide::X(x) => div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(gpui::relative(x / W))
            .w(px(1.))
            .bg(ACCENT),
        Guide::Y(y) => div()
            .absolute()
            .left_0()
            .right_0()
            .top(gpui::relative(y / H))
            .h(px(1.))
            .bg(ACCENT),
    });
    let hint = match this.drag.map(|d| d.grip) {
        Some(Grip::Focus) => "Moving the crop focus",
        _ if this.slots.is_empty() => "Blank: add the screen or the camera from the panel",
        _ => "Drag to move, edges to resize, ⌥-drag for the crop focus",
    };
    ui::pane()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(12.))
        .p(px(24.))
        .bg(WELL)
        .child(
            div()
                .id("builder-canvas")
                .relative()
                .w_full()
                .aspect_ratio(16. / 9.)
                .flex_none()
                .bg(gpui::black())
                .rounded(px(6.))
                .shadow_lg()
                .cursor_pointer()
                .child(
                    crate::ui::measure(this.canvas.clone())
                        .absolute()
                        .size_full(),
                )
                .children(this.shown.as_ref().map(|buffer| {
                    surface(buffer.clone())
                        .absolute()
                        .size_full()
                        .object_fit(ObjectFit::Fill)
                }))
                .children(slots)
                .children(guides)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, window, cx| this.press(e, window, cx)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .text_size(TEXT_SM)
                .text_color(MUTED)
                .child(hint)
                .child(div().flex_1())
                .child(ui::hint("←↑↓→", "Nudge"))
                .child(ui::hint("⌥ arrows", "Resize"))
                .child(ui::hint("tab", "Next slot")),
        )
}

fn section(title: &'static str) -> Div {
    div().flex().flex_col().gap(px(8.)).child(
        label(title)
            .text_size(TEXT_XS)
            .font_weight(FontWeight::MEDIUM),
    )
}

/// A small option button, lit when it is the current one.
fn option(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(28.))
        .px(px(10.))
        .rounded(RADIUS_SM)
        .bg(if on { FLOAT_ON } else { RAISED })
        .text_color(if on { STRONG } else { TEXT })
        .text_size(TEXT_SM)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT))
        .child(text.into())
}

fn inspector(this: &LayoutBuilder, cx: &mut Context<LayoutBuilder>) -> impl IntoElement {
    let selected = this.selected.map(|i| (i, this.slots[i]));
    let mut panel = ui::pane()
        .w(px(320.))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(18.))
        .p(px(16.))
        .text_size(TEXT_SM);

    panel = panel.child(
        section("Name").child(match this.editing {
            Some(name) => div().text_color(BODY).child(super::display_name(name)),
            None => div()
                .h(px(32.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(RADIUS_SM)
                .bg(WELL)
                .child(Input::new(&this.name).appearance(false)),
        }),
    );

    if this.editing.is_none() {
        let mut starts: Vec<(Option<Layout>, &'static str)> = vec![(None, "Blank")];
        starts.extend(
            [
                Layout::Face,
                Layout::FaceContent,
                Layout::Split,
                Layout::Content,
            ]
            .into_iter()
            .chain(this.theme.layouts.iter().map(|l| Layout::Custom(l.name)))
            .map(|l| (Some(l), layout_name(l))),
        );
        panel = panel.child(
            section("Start from").child(div().flex().flex_wrap().gap(px(4.)).children(
                starts.into_iter().enumerate().map(|(i, (layout, name))| {
                    option(("builder-start", i), name, false)
                        .on_click(cx.listener(move |this, _, _, cx| this.start_from(layout, cx)))
                }),
            )),
        );
    }

    // The drawing order, top first.
    let rows = this.slots.iter().enumerate().rev().map(|(i, s)| {
        let on = this.selected == Some(i);
        div()
            .id(("builder-slot", i))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .px(px(10.))
            .rounded(RADIUS_SM)
            .bg(if on { FLOAT_ON } else { RAISED })
            .cursor_pointer()
            .hover(|d| d.bg(FLOAT))
            .child(ui::icon(
                match s.track {
                    Track::Screen => "display",
                    Track::Camera => "camera",
                },
                ICON_SM,
                if on { STRONG } else { MUTED },
            ))
            .child(track_name(s.track))
            .child(div().flex_1())
            .child(
                mono(format!("{}x{}", s.width, s.height))
                    .text_size(TEXT_XS)
                    .text_color(MUTED),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(i);
                cx.notify();
            }))
    });
    let missing: Vec<Track> = [Track::Screen, Track::Camera]
        .into_iter()
        .filter(|t| !this.slots.iter().any(|s| s.track == *t))
        .collect();
    panel = panel.child(
        section("Slots, top first").children(rows).child(
            div()
                .flex()
                .gap(px(4.))
                .children(missing.into_iter().map(|track| {
                    option(
                        ("builder-add", track as usize),
                        format!("Add {}", track_name(track).to_lowercase()),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.add(track, cx)))
                }))
                .when(selected.is_some(), |row| {
                    row.child(div().flex_1())
                        .child(
                            option("builder-back", "Back", false)
                                .child(div().pl(px(6.)).child(kbd("[")))
                                .on_click(cx.listener(|this, _, _, cx| this.reorder(false, cx))),
                        )
                        .child(
                            option("builder-forward", "Forward", false)
                                .child(div().pl(px(6.)).child(kbd("]")))
                                .on_click(cx.listener(|this, _, _, cx| this.reorder(true, cx))),
                        )
                }),
        ),
    );

    if let Some((_, slot)) = selected {
        let camera_framed = slot.track == Track::Camera
            && this.theme.camera.is_some()
            && this.slots.iter().any(|s| s.track == Track::Screen);
        let place = div()
            .flex()
            .gap(px(12.))
            .font_family(MONO)
            .text_size(TEXT_XS)
            .text_color(BODY)
            .child(format!("x {}  y {}", slot.x, slot.y))
            .child(format!("{} x {}", slot.width, slot.height));
        let corners = if camera_framed {
            let camera = this.theme.camera.expect("checked above");
            div().text_color(MUTED).text_size(TEXT_XS).child(format!(
                "The brand frames the camera: {}, {} px corners",
                format!("{:?}", camera.shape).to_lowercase(),
                camera.corners
            ))
        } else {
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(CORNERS.into_iter().map(|c| {
                    option(
                        ("builder-corners", c as usize),
                        c.to_string(),
                        slot.corners == c,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_selected(|s| s.corners = c, cx)),
                    )
                }))
        };
        let focus = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .children([0u8, 50, 100].map(|fy| {
                div().flex().gap(px(3.)).children([0u8, 50, 100].map(|fx| {
                    let on = slot.focus == [fx, fy];
                    div()
                        .id(("builder-focus", usize::from(fy) * 1000 + usize::from(fx)))
                        .size(px(26.))
                        .rounded(px(5.))
                        .bg(if on { ACCENT } else { RAISED })
                        .hover(|d| d.bg(if on { ACCENT } else { FLOAT }))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_selected(|s| s.focus = [fx, fy], cx)
                        }))
                }))
            }));
        panel = panel
            .child(section("Place").child(place))
            .child(section("Corners").child(corners))
            .child(
                section("Crop focus").child(
                    div().flex().items_center().gap(px(12.)).child(focus).child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .text_color(MUTED)
                            .text_size(TEXT_XS)
                            .child(format!("{} %, {} %", slot.focus[0], slot.focus[1]))
                            .child("Which part of the track the slot keeps")
                            .child("⌥-drag inside the slot to fine tune"),
                    ),
                ),
            )
            .child(
                option("builder-remove", "Remove slot", false)
                    .child(div().pl(px(6.)).child(kbd("⌫")))
                    .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
            );
    }

    panel = panel.child(div().flex_1()).child(
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                Switch::new("builder-keys")
                    .checked(this.on_keys)
                    .accessibility_label("Offer on the 1 to 4 keys")
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.on_keys = *on;
                        cx.notify();
                    })),
            )
            .child("Offer on the 1 to 4 keys of this video"),
    );
    panel.children(this.error.clone().map(ui::alert))
}
