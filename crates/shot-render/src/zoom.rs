//! The punch-in at a moment: which zoom covers it, where it centers, and with `zoom-motion
//! smooth` how it springs in, out and between centers. Pure.
//!
//! Smooth motion happens inside each zoom's own range, so a frame depends only on the zooms of
//! its segment: a zoom springs in from the zoom that ends where it starts, or from no zoom, and
//! springs out over its last [`EASE`] unless the next zoom starts where it ends and takes over.
//! The spring is critically damped, so a zoom never overshoots, and settles within [`EASE`], at
//! most a third of the zoom.

use shot_core::{Micros, Zoom, ZoomMotion};

use crate::motion::Spring;

/// How long a smooth zoom takes to move in or out, at most a third of the zoom.
pub const EASE: Micros = Micros(450_000);

/// The zoom spring before it is fitted to [`EASE`] or a short zoom.
const SPRING: Spring = Spring::new(0.3, 1.0);

/// A punch-in: the scale and the point of the screen track it centers on, in its pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Punch {
    pub scale: f32,
    pub center: [f32; 2],
}

impl Punch {
    /// No zoom, centered on the screen of size `screen`.
    pub fn none(screen: (u32, u32)) -> Self {
        Self {
            scale: 1.0,
            center: [screen.0 as f32 / 2.0, screen.1 as f32 / 2.0],
        }
    }

    fn of(zoom: &Zoom, screen: (u32, u32)) -> Self {
        Self {
            scale: f32::from(zoom.scale) / 100.0,
            center: zoom
                .center
                .map_or(Self::none(screen).center, |c| [c.x as f32, c.y as f32]),
        }
    }

    fn mix(self, other: Self, amount: f32) -> Self {
        let lerp = |a: f32, b: f32| a + (b - a) * amount;
        Self {
            scale: lerp(self.scale, other.scale),
            center: [
                lerp(self.center[0], other.center[0]),
                lerp(self.center[1], other.center[1]),
            ],
        }
    }
}

/// The punch-in of `zooms`, in order and not overlapping, at media time `t` on a screen track
/// of size `screen`.
pub fn punch_at(zooms: &[Zoom], t: Micros, motion: ZoomMotion, screen: (u32, u32)) -> Punch {
    let Some(index) = zooms
        .iter()
        .position(|z| z.range.from() <= t && t < z.range.to())
    else {
        return Punch::none(screen);
    };
    let zoom = &zooms[index];
    let target = Punch::of(zoom, screen);
    if motion == ZoomMotion::Instant {
        return target;
    }
    let ease = Micros(EASE.0.min(zoom.range.duration().0 / 3).max(1));
    let spring = SPRING.settling_in(ease);
    let seconds = |micros: u64| micros as f32 / 1e6;
    let resting = Punch {
        scale: 1.0,
        ..target
    };
    let previous = index
        .checked_sub(1)
        .map(|i| &zooms[i])
        .filter(|p| p.range.to() == zoom.range.from());
    let from = previous.map_or(resting, |p| Punch::of(p, screen));
    let mut punch = from.mix(target, spring.value(seconds(t.0 - zoom.range.from().0)));
    let hands_over = zooms
        .get(index + 1)
        .is_some_and(|n| n.range.from() == zoom.range.to());
    if !hands_over {
        let out = (t.0 + ease.0).saturating_sub(zoom.range.to().0);
        punch = punch.mix(resting, spring.value(seconds(out)));
    }
    punch
}

/// When each of `zooms` (in order, not overlapping) starts, comes to rest and starts to spring
/// out, as [`punch_at`] moves it, for the sounds on a zoom. A zoom handing over to the next one
/// has no way out; an instant zoom rests at its start and leaves at its end.
pub fn phases(
    zooms: &[Zoom],
    motion: ZoomMotion,
) -> impl Iterator<Item = (Micros, Micros, Option<Micros>)> + '_ {
    zooms.iter().enumerate().map(move |(index, zoom)| {
        let (from, to) = (zoom.range.from(), zoom.range.to());
        let hands_over = zooms.get(index + 1).is_some_and(|n| n.range.from() == to);
        let ease = match motion {
            ZoomMotion::Instant => Micros(0),
            ZoomMotion::Smooth => Micros(EASE.0.min(zoom.range.duration().0 / 3).max(1)),
        };
        let exit = (!hands_over).then(|| Micros(to.0 - ease.0));
        (from, Micros(from.0 + ease.0), exit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Point, TimeRange};

    const SCREEN: (u32, u32) = (3840, 2160);

    fn zoom(from: u64, to: u64, x: i32, y: i32) -> Zoom {
        Zoom {
            scale: 200,
            range: TimeRange::new(Micros(from), Micros(to)).unwrap(),
            center: Some(Point { x, y }),
            by: None,
        }
    }

    #[test]
    fn instant_zooms_jump_to_their_center() {
        let zooms = [zoom(1_000_000, 3_000_000, 800, 600)];
        let at = |t| punch_at(&zooms, Micros(t), ZoomMotion::Instant, SCREEN);
        assert_eq!(at(999_999), Punch::none(SCREEN));
        assert_eq!(
            at(1_000_000),
            Punch {
                scale: 2.0,
                center: [800.0, 600.0]
            }
        );
    }

    #[test]
    fn smooth_zooms_spring_in_out_and_pan_to_the_next_center() {
        let zooms = [
            zoom(1_000_000, 3_000_000, 800, 600),
            zoom(3_000_000, 5_000_000, 2000, 600),
        ];
        let at = |t| punch_at(&zooms, Micros(t), ZoomMotion::Smooth, SCREEN);
        assert_eq!(at(1_000_000).scale, 1.0);
        let scales: Vec<f32> = (0..45).map(|i| at(1_000_000 + i * 10_000).scale).collect();
        assert!(scales.windows(2).all(|w| w[0] <= w[1]), "no overshoot");
        // A spring starts slowly and is past halfway well before the end of its move.
        assert!(scales[1] < 1.05 && scales[20] > 1.5);
        assert_eq!(at(1_451_000).scale, 2.0);
        // The first zoom hands over without zooming out: full scale up to its end.
        assert_eq!(at(2_999_999).scale, 2.0);
        assert_eq!(at(3_000_000).center, [800.0, 600.0]);
        assert!(at(3_100_000).center[0] > 800.0 && at(3_100_000).center[0] < 2000.0);
        assert_eq!(at(4_000_000).center, [2000.0, 600.0]);
        // The last one springs out to no zoom.
        assert_eq!(at(4_550_000).scale, 2.0);
        assert!(at(4_999_999).scale < 1.001);
        assert_eq!(at(5_000_000), Punch::none(SCREEN));
    }
}
