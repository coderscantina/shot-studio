//! Everything one output frame shows, for preview and export alike: the layout's source layers
//! with the punch-in of the moment, and the product video effects drawn from the event log and
//! the overlays. Pure, no GPU: [`compose`] decides, the render graph draws.
//!
//! - **Cursor**: the pointer at the event log's position, averaged over the style's smoothing,
//!   scaled by its size, clipped to the screen's slot. A ripple spreads from every click.
//! - **Keystrokes**: the latest key press as a label at the bottom, or the typed run of text.
//! - **Window**: the window layout's shadow and device frame under the window.
//! - **Overlays**: the templates of [`crate::overlay`], in file order, later ones on top.

use std::collections::HashMap;
use std::sync::Arc;

use shot_core::{
    DeviceFrame, Effects, Id, Keystrokes, Layout, Micros, Overlay, Shadow, Zoom, ZoomMotion,
};

use vello::peniko::ImageData;

use crate::code_diff::CodeDiff;
use crate::drawing::{Drawing, Rgba, Shape};
use crate::events::{EventLog, KeyPress, Modifier};
use crate::layout::{
    Canvas, Layer, LayoutSpec, Rect, Scene, Source, Sources, UnsupportedLayout, bezel,
};
use crate::materials;
use crate::overlay::{self, Cutaway, Stage};
use crate::theme::Theme;
use crate::zoom::punch_at;

/// The pointer's height at size 100, in pixels of a 2x screen track.
pub const POINTER_HEIGHT: f32 = 44.0;
/// How long a click's ripple spreads.
const RIPPLE: Micros = Micros(500_000);
/// How long a key press stays on screen.
const KEY_HOLD: Micros = Micros(1_100_000);
/// Typed keys closer than this form one run of text.
const TYPING_GAP: Micros = Micros(800_000);
/// The longest run of typed text the label shows, its end.
const RUN_CHARS: usize = 28;

/// What a segment draws beyond its recorded tracks: the format's effects, the take's event log,
/// the files its overlays name (code-diff snapshots and b-roll stills, by their path in the
/// video folder) and the video's theme.
#[derive(Debug, Clone, Default)]
pub struct Look {
    pub effects: Effects,
    pub events: Option<Arc<EventLog>>,
    pub diffs: HashMap<String, Arc<CodeDiff>>,
    pub images: HashMap<String, ImageData>,
    pub theme: Arc<Theme>,
    /// The segment's take, which chapter titles and brand templates are looked up by.
    pub take: Option<Id>,
}

/// One moment of a segment.
#[derive(Debug, Clone, Copy)]
pub struct Shot<'a> {
    pub layout: Layout,
    pub zooms: &'a [Zoom],
    pub overlays: &'a [Overlay],
    /// Media time of the take.
    pub time: Micros,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub scene: Scene,
    pub drawing: Drawing,
    /// When a b-roll clip covers the frame, the file and time its screen layer shows instead
    /// of the take's screen.
    pub cutaway: Option<Cutaway>,
}

/// The frame `shot` shows on `canvas`, for sources of `sizes`.
pub fn compose(
    shot: &Shot,
    look: &Look,
    canvas: Canvas,
    sizes: Sources<(u32, u32)>,
) -> Result<Frame, UnsupportedLayout> {
    let effects = &look.effects;
    let spec = LayoutSpec::styled(shot.layout, &effects.window)?;
    let scene_at = |t| {
        let punch = punch_at(shot.zooms, t, effects.zoom_motion, sizes.screen);
        spec.scene(canvas, sizes, punch)
    };
    let mut scene = scene_at(shot.time);
    if effects.zoom_motion == ZoomMotion::Smooth && !shot.zooms.is_empty() {
        let [open, close] = materials::shutter(shot.time).map(scene_at);
        for (layer, (a, b)) in scene
            .layers
            .iter_mut()
            .zip(open.layers.iter().zip(&close.layers))
        {
            layer.shutter = Rect::new(
                b.crop.left - a.crop.left,
                b.crop.top - a.crop.top,
                b.crop.right - a.crop.right,
                b.crop.bottom - a.crop.bottom,
            );
        }
    }
    let s = canvas.width as f32 / 3840.0;
    let mut drawing = Drawing::default();
    let screen = scene.layers.iter().find(|l| l.source == Source::Screen);
    if let (Some(style), Some(layer)) = (spec.window, screen) {
        drawing.under = window_decor(layer.frame, style.shadow, style.frame, s);
    }
    if let Some(layer) = screen {
        let view = ScreenView::new(layer, sizes.screen);
        if let Some(events) = &look.events {
            let on_screen = cursor(&view, events, effects, shot.time);
            if !on_screen.is_empty() {
                drawing.over.push(Shape::Clip {
                    rect: layer.frame,
                    radius: layer.radius,
                    shapes: on_screen,
                });
            }
            drawing
                .over
                .extend(keystrokes(events, effects.keystrokes, shot.time, canvas));
        }
    }
    let mut screen = screen.map(|layer| (*layer, ScreenView::new(layer, sizes.screen)));
    let mut cutaway = None;
    for overlay in shot.overlays {
        overlay::apply(
            overlay,
            &mut Stage {
                t: shot.time,
                canvas,
                look,
                screen: &mut screen,
                scene: &mut scene,
                drawing: &mut drawing,
                cutaway: &mut cutaway,
            },
        );
    }
    Ok(Frame {
        scene,
        drawing,
        cutaway,
    })
}

/// Maps points of the screen track to the canvas through the screen's layer.
#[derive(Debug, Clone)]
pub(crate) struct ScreenView {
    frame: Rect,
    crop: Rect,
    size: [f32; 2],
}

impl ScreenView {
    pub(crate) fn new(layer: &Layer, (width, height): (u32, u32)) -> Self {
        Self {
            frame: layer.frame,
            crop: layer.crop,
            size: [width as f32, height as f32],
        }
    }

    /// A rectangle of the screen track on the canvas.
    pub(crate) fn map_rect(&self, r: Rect) -> Rect {
        let [left, top] = self.map([r.left, r.top]);
        let [right, bottom] = self.map([r.right, r.bottom]);
        Rect::new(left, top, right, bottom)
    }

    pub(crate) fn map(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.frame.left
                + (p[0] / self.size[0] - self.crop.left) / self.crop.width() * self.frame.width(),
            self.frame.top
                + (p[1] / self.size[1] - self.crop.top) / self.crop.height() * self.frame.height(),
        ]
    }

    /// Canvas pixels per screen pixel.
    pub(crate) fn scale(&self) -> f32 {
        self.frame.width() / (self.crop.width() * self.size[0])
    }
}

fn cursor(view: &ScreenView, events: &EventLog, effects: &Effects, t: Micros) -> Vec<Shape> {
    let style = effects.cursor;
    let mut shapes = Vec::new();
    if style.ripple {
        for click in events.clicks() {
            if click.t > t {
                break;
            }
            let age = t.0 - click.t.0;
            if age >= RIPPLE.0 {
                continue;
            }
            let p = age as f32 / RIPPLE.0 as f32;
            let eased = 1.0 - (1.0 - p) * (1.0 - p);
            shapes.push(Shape::Ring {
                center: view.map(click.at),
                radius: (12.0 + 58.0 * eased) * view.scale(),
                width: 6.0 * view.scale(),
                color: Rgba::hex(0xC8F25A, 0.9 * (1.0 - p)),
            });
        }
    }
    if style.size > 0
        && let Some(at) = events.cursor(t, style.smoothing)
    {
        shapes.push(Shape::Pointer {
            tip: view.map(at),
            height: POINTER_HEIGHT * f32::from(style.size) / 100.0 * view.scale(),
        });
    }
    shapes
}

fn window_decor(frame: Rect, shadow: Shadow, device: DeviceFrame, s: f32) -> Vec<Shape> {
    let b = bezel(device);
    let body = Rect::new(
        frame.left - b.left * s,
        frame.top - b.top * s,
        frame.right + b.right * s,
        frame.bottom + b.bottom * s,
    );
    let mut shapes = Vec::new();
    let (blur, offset, alpha) = match shadow {
        Shadow::None => (0.0, 0.0, 0.0),
        Shadow::Soft => (70.0, 30.0, 0.55),
        Shadow::Hard => (6.0, 18.0, 0.75),
    };
    if alpha > 0.0 {
        shapes.push(Shape::Shadow {
            rect: Rect::new(
                body.left,
                body.top + offset * s,
                body.right,
                body.bottom + offset * s,
            ),
            radius: 24.0 * s,
            blur: blur * s,
            color: Rgba::hex(0x000000, alpha),
        });
    }
    match device {
        DeviceFrame::None => {}
        DeviceFrame::Macbook => {
            let lid = Rect::new(
                body.left + 40.0 * s,
                body.top,
                body.right - 40.0 * s,
                body.bottom - 40.0 * s,
            );
            shapes.push(Shape::Rect {
                rect: lid,
                radius: 36.0 * s,
                color: Rgba::hex(0x1B1C1F, 1.0),
            });
            shapes.push(Shape::Rect {
                rect: Rect::new(body.left, lid.bottom, body.right, body.bottom),
                radius: 18.0 * s,
                color: Rgba::hex(0xA7ABB3, 1.0),
            });
        }
        DeviceFrame::Iphone => {
            shapes.push(Shape::Rect {
                rect: body,
                radius: 120.0 * s,
                color: Rgba::hex(0x0B0B0C, 1.0),
            });
            let x = frame.left - 55.0 * s;
            let y = (frame.top + frame.bottom) / 2.0;
            shapes.push(Shape::Rect {
                rect: Rect::new(x - 16.0 * s, y - 60.0 * s, x + 16.0 * s, y + 60.0 * s),
                radius: 16.0 * s,
                color: Rgba::hex(0x000000, 1.0),
            });
        }
    }
    shapes
}

/// The keystroke label at `t`, if a key press is on screen.
fn keystrokes(events: &EventLog, mode: Keystrokes, t: Micros, canvas: Canvas) -> Option<Shape> {
    let text = key_label(events.keys(), mode, t)?;
    let s = canvas.width as f32 / 3840.0;
    Some(Shape::Label {
        center: [canvas.width as f32 / 2.0, canvas.height as f32 * 0.9],
        text,
        size: 64.0 * s,
        color: Rgba::hex(0xFFFFFF, 1.0),
        background: Rgba::hex(0x0A0B0D, 0.85),
    })
}

/// What the keystroke overlay shows at `t`: the latest shortcut, or with `all` the run of typed
/// text up to the latest key.
pub fn key_label(keys: &[KeyPress], mode: Keystrokes, t: Micros) -> Option<String> {
    if mode == Keystrokes::Off {
        return None;
    }
    let shown = |k: &&KeyPress| mode == Keystrokes::All || is_shortcut(k);
    let end = keys.partition_point(|k| k.t <= t);
    let index = keys[..end].iter().rposition(|k| shown(&k))?;
    let last = &keys[index];
    if t.0 - last.t.0 >= KEY_HOLD.0 {
        return None;
    }
    if mode == Keystrokes::Shortcuts || is_shortcut(last) || typed(last).is_none() {
        return Some(combo(last));
    }
    let mut start = index;
    while start > 0 {
        let previous = &keys[start - 1];
        let joins = !is_shortcut(previous)
            && typed(previous).is_some()
            && keys[start].t.0 - previous.t.0 < TYPING_GAP.0;
        if !joins {
            break;
        }
        start -= 1;
    }
    let run: String = keys[start..=index].iter().filter_map(typed).collect();
    let skip = run.chars().count().saturating_sub(RUN_CHARS);
    Some(run.chars().skip(skip).collect())
}

/// A key held with a modifier. Shift on its own only types a capital, so it does not count.
fn is_shortcut(key: &KeyPress) -> bool {
    key.mods.iter().any(|m| *m != Modifier::Shift) || (!key.mods.is_empty() && typed(key).is_none())
}

/// The text a key types in a run, if it types any.
fn typed(key: &KeyPress) -> Option<char> {
    let mut chars = key.key.chars();
    match (chars.next(), chars.next()) {
        _ if key.key == "space" => Some(' '),
        (Some(c), None) if key.mods.contains(&Modifier::Shift) => c.to_uppercase().next(),
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// A key with its modifiers in the macOS order, `⌃⌥⇧⌘K`.
fn combo(key: &KeyPress) -> String {
    let mut out = String::new();
    for (m, symbol) in [
        (Modifier::Fn, "fn "),
        (Modifier::Ctrl, "⌃"),
        (Modifier::Alt, "⌥"),
        (Modifier::Shift, "⇧"),
        (Modifier::Cmd, "⌘"),
    ] {
        if key.mods.contains(&m) {
            out.push_str(symbol);
        }
    }
    let name = match key.key.as_str() {
        "space" => "Space".to_owned(),
        "enter" => "↩".to_owned(),
        "tab" => "⇥".to_owned(),
        "escape" => "esc".to_owned(),
        "backspace" => "⌫".to_owned(),
        "delete" => "⌦".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        other => other.to_uppercase(),
    };
    out.push_str(&name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{CursorStyle, Point, TimeRange, ZoomMotion};

    const SIZES: Sources<(u32, u32)> = Sources {
        screen: (3840, 2160),
        camera: (1920, 1080),
    };

    fn look(events: &str, effects: Effects) -> Look {
        Look {
            effects,
            events: Some(Arc::new(EventLog::parse(events).unwrap())),
            diffs: HashMap::new(),
            images: HashMap::new(),
            ..Look::default()
        }
    }

    fn pointer(frame: &Frame) -> Option<([f32; 2], f32)> {
        frame.drawing.over.iter().find_map(|s| match s {
            Shape::Clip { shapes, .. } => shapes.iter().find_map(|s| match s {
                Shape::Pointer { tip, height } => Some((*tip, *height)),
                _ => None,
            }),
            _ => None,
        })
    }

    #[test]
    fn the_cursor_follows_the_zoom_and_its_style_without_new_media() {
        let events = "{\"t\":0,\"kind\":\"move\",\"x\":1000,\"y\":500}\n";
        let zooms = [Zoom {
            scale: 200,
            range: TimeRange::new(Micros(0), Micros(1_000_000)).unwrap(),
            center: Some(Point { x: 1000, y: 500 }),
            by: None,
        }];
        let shot = Shot {
            layout: Layout::Content,
            zooms: &zooms,
            overlays: &[],
            time: Micros(500_000),
        };
        let plain = compose(&shot, &look(events, Effects::default()), Canvas::UHD, SIZES).unwrap();
        // Zoomed 2x on (1000, 500): centered across, the top edge clamps it down. The cursor
        // doubles in size.
        let (tip, height) = pointer(&plain).unwrap();
        assert!((tip[0] - 1920.0).abs() < 0.01 && (tip[1] - 1000.0).abs() < 0.01);
        assert!((height - 88.0).abs() < 0.01);
        let bigger = Effects {
            cursor: CursorStyle {
                size: 150,
                ..Effects::default().cursor
            },
            zoom_motion: ZoomMotion::Smooth,
            ..Effects::default()
        };
        let styled = compose(&shot, &look(events, bigger), Canvas::UHD, SIZES).unwrap();
        assert!((pointer(&styled).unwrap().1 - 132.0).abs() < 0.01);
        let face = Shot {
            layout: Layout::Face,
            ..shot
        };
        let hidden = compose(&face, &look(events, Effects::default()), Canvas::UHD, SIZES).unwrap();
        assert_eq!(pointer(&hidden), None);
    }

    #[test]
    fn ripples_spread_from_each_click_while_it_is_recent() {
        let events = "{\"t\":0,\"kind\":\"down\",\"x\":100,\"y\":100,\"button\":\"left\"}\n";
        let effects = Effects {
            cursor: CursorStyle {
                ripple: true,
                ..Effects::default().cursor
            },
            ..Effects::default()
        };
        let rings = |t| {
            let shot = Shot {
                layout: Layout::Content,
                zooms: &[],
                overlays: &[],
                time: Micros(t),
            };
            let frame = compose(&shot, &look(events, effects), Canvas::UHD, SIZES).unwrap();
            format!("{:?}", frame.drawing).matches("Ring").count()
        };
        assert_eq!(rings(100_000), 1);
        assert_eq!(rings(600_000), 0);
    }

    #[test]
    fn keystrokes_show_shortcuts_or_the_typed_run() {
        let key = |t, key: &str, mods: Vec<Modifier>| KeyPress {
            t: Micros(t),
            key: key.into(),
            mods,
        };
        let keys = [
            key(0, "k", vec![Modifier::Cmd]),
            key(2_000_000, "c", vec![]),
            key(2_100_000, "d", vec![Modifier::Shift]),
            key(2_200_000, "space", vec![]),
            key(2_300_000, "x", vec![]),
        ];
        let at = |mode, t| key_label(&keys, mode, Micros(t));
        assert_eq!(at(Keystrokes::Shortcuts, 500_000).as_deref(), Some("⌘K"));
        assert_eq!(at(Keystrokes::Shortcuts, 2_300_000), None);
        assert_eq!(at(Keystrokes::All, 2_300_000).as_deref(), Some("cD x"));
        assert_eq!(at(Keystrokes::Off, 500_000), None);
    }
}
