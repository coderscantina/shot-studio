//! Where each source sits in a layout, and the scene a frame draws. Pure geometry, no GPU.
//!
//! Geometry is designed on a 3840x2160 canvas and scales with the canvas width:
//!
//! | Layout | Screen | Camera |
//! | --- | --- | --- |
//! | `face` | not shown | fills the canvas, cropped to fit |
//! | `content` | fills the canvas | not shown |
//! | `face-content` | fills the canvas | 864x486 inset, 72 px from the bottom right corner, 36 px corners |
//! | `split` | 2560x1440 panel on the left | 1080x1440 panel on the right, 48 px gap, both 32 px corners, centered |
//! | `window` | the captured window, 16:9, inside the style's padding and device frame | not shown |
//!
//! Sources are scaled to cover their slot and cropped around the center. A zoom punches in on the
//! layout's primary slot: the camera in `face`, the screen everywhere else. On the screen it
//! centers on the zoom's point, as far as the slot's crop allows.
//!
//! Capture does not record window bounds yet, so `window` shows the whole screen as the window.
//!
//! The brand's `camera` node frames the camera wherever it sits beside the screen, in
//! `face-content`, `split` and every channel layout with both tracks
//! ([`LayoutSpec::with_camera`]): its shape and corners, a border in the accent color and a
//! shadow that falls away from the scene's light. `face` fills the canvas and has no frame to
//! style.
//!
//! A channel layout ([`LayoutSpec::custom`]) places its slots on the same design canvas, with a
//! crop focus per slot, so it draws, zooms and morphs through the same code as the built-ins.
//! [`LayoutSpec::morph`] springs every slot of one layout into the next for a scene switch.

use shot_core::{
    CameraShape, CustomLayout, DeviceFrame, Effects, Layout, Shadow, Track, WindowStyle,
};

use crate::materials::LIGHT;
use crate::zoom::Punch;

/// The size of the output in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
}

impl Canvas {
    /// 3840x2160, the main cut's output.
    pub const UHD: Self = Self {
        width: 3840,
        height: 2160,
    };
}

/// A recorded track a layer draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Screen,
    Camera,
}

/// One value per source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sources<T> {
    pub screen: T,
    pub camera: T,
}

impl<T> Sources<T> {
    pub fn get(&self, source: Source) -> &T {
        match source {
            Source::Screen => &self.screen,
            Source::Camera => &self.camera,
        }
    }
}

/// An axis-aligned rectangle by its left, top, right and bottom edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub const UNIT: Self = Self::new(0.0, 0.0, 1.0, 1.0);
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(self) -> f32 {
        self.right - self.left
    }

    pub fn height(self) -> f32 {
        self.bottom - self.top
    }

    fn scaled(self, factor: f32) -> Self {
        Self::new(
            self.left * factor,
            self.top * factor,
            self.right * factor,
            self.bottom * factor,
        )
    }

    /// Shrinks the rectangle around its center by `factor`.
    fn shrunk(self, factor: f32) -> Self {
        let center = [
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        ];
        self.shrunk_at(factor, center)
    }

    /// Shrinks the rectangle by `factor` around `center`, moved as little as needed to stay
    /// inside the rectangle.
    fn shrunk_at(self, factor: f32, center: [f32; 2]) -> Self {
        let (hw, hh) = (self.width() / 2.0 / factor, self.height() / 2.0 / factor);
        let cx = center[0].clamp(self.left + hw, self.right - hw);
        let cy = center[1].clamp(self.top + hh, self.bottom - hh);
        Self::new(cx - hw, cy - hh, cx + hw, cy + hh)
    }

    pub fn contains(self, point: [f32; 2]) -> bool {
        (self.left..=self.right).contains(&point[0]) && (self.top..=self.bottom).contains(&point[1])
    }

    /// The rectangle `t` of the way from this one to `to`.
    pub fn lerp(self, to: Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Self::new(
            mix(self.left, to.left),
            mix(self.top, to.top),
            mix(self.right, to.right),
            mix(self.bottom, to.bottom),
        )
    }
}

/// Where one source goes in a layout, on the 3840x2160 design canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub source: Source,
    /// Destination in design canvas pixels.
    pub frame: Rect,
    /// Corner radius in design canvas pixels.
    pub radius: f32,
    /// In design canvas pixels, like the shadow's offset and blur.
    pub border: Option<Border>,
    pub shadow: Option<DropShadow>,
    /// Where the crop centers in the source, as a fraction of its width and height.
    pub focus: [f32; 2],
    /// 1 for an opaque slot; below while a morph fades it in or out.
    pub opacity: f32,
}

impl Slot {
    fn new(source: Source, frame: Rect, radius: f32) -> Self {
        Self {
            source,
            frame,
            radius,
            border: None,
            shadow: None,
            focus: [0.5, 0.5],
            opacity: 1.0,
        }
    }
}

/// How the brand frames the camera, in design canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraFrame {
    pub shape: CameraShape,
    pub corners: f32,
    /// 0 for none.
    pub border: f32,
    /// Gamma-encoded RGB of the border.
    pub color: [f32; 3],
    pub shadow: Shadow,
}

impl CameraFrame {
    /// The brand's `camera` node, its border in `accent`.
    pub fn of(camera: &shot_core::Camera, accent: [f32; 3]) -> Self {
        Self {
            shape: camera.shape,
            corners: f32::from(camera.corners),
            border: f32::from(camera.border),
            color: accent,
            shadow: camera.shadow,
        }
    }

    /// The shadow of a camera frame lifted off the canvas, falling away from the light.
    fn drop_shadow(self) -> Option<DropShadow> {
        let (elevation, blur, alpha) = match self.shadow {
            Shadow::None => return None,
            Shadow::Soft => (48.0, 40.0, 0.55),
            Shadow::Hard => (18.0, 3.0, 0.75),
        };
        Some(DropShadow {
            offset: LIGHT.shadow_offset(elevation),
            blur,
            alpha,
        })
    }
}

/// The fixed arrangement of a layout. Slots draw in order, later ones on top.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutSpec {
    pub slots: Vec<Slot>,
    /// The slot a zoom punches in on.
    pub primary: usize,
    /// The window layout's style, which draws a background, shadow and device frame.
    pub window: Option<WindowStyle>,
}

/// A layout the renderer does not draw: a built-in it does not draw yet, or a channel layout
/// the brand does not define.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedLayout(pub Layout);

impl std::fmt::Display for UnsupportedLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Layout::Custom(name) => write!(f, "the brand defines no layout `{name}`"),
            layout => write!(f, "the {layout} layout is not rendered yet"),
        }
    }
}

impl std::error::Error for UnsupportedLayout {}

const DESIGN_WIDTH: f32 = 3840.0;
const FULL: Rect = Rect::new(0.0, 0.0, 3840.0, 2160.0);

impl LayoutSpec {
    /// The layout as drawn with the default window style.
    pub fn of(layout: Layout) -> Result<Self, UnsupportedLayout> {
        Self::styled(layout, &Effects::default().window)
    }

    /// The layout, with `window` placing the captured window in the `window` layout and
    /// `custom` holding the channel's layouts.
    pub fn resolve(
        layout: Layout,
        window: &WindowStyle,
        custom: &[CustomLayout],
    ) -> Result<Self, UnsupportedLayout> {
        match layout {
            Layout::Custom(name) => custom
                .iter()
                .find(|c| c.name == name)
                .map(Self::custom)
                .ok_or(UnsupportedLayout(layout)),
            _ => Self::styled(layout, window),
        }
    }

    /// A channel layout's slots in their order. A zoom punches in on the screen, or on the
    /// camera where the screen is not shown.
    pub fn custom(layout: &CustomLayout) -> Self {
        let slots: Vec<Slot> = layout
            .slots
            .iter()
            .map(|s| {
                let (x, y) = (f32::from(s.x), f32::from(s.y));
                let frame = Rect::new(x, y, x + f32::from(s.width), y + f32::from(s.height));
                let source = match s.track {
                    Track::Screen => Source::Screen,
                    Track::Camera => Source::Camera,
                };
                Slot {
                    focus: s.focus.map(|f| f32::from(f) / 100.0),
                    ..Slot::new(source, frame, f32::from(s.corners))
                }
            })
            .collect();
        let primary = slots
            .iter()
            .position(|s| s.source == Source::Screen)
            .unwrap_or(0);
        Self {
            slots,
            primary,
            window: None,
        }
    }

    /// A built-in layout, with `window` placing the captured window in the `window` layout.
    pub fn styled(layout: Layout, window: &WindowStyle) -> Result<Self, UnsupportedLayout> {
        let slot = Slot::new;
        let spec = match layout {
            Layout::Face => Self {
                slots: vec![slot(Source::Camera, FULL, 0.0)],
                primary: 0,
                window: None,
            },
            Layout::Content => Self {
                slots: vec![slot(Source::Screen, FULL, 0.0)],
                primary: 0,
                window: None,
            },
            Layout::FaceContent => Self {
                slots: vec![
                    slot(Source::Screen, FULL, 0.0),
                    slot(
                        Source::Camera,
                        Rect::new(2904.0, 1602.0, 3768.0, 2088.0),
                        36.0,
                    ),
                ],
                primary: 0,
                window: None,
            },
            Layout::Split => Self {
                slots: vec![
                    slot(Source::Screen, Rect::new(76.0, 360.0, 2636.0, 1800.0), 32.0),
                    slot(
                        Source::Camera,
                        Rect::new(2684.0, 360.0, 3764.0, 1800.0),
                        32.0,
                    ),
                ],
                primary: 0,
                window: None,
            },
            Layout::Window => Self {
                slots: vec![slot(
                    Source::Screen,
                    window_frame(window),
                    f32::from(window.corners),
                )],
                primary: 0,
                window: Some(*window),
            },
            Layout::Screen | Layout::FaceCorner | Layout::Custom(_) => {
                return Err(UnsupportedLayout(layout));
            }
        };
        Ok(spec)
    }

    /// The layout with `camera` framing its camera slot when the camera sits beside the screen.
    /// A circle is the largest square of the slot: in a corner over the screen it keeps to that
    /// corner, elsewhere it is centered.
    pub fn with_camera(mut self, camera: CameraFrame) -> Self {
        let screen = self
            .slots
            .iter()
            .find(|s| s.source == Source::Screen)
            .map(|s| s.frame);
        let Some(screen) = screen else {
            return self;
        };
        for slot in self.slots.iter_mut().filter(|s| s.source == Source::Camera) {
            let f = slot.frame;
            slot.radius = match camera.shape {
                CameraShape::Rounded => camera.corners.min(f.width().min(f.height()) / 2.0),
                CameraShape::Square => 0.0,
                CameraShape::Circle => {
                    let side = f.width().min(f.height());
                    let over_screen =
                        screen.contains([f.left, f.top]) && screen.contains([f.right, f.bottom]);
                    let (cx, cy) = ((f.left + f.right) / 2.0, (f.top + f.bottom) / 2.0);
                    let (left, top) = if over_screen {
                        let left = if cx > DESIGN_WIDTH / 2.0 {
                            f.right - side
                        } else {
                            f.left
                        };
                        let top = if cy > FULL.bottom / 2.0 {
                            f.bottom - side
                        } else {
                            f.top
                        };
                        (left, top)
                    } else {
                        (cx - side / 2.0, cy - side / 2.0)
                    };
                    slot.frame = Rect::new(left, top, left + side, top + side);
                    side / 2.0
                }
            };
            slot.border = (camera.border > 0.0).then_some(Border {
                width: camera.border,
                color: camera.color,
            });
            slot.shadow = camera.drop_shadow();
        }
        self
    }

    /// Whether any slot draws `source`.
    pub fn uses(&self, source: Source) -> bool {
        self.slots.iter().any(|slot| slot.source == source)
    }

    /// The layout `t` of the way from `self` to `to`, `t` from 0 to 1 and beyond while a
    /// spring overshoots. A source both show moves its frame, corners, crop focus, border and
    /// shadow; one only one side shows fades and scales by 8 % on its own frame. Slots stack
    /// by their place in each layout, so a slot rising to the top passes the other halfway.
    /// The window style and background come from the side the morph is nearer.
    pub fn morph(&self, to: &Self, t: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        let fade = t.clamp(0.0, 1.0);
        fn find(spec: &LayoutSpec, source: Source) -> Option<(usize, &Slot)> {
            spec.slots
                .iter()
                .enumerate()
                .find(|(_, s)| s.source == source)
        }
        let depth = |spec: &Self, index: usize| index as f32 / spec.slots.len().max(1) as f32;
        let mut slots: Vec<(f32, Slot)> = [Source::Screen, Source::Camera]
            .into_iter()
            .filter_map(|source| match (find(self, source), find(to, source)) {
                (Some((i, a)), Some((j, b))) => Some((
                    mix(depth(self, i), depth(to, j)),
                    Slot {
                        source,
                        frame: a.frame.lerp(b.frame, t),
                        radius: mix(a.radius, b.radius).max(0.0),
                        border: lerp_border(a.border, b.border, fade),
                        shadow: lerp_shadow(a.shadow, b.shadow, fade),
                        focus: [mix(a.focus[0], b.focus[0]), mix(a.focus[1], b.focus[1])],
                        opacity: mix(a.opacity, b.opacity),
                    },
                )),
                (Some((i, a)), None) => Some((depth(self, i), a.faded(1.0 - fade))),
                (None, Some((j, b))) => Some((depth(to, j), b.faded(fade))),
                (None, None) => None,
            })
            .collect();
        slots.retain(|(_, s)| s.opacity > 0.0);
        slots.sort_by(|a, b| a.0.total_cmp(&b.0));
        let slots: Vec<Slot> = slots.into_iter().map(|(_, s)| s).collect();
        let near = if t < 0.5 { self } else { to };
        Self {
            primary: slots
                .iter()
                .position(|s| Some(s.source) == near.slots.get(near.primary).map(|p| p.source))
                .unwrap_or(0),
            slots,
            window: near.window,
        }
    }

    /// The scene of one frame: slots scaled to the canvas, each source cropped to cover its slot,
    /// and the primary slot punched in by `punch`. The camera ignores the punch's center.
    pub fn scene(&self, canvas: Canvas, sizes: Sources<(u32, u32)>, punch: Punch) -> Scene {
        let scale = canvas.width as f32 / DESIGN_WIDTH;
        let layers = self
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let frame = slot.frame.scaled(scale);
                let (width, height) = *sizes.get(slot.source);
                let mut crop = cover(frame, width as f32 / height as f32, slot.focus);
                if index == self.primary && punch.scale > 1.0 {
                    crop = match slot.source {
                        Source::Camera => crop.shrunk(punch.scale),
                        Source::Screen => {
                            let at = [
                                punch.center[0] / width as f32,
                                punch.center[1] / height as f32,
                            ];
                            crop.shrunk_at(punch.scale, at)
                        }
                    };
                }
                Layer {
                    border: slot.border.map(|b| Border {
                        width: b.width * scale,
                        ..b
                    }),
                    shadow: slot.shadow.map(|s| DropShadow {
                        offset: s.offset.map(|o| o * scale),
                        blur: s.blur * scale,
                        alpha: s.alpha,
                    }),
                    opacity: slot.opacity,
                    ..Layer::new(slot.source, frame, crop, slot.radius * scale)
                }
            })
            .collect();
        Scene {
            background: self.window.map_or(BACKGROUND, |w| srgb(w.background.0)),
            layers,
        }
    }
}

/// Where the window goes on the design canvas: 16:9, as large as the padding and the device
/// frame's bezel allow, centered.
fn window_frame(style: &WindowStyle) -> Rect {
    let padding = f32::from(style.padding);
    let bezel = bezel(style.frame);
    let room = Rect::new(
        padding + bezel.left,
        padding + bezel.top,
        DESIGN_WIDTH - padding - bezel.right,
        FULL.bottom - padding - bezel.bottom,
    );
    let width = room.width().min(room.height() * 16.0 / 9.0);
    let height = width * 9.0 / 16.0;
    let (cx, cy) = (
        (room.left + room.right) / 2.0,
        (room.top + room.bottom) / 2.0,
    );
    Rect::new(
        cx - width / 2.0,
        cy - height / 2.0,
        cx + width / 2.0,
        cy + height / 2.0,
    )
}

/// The bezel a device frame draws around the window on each side, in design canvas pixels.
pub fn bezel(frame: DeviceFrame) -> Rect {
    match frame {
        DeviceFrame::None => Rect::new(0.0, 0.0, 0.0, 0.0),
        // A laptop lid, with the wider base below it.
        DeviceFrame::Macbook => Rect::new(96.0, 56.0, 96.0, 96.0),
        DeviceFrame::Iphone => Rect::new(110.0, 60.0, 110.0, 60.0),
    }
}

/// Gamma-encoded RGB of an sRGB color.
pub fn srgb(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|c| f32::from(c) / 255.0)
}

/// The canvas behind the slots, a near black with a hint of blue.
pub const BACKGROUND: [f32; 3] = [15.0 / 255.0, 17.0 / 255.0, 21.0 / 255.0];

/// The part of a source with aspect `source_aspect` that covers `frame` when scaled, centered
/// on `focus` as far as the source allows, in texture coordinates.
fn cover(frame: Rect, source_aspect: f32, focus: [f32; 2]) -> Rect {
    let frame_aspect = frame.width() / frame.height();
    let (width, height) = if source_aspect > frame_aspect {
        (frame_aspect / source_aspect, 1.0)
    } else {
        (1.0, source_aspect / frame_aspect)
    };
    let left = (focus[0] - width / 2.0).clamp(0.0, 1.0 - width);
    let top = (focus[1] - height / 2.0).clamp(0.0, 1.0 - height);
    Rect::new(left, top, left + width, top + height)
}

impl Slot {
    /// The slot at `presence`, 0 gone and 1 whole: faded, and scaled around its center from
    /// 92 % up.
    fn faded(&self, presence: f32) -> Self {
        let frame = self.frame.shrunk(1.0 / (0.92 + 0.08 * presence));
        Self {
            frame,
            opacity: self.opacity * presence,
            ..*self
        }
    }
}

fn lerp_border(a: Option<Border>, b: Option<Border>, t: f32) -> Option<Border> {
    let zero =
        |o: Option<Border>, other: Option<Border>| o.or(other.map(|b| Border { width: 0.0, ..b }));
    let (a, b) = (zero(a, b)?, zero(b, a)?);
    let mix = |x: f32, y: f32| x + (y - x) * t;
    Some(Border {
        width: mix(a.width, b.width),
        color: [0, 1, 2].map(|i| mix(a.color[i], b.color[i])),
    })
}

fn lerp_shadow(a: Option<DropShadow>, b: Option<DropShadow>, t: f32) -> Option<DropShadow> {
    let zero = |o: Option<DropShadow>, other: Option<DropShadow>| {
        o.or(other.map(|s| DropShadow { alpha: 0.0, ..s }))
    };
    let (a, b) = (zero(a, b)?, zero(b, a)?);
    let mix = |x: f32, y: f32| x + (y - x) * t;
    Some(DropShadow {
        offset: [mix(a.offset[0], b.offset[0]), mix(a.offset[1], b.offset[1])],
        blur: mix(a.blur, b.blur),
        alpha: mix(a.alpha, b.alpha),
    })
}

/// What one frame draws, in canvas pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Gamma-encoded BT.709 RGB.
    pub background: [f32; 3],
    pub layers: Vec<Layer>,
}

/// One source drawn into a rectangle of the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    pub source: Source,
    /// Destination in canvas pixels.
    pub frame: Rect,
    /// The part of the source shown, in texture coordinates.
    pub crop: Rect,
    /// Corner radius in canvas pixels.
    pub radius: f32,
    /// The size in canvas pixels of the blocks the layer is averaged over, 0 for a sharp one.
    pub blur: f32,
    /// How `crop` changes from the shutter's opening to its closing, for motion blur. Zero when
    /// the layer holds still.
    pub shutter: Rect,
    /// 1 for an opaque layer.
    pub opacity: f32,
    pub border: Option<Border>,
    pub shadow: Option<DropShadow>,
    /// How far the source turns inside `crop`, around the crop's center, in radians: the
    /// hand-held camera's roll.
    pub turn: f32,
}

/// A border drawn inside a layer's edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Border {
    /// In canvas pixels.
    pub width: f32,
    /// Gamma-encoded RGB.
    pub color: [f32; 3],
}

/// A black shadow of a layer's shape under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropShadow {
    /// From the layer to its shadow, in canvas pixels.
    pub offset: [f32; 2],
    /// Standard deviation of the shadow's edge, in canvas pixels.
    pub blur: f32,
    pub alpha: f32,
}

impl Layer {
    /// A sharp, opaque layer that holds still.
    pub fn new(source: Source, frame: Rect, crop: Rect, radius: f32) -> Self {
        Self {
            source,
            frame,
            crop,
            radius,
            blur: 0.0,
            shutter: Rect::ZERO,
            opacity: 1.0,
            border: None,
            shadow: None,
            turn: 0.0,
        }
    }

    /// What drawing the layer covers on the canvas: its frame, and its shadow.
    pub fn extent(&self) -> Rect {
        let Some(shadow) = self.shadow else {
            return self.frame;
        };
        let reach = shadow.blur * 3.0;
        let f = self.frame;
        Rect::new(
            f.left.min(f.left + shadow.offset[0]) - reach,
            f.top.min(f.top + shadow.offset[1]) - reach,
            f.right.max(f.right + shadow.offset[0]) + reach,
            f.bottom.max(f.bottom + shadow.offset[1]) + reach,
        )
    }

    /// How far the shown content moves on the canvas during the shutter, in canvas pixels.
    pub fn travel(&self) -> f32 {
        let s = self.shutter;
        let across = self.frame.width() / self.crop.width();
        let down = self.frame.height() / self.crop.height();
        (s.left.abs().max(s.right.abs()) * across).max(s.top.abs().max(s.bottom.abs()) * down)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: Sources<(u32, u32)> = Sources {
        screen: (3840, 2160),
        camera: (1920, 1080),
    };
    const NONE: Punch = Punch {
        scale: 1.0,
        center: [1920.0, 1080.0],
    };

    fn centered(scale: f32) -> Punch {
        Punch { scale, ..NONE }
    }

    fn assert_close(a: Rect, b: Rect) {
        let pairs = [
            (a.left, b.left),
            (a.top, b.top),
            (a.right, b.right),
            (a.bottom, b.bottom),
        ];
        for (x, y) in pairs {
            assert!((x - y).abs() < 1e-4, "{a:?} is not {b:?}");
        }
    }

    #[test]
    fn face_and_content_fill_the_canvas_with_one_source() {
        let face = LayoutSpec::of(Layout::Face).unwrap();
        let scene = face.scene(Canvas::UHD, SIZES, NONE);
        assert_eq!(scene.layers.len(), 1);
        assert_eq!(scene.layers[0].source, Source::Camera);
        assert_close(scene.layers[0].frame, FULL);
        assert_close(scene.layers[0].crop, Rect::UNIT);
        assert!(!face.uses(Source::Screen));

        let content = LayoutSpec::of(Layout::Content).unwrap();
        assert_eq!(
            content.scene(Canvas::UHD, SIZES, NONE).layers[0].source,
            Source::Screen
        );
    }

    #[test]
    fn slots_crop_their_source_to_cover_and_scale_with_the_canvas() {
        let split = LayoutSpec::of(Layout::Split).unwrap();
        let half = Canvas {
            width: 1920,
            height: 1080,
        };
        let scene = split.scene(half, SIZES, NONE);
        let (screen, camera) = (scene.layers[0], scene.layers[1]);
        assert_close(screen.frame, Rect::new(38.0, 180.0, 1318.0, 900.0));
        assert_close(screen.crop, Rect::UNIT);
        assert_eq!(camera.radius, 16.0);
        // A 3:4 slot shows the middle 42 % of a 16:9 camera.
        let width = 0.75 / (16.0 / 9.0);
        assert_close(
            camera.crop,
            Rect::new((1.0 - width) / 2.0, 0.0, (1.0 + width) / 2.0, 1.0),
        );
    }

    #[test]
    fn zoom_punches_in_on_the_primary_slot_only() {
        let face_content = LayoutSpec::of(Layout::FaceContent).unwrap();
        let scene = face_content.scene(Canvas::UHD, SIZES, centered(1.25));
        assert_close(scene.layers[0].crop, Rect::new(0.1, 0.1, 0.9, 0.9));
        assert_close(scene.layers[1].crop, Rect::UNIT);

        let face = LayoutSpec::of(Layout::Face).unwrap();
        let scene = face.scene(Canvas::UHD, SIZES, centered(2.0));
        assert_close(scene.layers[0].crop, Rect::new(0.25, 0.25, 0.75, 0.75));
    }

    #[test]
    fn a_centered_zoom_stays_inside_the_source() {
        let content = LayoutSpec::of(Layout::Content).unwrap();
        let at = |center| {
            content
                .scene(Canvas::UHD, SIZES, Punch { scale: 2.0, center })
                .layers[0]
                .crop
        };
        assert_close(at([960.0, 540.0]), Rect::new(0.0, 0.0, 0.5, 0.5));
        // A point near the right edge moves the crop only as far as the edge.
        assert_close(at([3800.0, 1080.0]), Rect::new(0.5, 0.25, 1.0, 0.75));
    }

    #[test]
    fn the_window_sits_inside_padding_and_bezel_on_its_background() {
        let style = WindowStyle {
            padding: 64,
            ..Effects::default().window
        };
        let spec = LayoutSpec::styled(Layout::Window, &style).unwrap();
        let scene = spec.scene(Canvas::UHD, SIZES, NONE);
        assert_eq!(scene.background, srgb(style.background.0));
        // 2032 px of height is left, so the 16:9 window is 3612.4 px wide, centered.
        let frame = scene.layers[0].frame;
        assert_close(
            frame,
            Rect::new(1920.0 - 1806.2222, 64.0, 1920.0 + 1806.2222, 2096.0),
        );
        let framed = LayoutSpec::styled(
            Layout::Window,
            &WindowStyle {
                frame: DeviceFrame::Macbook,
                ..style
            },
        )
        .unwrap();
        assert!(framed.slots[0].frame.bottom < 2160.0 - 64.0 - 96.0 + 0.01);
    }

    #[test]
    fn the_brand_frames_the_camera_beside_the_screen() {
        let camera = CameraFrame {
            shape: CameraShape::Circle,
            corners: 24.0,
            border: 8.0,
            color: [1.0, 0.0, 0.0],
            shadow: Shadow::Soft,
        };
        let half = Canvas {
            width: 1920,
            height: 1080,
        };
        // An inset over the screen keeps to its corner, a panel beside it is centered.
        let inset = LayoutSpec::of(Layout::FaceContent)
            .unwrap()
            .with_camera(camera)
            .scene(half, SIZES, NONE);
        let circle = inset.layers[1];
        assert_close(circle.frame, Rect::new(1641.0, 801.0, 1884.0, 1044.0));
        assert_eq!(circle.radius, 121.5);
        assert_eq!(circle.border.unwrap().width, 4.0);
        let shadow = circle.shadow.unwrap();
        assert!(shadow.offset[0] > 0.0 && shadow.offset[1] > 0.0);
        assert!(circle.extent().right > circle.frame.right);
        assert_eq!(inset.layers[0].shadow, None);
        let panel = LayoutSpec::of(Layout::Split)
            .unwrap()
            .with_camera(camera)
            .scene(Canvas::UHD, SIZES, NONE)
            .layers[1];
        assert_close(panel.frame, Rect::new(2684.0, 540.0, 3764.0, 1620.0));
        let square = LayoutSpec::of(Layout::Split)
            .unwrap()
            .with_camera(CameraFrame {
                shape: CameraShape::Square,
                ..camera
            });
        assert_eq!(square.slots[1].radius, 0.0);
        let face = LayoutSpec::of(Layout::Face).unwrap();
        assert_eq!(face.clone().with_camera(camera), face);
    }

    #[test]
    fn layouts_after_06_are_refused() {
        assert_eq!(
            LayoutSpec::of(Layout::Screen),
            Err(UnsupportedLayout(Layout::Screen))
        );
    }

    fn talk_left() -> CustomLayout {
        let brand = shot_core::Brand::parse(
            "layout talk-left {\n    camera x=0 y=0 width=1536 height=2160 focus-x=0 focus-y=50\n    screen x=1632 y=486 width=2112 height=1188 corners=24\n}\n",
        )
        .unwrap();
        brand.layouts[0].clone()
    }

    #[test]
    fn a_channel_layout_draws_its_slots_with_their_crop_focus() {
        let custom = talk_left();
        let name = Layout::Custom(custom.name);
        assert_eq!(
            LayoutSpec::resolve(name, &Effects::default().window, &[]),
            Err(UnsupportedLayout(name))
        );
        let spec = LayoutSpec::resolve(
            name,
            &Effects::default().window,
            std::slice::from_ref(&custom),
        )
        .unwrap();
        assert_eq!(spec.primary, 1, "a zoom punches in on the screen");
        let scene = spec.scene(Canvas::UHD, SIZES, NONE);
        assert_eq!(scene.layers[0].source, Source::Camera);
        // A tall slot keeps the left edge of the camera with its focus at 0.
        let camera = scene.layers[0].crop;
        assert_close(camera, Rect::new(0.0, 0.0, camera.width(), 1.0));
        assert!(camera.width() < 0.5);
        assert_eq!(scene.layers[1].radius, 24.0);
    }

    #[test]
    fn a_morph_moves_shared_slots_and_fades_the_others() {
        let face = LayoutSpec::of(Layout::Face).unwrap();
        let face_content = LayoutSpec::of(Layout::FaceContent).unwrap();
        let start = face.morph(&face_content, 0.0);
        assert_eq!(start.slots.len(), 1, "the screen is not there yet");
        assert_close(start.slots[0].frame, FULL);
        let end = face.morph(&face_content, 1.0);
        assert_eq!(end.slots, face_content.slots);
        let middle = face.morph(&face_content, 0.5);
        let camera = middle
            .slots
            .iter()
            .find(|s| s.source == Source::Camera)
            .unwrap();
        let inset = face_content.slots[1].frame;
        assert_close(camera.frame, FULL.lerp(inset, 0.5));
        assert_eq!(camera.radius, 18.0);
        let screen = middle
            .slots
            .iter()
            .find(|s| s.source == Source::Screen)
            .unwrap();
        assert_eq!(screen.opacity, 0.5);
        assert!(screen.frame.width() < FULL.width());
        // The camera rises above the screen, as the inset sits on top.
        assert_eq!(middle.slots.last().unwrap().source, Source::Camera);

        let custom = LayoutSpec::custom(&talk_left());
        let to_custom = face_content.morph(&custom, 0.5);
        assert_eq!(to_custom.slots.len(), 2);
        let screen = to_custom
            .slots
            .iter()
            .find(|s| s.source == Source::Screen)
            .unwrap();
        assert_eq!(screen.focus, [0.5, 0.5]);
        let camera = to_custom
            .slots
            .iter()
            .find(|s| s.source == Source::Camera)
            .unwrap();
        assert_eq!(camera.focus, [0.25, 0.5]);
    }
}
