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
//!
//! Sources are scaled to cover their slot and cropped around the center. A zoom punches in on the
//! layout's primary slot: the camera in `face`, the screen everywhere else.

use shot_core::Layout;

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
        let (cx, cy) = (
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        );
        let (hw, hh) = (self.width() / 2.0 / factor, self.height() / 2.0 / factor);
        Self::new(cx - hw, cy - hh, cx + hw, cy + hh)
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
}

/// The fixed arrangement of a layout. Slots draw in order, later ones on top.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutSpec {
    pub slots: Vec<Slot>,
    /// The slot a zoom punches in on.
    pub primary: usize,
}

/// A layout the renderer does not draw yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the {0} layout is not rendered yet")]
pub struct UnsupportedLayout(pub Layout);

const DESIGN_WIDTH: f32 = 3840.0;
const FULL: Rect = Rect::new(0.0, 0.0, 3840.0, 2160.0);

impl LayoutSpec {
    pub fn of(layout: Layout) -> Result<Self, UnsupportedLayout> {
        let slot = |source, frame, radius| Slot {
            source,
            frame,
            radius,
        };
        let spec = match layout {
            Layout::Face => Self {
                slots: vec![slot(Source::Camera, FULL, 0.0)],
                primary: 0,
            },
            Layout::Content => Self {
                slots: vec![slot(Source::Screen, FULL, 0.0)],
                primary: 0,
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
            },
            Layout::Screen | Layout::FaceCorner | Layout::Window => {
                return Err(UnsupportedLayout(layout));
            }
        };
        Ok(spec)
    }

    /// Whether any slot draws `source`.
    pub fn uses(&self, source: Source) -> bool {
        self.slots.iter().any(|slot| slot.source == source)
    }

    /// The scene of one frame: slots scaled to the canvas, each source cropped to cover its slot,
    /// and the primary slot punched in by `zoom` (1.0 for none).
    pub fn scene(&self, canvas: Canvas, sizes: Sources<(u32, u32)>, zoom: f32) -> Scene {
        let scale = canvas.width as f32 / DESIGN_WIDTH;
        let layers = self
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let frame = slot.frame.scaled(scale);
                let (width, height) = *sizes.get(slot.source);
                let mut crop = cover(frame, width as f32 / height as f32);
                if index == self.primary && zoom > 1.0 {
                    crop = crop.shrunk(zoom);
                }
                Layer {
                    source: slot.source,
                    frame,
                    crop,
                    radius: slot.radius * scale,
                }
            })
            .collect();
        Scene {
            background: BACKGROUND,
            layers,
        }
    }
}

/// The canvas behind the slots, a near black with a hint of blue.
pub const BACKGROUND: [f32; 3] = [15.0 / 255.0, 17.0 / 255.0, 21.0 / 255.0];

/// The part of a source with aspect `source_aspect` that covers `frame` when scaled, centered,
/// in texture coordinates.
fn cover(frame: Rect, source_aspect: f32) -> Rect {
    let frame_aspect = frame.width() / frame.height();
    if source_aspect > frame_aspect {
        let width = frame_aspect / source_aspect;
        Rect::new((1.0 - width) / 2.0, 0.0, (1.0 + width) / 2.0, 1.0)
    } else {
        let height = source_aspect / frame_aspect;
        Rect::new(0.0, (1.0 - height) / 2.0, 1.0, (1.0 + height) / 2.0)
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: Sources<(u32, u32)> = Sources {
        screen: (3840, 2160),
        camera: (1920, 1080),
    };

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
        let scene = face.scene(Canvas::UHD, SIZES, 1.0);
        assert_eq!(scene.layers.len(), 1);
        assert_eq!(scene.layers[0].source, Source::Camera);
        assert_close(scene.layers[0].frame, FULL);
        assert_close(scene.layers[0].crop, Rect::UNIT);
        assert!(!face.uses(Source::Screen));

        let content = LayoutSpec::of(Layout::Content).unwrap();
        assert_eq!(
            content.scene(Canvas::UHD, SIZES, 1.0).layers[0].source,
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
        let scene = split.scene(half, SIZES, 1.0);
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
        let scene = face_content.scene(Canvas::UHD, SIZES, 1.25);
        assert_close(scene.layers[0].crop, Rect::new(0.1, 0.1, 0.9, 0.9));
        assert_close(scene.layers[1].crop, Rect::UNIT);

        let face = LayoutSpec::of(Layout::Face).unwrap();
        let scene = face.scene(Canvas::UHD, SIZES, 2.0);
        assert_close(scene.layers[0].crop, Rect::new(0.25, 0.25, 0.75, 0.75));
    }

    #[test]
    fn layouts_after_03_are_refused() {
        assert_eq!(
            LayoutSpec::of(Layout::Window),
            Err(UnsupportedLayout(Layout::Window))
        );
    }
}
