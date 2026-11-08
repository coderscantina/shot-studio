//! Vector shapes and text a frame draws under and over its source layers, in canvas pixels, and
//! the materials between them: soft shadows and sheets that tilt and sit on frosted glass. Pure
//! data: [`crate::frame`] decides what to draw, the vector and material passes draw it.

use std::sync::Arc;

use shot_core::Micros;
use vello::peniko::ImageData;

use crate::kinetic::Kinetic;
use crate::layout::Rect;
use crate::lottie::Filled;
use crate::typeface::Face;

/// Straight-alpha sRGB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba(pub [f32; 4]);

impl Rgba {
    pub const fn hex(rgb: u32, alpha: f32) -> Self {
        let [_, r, g, b] = rgb.to_be_bytes();
        Self([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, alpha])
    }

    pub fn faded(self, factor: f32) -> Self {
        let [r, g, b, a] = self.0;
        Self([r, g, b, a * factor])
    }
}

/// The system fonts, or a font file by content (see [`crate::typeface`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Sans,
    Mono,
    Face(Face),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect {
        rect: Rect,
        radius: f32,
        color: Rgba,
    },
    /// A blurred rounded rectangle.
    Shadow {
        rect: Rect,
        radius: f32,
        blur: f32,
        color: Rgba,
    },
    Ring {
        center: [f32; 2],
        radius: f32,
        width: f32,
        color: Rgba,
    },
    /// The arrow pointer with its tip at `tip`, `height` tall.
    Pointer { tip: [f32; 2], height: f32 },
    /// One line of text with its top left at `origin`.
    Text {
        origin: [f32; 2],
        text: String,
        size: f32,
        font: Font,
        color: Rgba,
    },
    /// One line of text centered on `center`.
    Centered {
        center: [f32; 2],
        text: String,
        size: f32,
        font: Font,
        color: Rgba,
    },
    /// One line of text centered on `center` on a rounded box that fits it.
    Label {
        center: [f32; 2],
        text: String,
        size: f32,
        color: Rgba,
        background: Rgba,
    },
    /// `shapes` clipped to a rounded rectangle.
    Clip {
        rect: Rect,
        radius: f32,
        shapes: Vec<Shape>,
    },
    /// The outline of a rounded rectangle, `width` wide, centered on its edge.
    Outline {
        rect: Rect,
        radius: f32,
        width: f32,
        color: Rgba,
    },
    /// The `crop` of an image (fractions of it) scaled into `rect`, with rounded corners.
    Image {
        rect: Rect,
        radius: f32,
        image: ImageData,
        crop: Rect,
        opacity: f32,
    },
    /// Caption words wrapped into lines at most `width` wide, centered on `center`, on a
    /// rounded box. The word at `highlight` draws on an accent pill in the background color.
    Caption {
        center: [f32; 2],
        width: f32,
        words: Vec<String>,
        highlight: Option<usize>,
        size: f32,
        color: Rgba,
        accent: Rgba,
        background: Rgba,
        /// The highlighted word's scale, 1 once it has settled.
        pop: f32,
    },
    /// Text whose words or glyphs move on their own springs.
    Kinetic(Box<Kinetic>),
    /// A filled Lottie template `elapsed` after it started, its top left at `origin`, `scale`
    /// canvas pixels per Lottie pixel. Its text slots draw with parley.
    Lottie {
        lottie: Arc<Filled>,
        origin: [f32; 2],
        scale: f32,
        elapsed: Micros,
        opacity: f32,
        /// What its text slots draw in.
        font: Font,
    },
}

impl Shape {
    /// A rectangle that holds everything the shape draws.
    pub fn bounds(&self) -> Rect {
        match self {
            Self::Rect { rect, .. } => *rect,
            Self::Shadow { rect, blur, .. } => grow(*rect, blur * 3.0),
            Self::Ring {
                center,
                radius,
                width,
                ..
            } => around(*center, radius + width),
            Self::Pointer { tip, height } => Rect::new(
                tip[0] - height * 0.1,
                tip[1] - height * 0.1,
                tip[0] + height * 0.7,
                tip[1] + height * 1.1,
            ),
            Self::Text {
                origin, text, size, ..
            } => Rect::new(
                origin[0],
                origin[1],
                origin[0] + text.chars().count() as f32 * size,
                origin[1] + size * 1.5,
            ),
            Self::Label {
                center, text, size, ..
            }
            | Self::Centered {
                center, text, size, ..
            } => {
                let (w, h) = label_box(text, *size);
                Rect::new(center[0] - w, center[1] - h, center[0] + w, center[1] + h)
            }
            Self::Outline { rect, width, .. } => grow(*rect, *width),
            Self::Image { rect, .. } => *rect,
            Self::Caption {
                center,
                width,
                size,
                ..
            } => Rect::new(
                center[0] - width / 2.0 - size,
                center[1] - size * 3.0,
                center[0] + width / 2.0 + size,
                center[1] + size * 3.0,
            ),
            Self::Clip { rect, shapes, .. } => shapes
                .iter()
                .map(Shape::bounds)
                .reduce(union)
                .and_then(|inner| intersect(inner, *rect))
                .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0)),
            Self::Kinetic(text) => text.bounds(),
            Self::Lottie {
                lottie,
                origin,
                scale,
                ..
            } => {
                let (width, height) = lottie.size();
                Rect::new(
                    origin[0],
                    origin[1],
                    origin[0] + width as f32 * scale,
                    origin[1] + height as f32 * scale,
                )
            }
        }
    }
}

/// The blurred silhouette of shapes, in one color: a soft shadow of any shape.
#[derive(Debug, Clone, PartialEq)]
pub struct SoftShadow {
    /// Only their coverage counts.
    pub shapes: Vec<Shape>,
    /// Standard deviation of the blur, in canvas pixels.
    pub blur: f32,
    pub color: Rgba,
    /// From the shapes to their shadow, see [`crate::materials::Light::shadow_offset`].
    pub offset: [f32; 2],
}

impl SoftShadow {
    /// A rectangle that holds the whole shadow.
    pub fn bounds(&self) -> Option<Rect> {
        let inner = self.shapes.iter().map(Shape::bounds).reduce(union)?;
        let r = grow(inner, self.blur * 3.0);
        Some(Rect::new(
            r.left + self.offset[0],
            r.top + self.offset[1],
            r.right + self.offset[0],
            r.bottom + self.offset[1],
        ))
    }
}

/// Frosted glass: what lies behind, blurred and tinted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glass {
    /// Standard deviation of the blur, in canvas pixels.
    pub blur: f32,
    /// Mixed over the blurred backdrop by its alpha.
    pub tint: Rgba,
}

/// Where a sheet sits at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Of the sheet, in canvas pixels.
    pub center: [f32; 2],
    pub scale: f32,
    /// Rotation in degrees about the horizontal axis (top away from the viewer when positive)
    /// and the vertical axis (right side away), seen in perspective.
    pub tilt: [f32; 2],
    pub opacity: f32,
}

/// Shapes drawn flat in their own coordinates, then placed on the canvas as one plane that can
/// scale, tilt in perspective, catch the light and sit on frosted glass. Anything that moves as
/// one piece is a sheet: it is drawn once and motion blurred along its path by the material
/// pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    /// In canvas pixels at scale 1. Shapes draw with the origin at its top left.
    pub size: [f32; 2],
    /// Corner radius of the glass and the light's sheen.
    pub radius: f32,
    pub glass: Option<Glass>,
    pub shapes: Vec<Shape>,
    /// At the start and the end of the frame's shutter, equal when it holds still.
    pub placement: [Placement; 2],
}

/// What a frame draws beyond its sources, from the bottom up: `under`, the layers, `shadows`,
/// `sheets` in order, then `over`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    /// Between the background and the first layer.
    pub under: Vec<Shape>,
    /// On top of every layer.
    pub over: Vec<Shape>,
    pub shadows: Vec<SoftShadow>,
    /// Glass blurs the layers and shadows behind it, not other sheets.
    pub sheets: Vec<Sheet>,
}

/// A generous half width and half height for a label's box, so bounds never cut it.
fn label_box(text: &str, size: f32) -> (f32, f32) {
    (text.chars().count() as f32 * size * 0.5 + size, size)
}

fn grow(rect: Rect, by: f32) -> Rect {
    Rect::new(
        rect.left - by,
        rect.top - by,
        rect.right + by,
        rect.bottom + by,
    )
}

fn around(center: [f32; 2], radius: f32) -> Rect {
    Rect::new(
        center[0] - radius,
        center[1] - radius,
        center[0] + radius,
        center[1] + radius,
    )
}

pub fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.left.min(b.left),
        a.top.min(b.top),
        a.right.max(b.right),
        a.bottom.max(b.bottom),
    )
}

pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let r = Rect::new(
        a.left.max(b.left),
        a.top.max(b.top),
        a.right.min(b.right),
        a.bottom.min(b.bottom),
    );
    (r.left < r.right && r.top < r.bottom).then_some(r)
}
