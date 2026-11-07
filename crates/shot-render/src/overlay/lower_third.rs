//! The `lower-third` template: a name or a line of text at the bottom left, sliding in.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `text` | the line, usually a name or a channel |
//! | `subtitle` | optional, a smaller line under it |

use shot_core::Micros;

use super::{ParamKind, ParamSpec, Params, TemplateSpec};
use crate::drawing::{Font, Rgba, Shape};
use crate::layout::{Canvas, Rect};

pub const TEMPLATE: &str = "lower-third";

pub const SPEC: TemplateSpec = TemplateSpec {
    name: TEMPLATE,
    about: "a name or line at the bottom left",
    params: &[
        ParamSpec::required("text", ParamKind::Text, "the line"),
        ParamSpec::optional("subtitle", ParamKind::Text, "a smaller line under it"),
    ],
    animated: true,
};

/// The default `enter` and `exit`.
const SLIDE: Micros = Micros(300_000);

pub fn draw(params: Params, t: Micros, canvas: Canvas) -> Vec<Shape> {
    let Some(text) = params.text("text") else {
        return Vec::new();
    };
    let presence = params.presence(SLIDE).at(t);
    if presence <= 0.0 {
        return Vec::new();
    }
    let s = canvas.width as f32 / 3840.0;
    let subtitle = params.text("subtitle");
    let size = 84.0 * s;
    let width = (text.chars().count() as f32 * size * 0.56 + 160.0 * s).max(700.0 * s);
    let height = if subtitle.is_some() { 230.0 } else { 160.0 } * s;
    let left = 120.0 * s - (1.0 - presence) * 120.0 * s;
    let top = canvas.height as f32 * 0.78;
    let fade = |c: Rgba| c.faded(presence);
    let mut shapes = vec![
        Shape::Rect {
            rect: Rect::new(left, top, left + width, top + height),
            radius: 20.0 * s,
            color: fade(Rgba::hex(0x16181D, 0.92)),
        },
        Shape::Rect {
            rect: Rect::new(left, top, left + 12.0 * s, top + height),
            radius: 6.0 * s,
            color: fade(Rgba::hex(0xC8F25A, 1.0)),
        },
        Shape::Text {
            origin: [left + 64.0 * s, top + 34.0 * s],
            text: text.to_owned(),
            size,
            font: Font::Sans,
            color: fade(Rgba::hex(0xF0F3F6, 1.0)),
        },
    ];
    if let Some(subtitle) = subtitle {
        shapes.push(Shape::Text {
            origin: [left + 64.0 * s, top + 148.0 * s],
            text: subtitle.to_owned(),
            size: 48.0 * s,
            font: Font::Sans,
            color: fade(Rgba::hex(0x9AA4B2, 1.0)),
        });
    }
    shapes
}
