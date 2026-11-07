//! Vector shapes and text a frame draws under and over its source layers, in canvas pixels. Pure
//! data: [`crate::frame`] decides what to draw, the vector pass draws it with vello and parley.

use vello::peniko::ImageData;

use crate::layout::Rect;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Sans,
    Mono,
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
        }
    }
}

/// What a frame draws beyond its sources.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    /// Between the background and the first layer.
    pub under: Vec<Shape>,
    /// On top of every layer.
    pub over: Vec<Shape>,
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
