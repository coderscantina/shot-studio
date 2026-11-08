//! The `end-card` template: the video's last seconds as a YouTube end screen. It draws the
//! brand's background, a headline, and frames where the end screen's elements go, so the
//! elements YouTube Studio places on top of them line up with the design.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `text` | optional, the headline, "Thanks for watching" by default |
//! | `subtitle` | optional, a smaller line under it |
//! | `videos` | optional, how many video elements to leave room for, 0 to 2, 2 by default |
//! | `subscribe` | optional, whether to leave room for the subscribe button, true by default |
//!
//! [`EndScreen::of`] gives the element areas: everything stays inside the inner 90% of the
//! frame, YouTube's safe area for end screen elements, two 16:9 video areas stack on the right
//! and the round subscribe button sits under the headline at the left.

use shot_core::{Micros, Overlay, TimeRange};

use super::{ParamKind, ParamSpec, Params, Stage, TemplateSpec};
use crate::drawing::{Rgba, Shape};
use crate::kinetic::Kinetic;
use crate::layout::{Canvas, Rect};
use crate::motion::{Part, Reveal};

pub const TEMPLATE: &str = "end-card";

pub const SPEC: TemplateSpec = TemplateSpec {
    name: TEMPLATE,
    about: "the end screen, with room for YouTube's elements",
    params: &[
        ParamSpec::optional("text", ParamKind::Text, "the headline"),
        ParamSpec::optional("subtitle", ParamKind::Text, "a smaller line under it"),
        ParamSpec::optional("videos", ParamKind::Integer, "video elements, 0 to 2"),
        ParamSpec::optional(
            "subscribe",
            ParamKind::Bool,
            "room for the subscribe button",
        ),
    ],
    animated: true,
};

/// The default headline.
pub const HEADLINE: &str = "Thanks for watching";

/// Where YouTube's end screen elements go on a canvas, in its pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EndScreen {
    /// The safe area: the inner 90% of the frame.
    pub safe: Rect,
    /// Two 16:9 video elements, top first.
    pub videos: [Rect; 2],
    /// The subscribe button, a circle in this square.
    pub subscribe: Rect,
    /// The headline's top left.
    pub headline: [f32; 2],
}

impl EndScreen {
    pub fn of(canvas: Canvas) -> Self {
        let (w, h) = (canvas.width as f32, canvas.height as f32);
        let safe = Rect::new(w * 0.05, h * 0.05, w * 0.95, h * 0.95);
        let video = [w * 0.36, w * 0.36 * 9.0 / 16.0];
        let gap = h * 0.05;
        let top = (h - 2.0 * video[1] - gap) / 2.0;
        let right = safe.right - w * 0.02;
        let at = |top: f32| Rect::new(right - video[0], top, right, top + video[1]);
        let button = h * 0.22;
        let headline = [safe.left + w * 0.03, h * 0.3];
        Self {
            safe,
            videos: [at(top), at(top + video[1] + gap)],
            subscribe: Rect::new(
                headline[0],
                h * 0.56,
                headline[0] + button,
                h * 0.56 + button,
            ),
            headline,
        }
    }
}

pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    let params = Params(overlay);
    let theme = stage.theme();
    let (canvas, s, t) = (stage.canvas, stage.scale(), stage.t);
    let motion = params.motion(theme);
    let palette = &theme.palette;
    let screen = EndScreen::of(canvas);
    let range = overlay.range;
    let videos = params.int("videos").unwrap_or(2).clamp(0, 2) as usize;
    let subscribe = params.bool("subscribe").unwrap_or(true);
    let parts = 2 + videos + usize::from(subscribe);

    let backdrop = motion.pose(range, Part::new(0, parts), t).presence();
    stage.drawing.over.push(Shape::Rect {
        rect: Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32),
        radius: 0.0,
        color: palette.background.faded(backdrop),
    });
    let text = params.text("text").unwrap_or(HEADLINE);
    let text_range =
        TimeRange::new(Micros(range.from().0 + motion.stagger.0), range.to()).unwrap_or(range);
    let size = 150.0 * s;
    stage.drawing.over.push(Shape::Kinetic(Box::new(Kinetic {
        origin: screen.headline,
        text: text.to_owned(),
        size,
        font: theme.fonts.heading(),
        color: palette.text,
        width: Some(screen.videos[0].left - screen.headline[0] - 120.0 * s),
        reveal: theme.moves.reveal,
        unit: theme.moves.unit,
        motion,
        range: text_range,
        t: motion.freeze(text_range, text.split_whitespace().count(), t),
    })));
    if let Some(subtitle) = params.text("subtitle") {
        let pose = motion.pose(range, Part::new(1, parts), t);
        stage.drawing.over.push(Shape::Text {
            origin: [screen.headline[0], screen.headline[1] + size * 2.75],
            text: subtitle.to_owned(),
            size: 60.0 * s,
            font: theme.fonts.body(),
            color: palette.muted().faded(pose.presence()),
        });
    }
    // The element frames rise into place over the backdrop, one after another.
    let mut slots: Vec<(Rect, f32)> = Vec::new();
    if subscribe {
        slots.push((screen.subscribe, screen.subscribe.width() / 2.0));
    }
    slots.extend(screen.videos[..videos].iter().map(|v| (*v, 28.0 * s)));
    for (index, (rect, radius)) in slots.into_iter().enumerate() {
        let pose = motion.pose(range, Part::new(2 + index, parts), t);
        if !pose.visible() {
            continue;
        }
        let alpha = pose.presence();
        let rise = pose.offset() * 160.0 * s * motion.travel.min(1.0);
        let grow = match theme.moves.reveal {
            Reveal::Pop => (0.6 + 0.4 * pose.enter) * (1.0 - 0.3 * pose.exit),
            _ => 1.0,
        };
        let (cx, cy) = (
            (rect.left + rect.right) / 2.0,
            (rect.top + rect.bottom) / 2.0 + rise,
        );
        let (hw, hh) = (rect.width() / 2.0 * grow, rect.height() / 2.0 * grow);
        let at = Rect::new(cx - hw, cy - hh, cx + hw, cy + hh);
        let radius = radius * grow;
        stage.drawing.over.extend([
            Shape::Shadow {
                rect: at,
                radius,
                blur: 48.0 * s,
                color: Rgba::hex(0x000000, 0.45 * alpha),
            },
            Shape::Rect {
                rect: at,
                radius,
                color: palette.card.faded(alpha),
            },
            Shape::Outline {
                rect: at,
                radius,
                width: 6.0 * s,
                color: palette.text.faded(0.14 * alpha),
            },
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_element_areas_stay_inside_the_safe_area_and_apart() {
        for canvas in [
            Canvas::UHD,
            Canvas {
                width: 1920,
                height: 1080,
            },
        ] {
            let screen = EndScreen::of(canvas);
            let inside = |r: Rect| {
                r.left >= screen.safe.left
                    && r.top >= screen.safe.top
                    && r.right <= screen.safe.right
                    && r.bottom <= screen.safe.bottom
            };
            let [top, bottom] = screen.videos;
            assert!(inside(top) && inside(bottom) && inside(screen.subscribe));
            assert!(top.bottom < bottom.top);
            assert!((top.width() / top.height() - 16.0 / 9.0).abs() < 1e-3);
            assert!(screen.subscribe.right < top.left);
        }
    }
}
