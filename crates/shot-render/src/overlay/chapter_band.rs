//! The `chapter-band` template: the chapter's title on a band as the chapter starts. The brand's
//! `on chapter-start` rule places it.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `text` | optional, the title. By default the title of the chapter the overlay opens |
//! | `label` | optional, a small line above it. By default "Chapter" and the chapter's number |
//!
//! The band comes in from the left edge as one sheet (on glass in the launch preset), the label
//! with it and the title in kinetic type a stagger later. Without `text`, the chapter's title
//! enters the key of every chunk the band draws on.

use shot_core::{Id, Micros, Overlay, TimeRange};

use super::card::Card;
use super::{ParamKind, ParamSpec, Params, Stage, TemplateSpec};
use crate::drawing::Shape;
use crate::kinetic::Kinetic;
use crate::layout::Rect;
use crate::motion::Part;
use crate::theme::Theme;

pub const TEMPLATE: &str = "chapter-band";

pub const SPEC: TemplateSpec = TemplateSpec {
    name: TEMPLATE,
    about: "the chapter's title on a band",
    params: &[
        ParamSpec::optional(
            "text",
            ParamKind::Text,
            "the title, the chapter's by default",
        ),
        ParamSpec::optional("label", ParamKind::Text, "a small line above the title"),
    ],
    animated: true,
};

/// The title a band of `take` shows: its `text`, or the title of the chapter it opens.
pub fn title<'a>(overlay: &'a Overlay, theme: &'a Theme, take: Option<&Id>) -> Option<&'a str> {
    Params(overlay)
        .text("text")
        .or_else(|| chapter(overlay, theme, take).map(|(_, title)| title))
}

fn label(overlay: &Overlay, theme: &Theme, take: Option<&Id>) -> Option<String> {
    Params(overlay)
        .text("label")
        .map(str::to_owned)
        .or_else(|| chapter(overlay, theme, take).map(|(number, _)| format!("Chapter {number}")))
}

fn chapter<'a>(overlay: &Overlay, theme: &'a Theme, take: Option<&Id>) -> Option<(usize, &'a str)> {
    theme.chapter(take?, overlay.range.from())
}

pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    let theme = stage.theme();
    let take = stage.look.take.as_ref();
    let Some(title) = title(overlay, theme, take).filter(|t| !t.is_empty()) else {
        return;
    };
    let label = label(overlay, theme, take);
    let (canvas, s, t) = (stage.canvas, stage.scale(), stage.t);
    let motion = Params(overlay).motion(theme);
    let palette = &theme.palette;
    let size = 132.0 * s;
    let chars = title.chars().count() as f32;
    let width = (chars * size * 0.58 + 340.0 * s).clamp(1400.0 * s, canvas.width as f32 * 0.86);
    let height = 340.0 * s;
    let text_range = TimeRange::new(
        Micros(overlay.range.from().0 + motion.stagger.0),
        overlay.range.to(),
    )
    .unwrap_or(overlay.range);
    let mut shapes = vec![Shape::Rect {
        rect: Rect::new(150.0 * s, 0.0, 168.0 * s, height),
        radius: 0.0,
        color: palette.accent,
    }];
    if let Some(label) = label {
        shapes.push(Shape::Text {
            origin: [220.0 * s, 52.0 * s],
            text: label.to_uppercase(),
            size: 46.0 * s,
            font: theme.fonts.body(),
            color: palette.accent,
        });
    }
    shapes.push(Shape::Kinetic(Box::new(Kinetic {
        origin: [220.0 * s, 128.0 * s],
        text: title.to_owned(),
        size,
        font: theme.fonts.heading(),
        color: palette.text,
        width: Some(width - 300.0 * s),
        reveal: theme.moves.reveal,
        unit: theme.moves.unit,
        motion,
        range: text_range,
        t: motion.freeze(text_range, title.split_whitespace().count(), t),
    })));
    let moves = theme.moves;
    // The band reaches past the left edge, so it never shows a corner there.
    let left = -150.0 * s;
    Card {
        size: [width, height],
        radius: 24.0 * s,
        rest: [left + width / 2.0, canvas.height as f32 * 0.62],
        from: [-(width + left), 0.0],
        fill: palette.card.faded(0.94),
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
