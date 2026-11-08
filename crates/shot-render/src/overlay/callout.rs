//! The `callout` template: a label pointing at something on the screen recording, a click or a
//! box of text. It follows the layout and the zoom like a highlight and draws only where the
//! layout shows the screen.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `x`, `y` | the point in pixels of the screen track, or with `width` and `height` the box's top left |
//! | `width`, `height` | optional, a box instead of a point |
//! | `text` | optional, the label |
//!
//! A ring springs open on the point (or an outline around the box), then the label comes in
//! beside it as a sheet on the side towards the middle of the frame. A rule copies `x` and `y`
//! from a click, or `x`, `y`, `width`, `height` and `text` from an OCR box; a callout drawn by
//! hand snaps to the nearest of either with [`snap`].

use shot_core::{Attr, ElementKind, Elements, Micros, Overlay, ParamValue, Point};

use super::card::Card;
use super::{ParamKind, ParamSpec, Params, Stage, TemplateSpec};
use crate::drawing::Shape;
use crate::layout::Rect;
use crate::motion::{Part, Reveal};

pub const TEMPLATE: &str = "callout";

pub const SPEC: TemplateSpec = TemplateSpec {
    name: TEMPLATE,
    about: "a label pointing at a click or a box on the screen",
    params: &[
        ParamSpec::required("x", ParamKind::Integer, "point or left edge, screen pixels"),
        ParamSpec::required("y", ParamKind::Integer, "point or top edge, screen pixels"),
        ParamSpec::optional("width", ParamKind::Integer, "a box instead, screen pixels"),
        ParamSpec::optional("height", ParamKind::Integer, "a box instead, screen pixels"),
        ParamSpec::optional("text", ParamKind::Text, "the label"),
    ],
    animated: true,
};

/// How far from a click a hand-drawn point snaps to it, in screen pixels.
pub const SNAP_DISTANCE: f32 = 160.0;
/// How far in time from the callout's start a click still counts.
pub const SNAP_TIME: Micros = Micros(1_500_000);

/// What a hand-drawn callout snapped to: its parameters, as a rule would copy them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Snap {
    Click(Point),
    Box {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        text: String,
    },
}

impl Snap {
    /// The callout's parameters for the snapped target.
    pub fn params(&self) -> Vec<(&'static str, ParamValue)> {
        match self {
            Self::Click(p) => vec![
                ("x", ParamValue::Integer(p.x.into())),
                ("y", ParamValue::Integer(p.y.into())),
            ],
            Self::Box {
                x,
                y,
                width,
                height,
                text,
            } => vec![
                ("x", ParamValue::Integer(*x)),
                ("y", ParamValue::Integer(*y)),
                ("width", ParamValue::Integer(*width)),
                ("height", ParamValue::Integer(*height)),
                ("text", ParamValue::Text(text.clone())),
            ],
        }
    }
}

/// What a callout drawn at `at` (screen pixels) at take time `t` snaps to: an OCR box on screen
/// at `t` that holds the point, else the nearest click within [`SNAP_DISTANCE`] and
/// [`SNAP_TIME`], else nothing and the point stays where it was drawn.
pub fn snap(elements: &Elements, t: Micros, at: Point) -> Option<Snap> {
    let (px, py) = (at.x as i64, at.y as i64);
    let boxed = elements.of_kind(ElementKind::Ocr).find_map(|e| {
        let range = e.range();
        let (x, y) = (e.integer(Attr::X)?, e.integer(Attr::Y)?);
        let (w, h) = (e.integer(Attr::Width)?, e.integer(Attr::Height)?);
        let holds = (x..=x + w).contains(&px) && (y..=y + h).contains(&py);
        (range.from() <= t && t < range.to() && holds).then(|| Snap::Box {
            x,
            y,
            width: w,
            height: h,
            text: e.text(Attr::Text).unwrap_or_default().to_owned(),
        })
    });
    boxed.or_else(|| {
        elements
            .of_kind(ElementKind::Click)
            .filter(|e| e.t.0.abs_diff(t.0) <= SNAP_TIME.0)
            .filter_map(|e| {
                let p = e.point()?;
                let distance = ((p.x - at.x) as f32).hypot((p.y - at.y) as f32);
                (distance <= SNAP_DISTANCE).then_some((distance, p))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| Snap::Click(p))
    })
}

pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    let p = Params(overlay);
    let (Some(x), Some(y), Some((layer, view))) = (p.int("x"), p.int("y"), stage.screen.as_ref())
    else {
        return;
    };
    let (layer, view) = (*layer, view.clone());
    let theme = stage.theme();
    let (canvas, s, t) = (stage.canvas, stage.scale(), stage.t);
    let motion = p.motion(theme);
    let palette = &theme.palette;
    let target = match (p.int("width"), p.int("height")) {
        (Some(w), Some(h)) => view.map_rect(Rect::new(
            x as f32,
            y as f32,
            (x + w) as f32,
            (y + h) as f32,
        )),
        _ => {
            let [cx, cy] = view.map([x as f32, y as f32]);
            let r = 56.0 * view.scale().max(s);
            Rect::new(cx - r, cy - r, cx + r, cy + r)
        }
    };
    let point = !matches!((p.int("width"), p.int("height")), (Some(_), Some(_)));
    let text = p.text("text").filter(|t| !t.is_empty());
    let parts = 1 + usize::from(text.is_some());
    let pose = motion.pose(overlay.range, Part::new(0, parts), t);
    if pose.visible() {
        let open = match theme.moves.reveal {
            Reveal::Pop => (0.3 + 0.7 * pose.enter).max(0.0),
            _ => 0.6 + 0.4 * pose.enter.clamp(0.0, 1.0),
        } * (1.0 - 0.25 * pose.exit);
        let (cx, cy) = (
            (target.left + target.right) / 2.0,
            (target.top + target.bottom) / 2.0,
        );
        let (hw, hh) = (target.width() / 2.0 * open, target.height() / 2.0 * open);
        let ring = Rect::new(cx - hw, cy - hh, cx + hw, cy + hh);
        let alpha = pose.presence();
        stage.drawing.over.push(Shape::Clip {
            rect: layer.frame,
            radius: layer.radius,
            shapes: vec![
                Shape::Rect {
                    rect: ring,
                    radius: if point { hw } else { 10.0 * s },
                    color: palette.accent.faded(0.14 * alpha),
                },
                Shape::Outline {
                    rect: ring,
                    radius: if point { hw } else { 10.0 * s },
                    width: (8.0 * s).max(2.0),
                    color: palette.accent.faded(alpha),
                },
            ],
        });
    }
    let Some(text) = text else {
        return;
    };
    let size = 64.0 * s;
    let width = text.chars().count() as f32 * size * 0.56 + 96.0 * s;
    let height = size * 1.9;
    // Beside the target, on the side towards the middle of the frame.
    let towards_right = (target.left + target.right) / 2.0 < canvas.width as f32 / 2.0;
    let gap = 36.0 * s;
    let center_x = if towards_right {
        target.right + gap + width / 2.0
    } else {
        target.left - gap - width / 2.0
    };
    let center = [
        center_x.clamp(width / 2.0 + gap, canvas.width as f32 - width / 2.0 - gap),
        ((target.top + target.bottom) / 2.0).clamp(
            height / 2.0 + gap,
            canvas.height as f32 - height / 2.0 - gap,
        ),
    ];
    let side = if towards_right { -1.0 } else { 1.0 };
    let moves = theme.moves;
    Card {
        size: [width, height],
        radius: height / 2.0,
        rest: center,
        from: [side * 140.0 * s, 0.0],
        fill: palette.accent,
        shapes: vec![Shape::Centered {
            center: [width / 2.0, height / 2.0],
            text: text.to_owned(),
            size,
            font: theme.fonts.heading(),
            color: palette.on_accent(),
        }],
        scale: s,
    }
    .draw(
        &motion,
        &moves,
        overlay.range,
        Part::new(1, parts),
        t,
        stage.drawing,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Element, Id, TimeRange};

    #[test]
    fn a_drawn_callout_snaps_to_the_box_it_is_in_or_the_nearest_click() {
        let mut elements = Elements::new(Id::new("t01").unwrap());
        let ocr = Element::over(
            ElementKind::Ocr,
            TimeRange::new(Micros(1_000_000), Micros(5_000_000)).unwrap(),
            "vision",
        )
        .with_text(Attr::Text, "Deploy")
        .with_integer(Attr::X, 100)
        .with_integer(Attr::Y, 200)
        .with_integer(Attr::Width, 300)
        .with_integer(Attr::Height, 40);
        let click = |t, x, y| {
            Element::at(ElementKind::Click, Micros(t), "events")
                .with_integer(Attr::X, x)
                .with_integer(Attr::Y, y)
        };
        elements.push(ocr).unwrap();
        elements.push(click(2_000_000, 900, 900)).unwrap();
        elements.push(click(2_400_000, 1000, 960)).unwrap();
        let at = |t, x, y| snap(&elements, Micros(t), Point { x, y });
        assert!(matches!(
            at(2_000_000, 150, 210),
            Some(Snap::Box { x: 100, .. })
        ));
        assert_eq!(
            at(2_000_000, 980, 950),
            Some(Snap::Click(Point { x: 1000, y: 960 }))
        );
        // Too far from the click, or the box is not on screen yet.
        assert_eq!(at(2_000_000, 1400, 1400), None);
        assert_eq!(at(500_000, 150, 210), None);
    }
}
