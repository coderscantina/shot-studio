//! A card that moves as one piece, shared by the templates that slide or pop a panel in: a
//! [`Sheet`] placed at both ends of the shutter, so it motion blurs, on frosted glass where the
//! preset uses it, tilting on its way in, with a soft shadow that follows it away from the light.

use shot_core::{Micros, TimeRange};

use crate::drawing::{Drawing, Glass, Placement, Rgba, Shape, Sheet, SoftShadow};
use crate::layout::Rect;
use crate::materials::{self, LIGHT};
use crate::motion::{Motion, Moves, Part, Pose, Reveal};

/// How far in front of the canvas cards float, in 4K pixels: sets the shadow's offset.
const ELEVATION: f32 = 60.0;

pub(crate) struct Card {
    /// In canvas pixels at scale 1.
    pub size: [f32; 2],
    pub radius: f32,
    /// Where its center rests.
    pub rest: [f32; 2],
    /// Where it comes from: the offset of its center before it starts, at full density.
    pub from: [f32; 2],
    /// The panel color. On glass it tints the blurred backdrop instead.
    pub fill: Rgba,
    /// Drawn on the panel, with the card's top left at the origin.
    pub shapes: Vec<Shape>,
    /// Canvas pixels per 4K pixel.
    pub scale: f32,
}

impl Card {
    /// Draws the card of an element on screen over `range` as `part` of it, at `t`.
    pub fn draw(
        self,
        motion: &Motion,
        moves: &Moves,
        range: TimeRange,
        part: Part,
        t: Micros,
        drawing: &mut Drawing,
    ) {
        let place =
            |t: Micros| self.placement(motion, moves, range, motion.pose(range, part, t), t);
        let placement = materials::shutter(t).map(place);
        if placement.iter().all(|p| p.opacity <= 0.0) {
            return;
        }
        let s = self.scale;
        let now = place(t);
        let half = [
            self.size[0] / 2.0 * now.scale,
            self.size[1] / 2.0 * now.scale,
        ];
        let [dx, dy] = LIGHT.shadow_offset(ELEVATION * s);
        drawing.shadows.push(SoftShadow {
            shapes: vec![Shape::Rect {
                rect: Rect::new(
                    now.center[0] - half[0],
                    now.center[1] - half[1],
                    now.center[0] + half[0],
                    now.center[1] + half[1],
                ),
                radius: self.radius * now.scale,
                color: Rgba::hex(0x000000, 1.0),
            }],
            blur: 36.0 * s,
            color: Rgba::hex(0x000000, 0.5 * now.opacity),
            offset: [dx, dy],
        });
        let panel = (!moves.glass).then(|| Shape::Rect {
            rect: Rect::new(0.0, 0.0, self.size[0], self.size[1]),
            radius: self.radius,
            color: self.fill,
        });
        let [r, g, b, _] = self.fill.0;
        drawing.sheets.push(Sheet {
            size: self.size,
            radius: self.radius,
            glass: moves.glass.then_some(Glass {
                blur: 36.0 * s,
                tint: Rgba([r, g, b, 0.55]),
            }),
            shapes: panel.into_iter().chain(self.shapes).collect(),
            placement,
        });
    }

    /// Where the card is at a pose: it travels along `from`, pops by scale in the pop preset,
    /// tilts on its way where the preset tilts, and drifts by parallax on glass.
    fn placement(
        &self,
        motion: &Motion,
        moves: &Moves,
        range: TimeRange,
        pose: Pose,
        t: Micros,
    ) -> Placement {
        let away = pose.offset();
        let travel = motion.travel.min(1.0);
        let (reach, scale, opacity) = match moves.reveal {
            Reveal::Pop => (
                0.2,
                (0.6 + 0.4 * pose.enter) * (1.0 - 0.3 * pose.exit),
                (pose.enter * 3.0).clamp(0.0, 1.0) * (1.0 - pose.exit),
            ),
            Reveal::Fade => (0.25, 1.0, pose.presence()),
            Reveal::Rise | Reveal::Mask => (1.0, 1.0, pose.presence()),
        };
        let shift = away * reach * travel;
        let lean = moves.tilt * away;
        let side = self.from[0].signum();
        let mut placement = Placement {
            center: [
                self.rest[0] + self.from[0] * shift,
                self.rest[1] + self.from[1] * shift,
            ],
            scale: scale.max(0.0),
            tilt: [lean * 0.4, lean * side],
            opacity,
        };
        if moves.glass {
            let middle = Micros(range.from().0 + range.duration().0 / 2);
            let camera = materials::drift(t, middle, 14.0 * self.scale, [1.0, -0.3]);
            placement = materials::parallax(placement, 0.8, camera);
        }
        placement
    }
}
