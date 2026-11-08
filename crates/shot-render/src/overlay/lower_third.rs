//! The `lower-third` template: a name or a line of text at the bottom left, on a card that
//! comes in from the left with its line in kinetic type.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `text` | the line, usually a name or a channel |
//! | `subtitle` | optional, a smaller line under it |

use shot_core::{Micros, Overlay, TimeRange};

use super::card::Card;
use super::{ParamKind, ParamSpec, Params, Stage, TemplateSpec};
use crate::drawing::Shape;
use crate::kinetic::Kinetic;
use crate::layout::Rect;
use crate::motion::Part;

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

pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    let params = Params(overlay);
    let Some(text) = params.text("text") else {
        return;
    };
    let theme = stage.theme();
    let (canvas, s, t) = (stage.canvas, stage.scale(), stage.t);
    let motion = params.motion(theme);
    let palette = &theme.palette;
    let subtitle = params.text("subtitle");
    let size = 84.0 * s;
    let longest = text
        .chars()
        .count()
        .max(subtitle.map_or(0, |s| s.chars().count() * 4 / 7));
    let width = (longest as f32 * size * 0.56 + 160.0 * s).max(700.0 * s);
    let height = if subtitle.is_some() { 230.0 } else { 160.0 } * s;
    let left = 120.0 * s;
    let top = canvas.height as f32 * 0.78;
    let text_range = TimeRange::new(
        Micros(overlay.range.from().0 + motion.stagger.0),
        overlay.range.to(),
    )
    .unwrap_or(overlay.range);
    let words = text.split_whitespace().count();
    let mut shapes = vec![
        Shape::Clip {
            rect: Rect::new(0.0, 0.0, width, height),
            radius: 20.0 * s,
            shapes: vec![Shape::Rect {
                rect: Rect::new(0.0, 0.0, 12.0 * s, height),
                radius: 0.0,
                color: palette.accent,
            }],
        },
        Shape::Kinetic(Box::new(Kinetic {
            origin: [64.0 * s, 34.0 * s],
            text: text.to_owned(),
            size,
            font: theme.fonts.heading(),
            color: palette.text,
            width: None,
            reveal: theme.moves.reveal,
            unit: theme.moves.unit,
            motion,
            range: text_range,
            t: motion.freeze(text_range, words, t),
        })),
    ];
    if let Some(subtitle) = subtitle {
        shapes.push(Shape::Text {
            origin: [64.0 * s, 148.0 * s],
            text: subtitle.to_owned(),
            size: 48.0 * s,
            font: theme.fonts.body(),
            color: palette.muted(),
        });
    }
    let moves = theme.moves;
    Card {
        size: [width, height],
        radius: 20.0 * s,
        rest: [left + width / 2.0, top + height / 2.0],
        from: [-(width * 0.5 + 160.0 * s), 0.0],
        fill: palette.card.faded(0.92),
        shapes,
        scale: s,
    }
    .draw(
        &motion,
        &moves,
        overlay.range,
        Part::WHOLE,
        t,
        stage.drawing,
    );
}
