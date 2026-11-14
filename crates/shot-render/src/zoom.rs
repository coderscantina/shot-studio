//! The punch-in at a moment: which zoom covers it, what it frames, and how it ramps in, out
//! and between centers. Pure.
//!
//! A zoom frames its point or the layout's center (`auto`), the screen's center, or a quarter
//! of the screen, which it fills at 200 percent or more. It eases in over its ramp-in and out
//! over its ramp-out, each its own or `zoom-motion`'s: none for `instant`, [`EASE`] for
//! `smooth`, at most a third of the zoom. A ramp of 0 switches hard.
//!
//! Ramps happen inside each zoom's own range, so a frame depends only on the zooms of its
//! segment: a zoom ramps in from the zoom that ends where it starts, or from no zoom, and ramps
//! out over its last ramp-out unless the next zoom starts where it ends and takes over. The
//! spring is critically damped and lands exactly within its ramp, so a zoom never overshoots
//! and hands over without a jump.

use shot_core::{Framing, Micros, Zoom, ZoomMotion};

use crate::motion::Spring;

/// How long a smooth zoom takes to move in or out, at most a third of the zoom.
pub const EASE: Micros = Micros(450_000);

/// The zoom spring before it is fitted to a ramp.
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

    /// Where `zoom` comes to rest on a screen of size `screen`.
    pub fn of(zoom: &Zoom, screen: (u32, u32)) -> Self {
        let framing = zoom.shape.framing;
        let middle = Self::none(screen).center;
        let center = match framing.quarter() {
            Some([x, y]) => [screen.0 as f32 * x, screen.1 as f32 * y],
            None if framing == Framing::Center => middle,
            None => zoom.center.map_or(middle, |c| [c.x as f32, c.y as f32]),
        };
        Self {
            scale: f32::from(zoom.scale.max(framing.min_scale())) / 100.0,
            center,
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

/// How long `zoom` ramps in and out under `motion`: its own ramps, else the motion's, scaled
/// down together when they do not fit into the zoom.
pub fn ramps(zoom: &Zoom, motion: ZoomMotion) -> (Micros, Micros) {
    let length = zoom.range.duration().0;
    let default = match motion {
        ZoomMotion::Instant => 0,
        ZoomMotion::Smooth => EASE.0.min(length / 3),
    };
    let ramp_in = zoom.shape.ramp_in.map_or(default, |r| r.0);
    let ramp_out = zoom.shape.ramp_out.map_or(default, |r| r.0);
    let sum = ramp_in + ramp_out;
    if sum <= length {
        return (Micros(ramp_in), Micros(ramp_out));
    }
    let fitted = (u128::from(ramp_in) * u128::from(length) / u128::from(sum)) as u64;
    (Micros(fitted), Micros(length - fitted))
}

/// Whether any of `zooms` moves gradually rather than switching, so a frame depends on whole
/// zooms and their neighbours.
pub fn eases(zooms: &[Zoom], motion: ZoomMotion) -> bool {
    zooms.iter().any(|z| {
        let (ramp_in, ramp_out) = ramps(z, motion);
        ramp_in.0 + ramp_out.0 > 0
    })
}

/// How far a ramp of `ramp` has come `elapsed` microseconds after it started, 0 to 1.
fn progress(ramp: Micros, elapsed: u64) -> f32 {
    if ramp.0 == 0 {
        return 1.0;
    }
    SPRING.settling_in(ramp).value(elapsed as f32 / 1e6)
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
    let (ramp_in, ramp_out) = ramps(zoom, motion);
    let resting = Punch {
        scale: 1.0,
        ..target
    };
    let previous = index
        .checked_sub(1)
        .map(|i| &zooms[i])
        .filter(|p| p.range.to() == zoom.range.from());
    let from = previous.map_or(resting, |p| Punch::of(p, screen));
    let mut punch = from.mix(target, progress(ramp_in, t.0 - zoom.range.from().0));
    let hands_over = zooms
        .get(index + 1)
        .is_some_and(|n| n.range.from() == zoom.range.to());
    if !hands_over && ramp_out.0 > 0 {
        let out = (t.0 + ramp_out.0).saturating_sub(zoom.range.to().0);
        if out > 0 {
            punch = punch.mix(resting, progress(ramp_out, out));
        }
    }
    punch
}

/// When each of `zooms` (in order, not overlapping) starts, comes to rest and starts to ramp
/// out, as [`punch_at`] moves it, for the sounds on a zoom. A zoom handing over to the next one
/// has no way out; one without ramps rests at its start and leaves at its end.
pub fn phases(
    zooms: &[Zoom],
    motion: ZoomMotion,
) -> impl Iterator<Item = (Micros, Micros, Option<Micros>)> + '_ {
    zooms.iter().enumerate().map(move |(index, zoom)| {
        let (from, to) = (zoom.range.from(), zoom.range.to());
        let hands_over = zooms.get(index + 1).is_some_and(|n| n.range.from() == to);
        let (ramp_in, ramp_out) = ramps(zoom, motion);
        let exit = (!hands_over).then(|| Micros(to.0 - ramp_out.0));
        (from, Micros(from.0 + ramp_in.0), exit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Point, TimeRange, ZoomShape};

    const SCREEN: (u32, u32) = (3840, 2160);

    fn zoom(from: u64, to: u64, x: i32, y: i32) -> Zoom {
        Zoom {
            scale: 200,
            range: TimeRange::new(Micros(from), Micros(to)).unwrap(),
            center: Some(Point { x, y }),
            shape: ZoomShape::default(),
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

    fn shaped(mut zoom: Zoom, framing: Framing, ramps: Option<(u64, u64)>) -> Zoom {
        zoom.shape = ZoomShape {
            framing,
            ramp_in: ramps.map(|r| Micros(r.0)),
            ramp_out: ramps.map(|r| Micros(r.1)),
        };
        zoom
    }

    #[test]
    fn framing_picks_the_point_the_center_or_a_quarter_at_200_percent() {
        let at = |framing, scale| {
            let mut z = shaped(zoom(0, 1_000_000, 800, 600), framing, None);
            z.scale = scale;
            punch_at(&[z], Micros(500_000), ZoomMotion::Instant, SCREEN)
        };
        assert_eq!(at(Framing::Auto, 150).center, [800.0, 600.0]);
        assert_eq!(at(Framing::Center, 150).center, [1920.0, 1080.0]);
        let quarter = at(Framing::BottomRight, 150);
        assert_eq!(quarter.center, [2880.0, 1620.0]);
        assert_eq!(quarter.scale, 2.0, "a quarter fills the frame");
        assert_eq!(at(Framing::TopLeft, 300).scale, 3.0, "a higher scale stays");
    }

    #[test]
    fn ramps_override_the_motion_and_fit_into_the_zoom() {
        let ramped = shaped(
            zoom(1_000_000, 3_000_000, 800, 600),
            Framing::Auto,
            Some((600_000, 0)),
        );
        let zooms = [ramped];
        let at = |t| punch_at(&zooms, Micros(t), ZoomMotion::Instant, SCREEN).scale;
        // Eases in over its 600 ms even with instant motion, and switches out hard.
        assert!(at(1_300_000) > 1.0 && at(1_300_000) < 2.0);
        assert_eq!(at(1_610_000), 2.0);
        assert_eq!(at(2_999_999), 2.0);
        assert_eq!(at(3_000_000), 1.0);
        // Ramps longer than the zoom share it in proportion.
        let long = shaped(
            zoom(0, 1_000_000, 0, 0),
            Framing::Auto,
            Some((1_500_000, 500_000)),
        );
        assert_eq!(
            ramps(&long, ZoomMotion::Smooth),
            (Micros(750_000), Micros(250_000))
        );
        // No ramps of its own: the motion's, as before.
        let plain = zoom(0, 3_000_000, 0, 0);
        assert_eq!(ramps(&plain, ZoomMotion::Instant), (Micros(0), Micros(0)));
        assert_eq!(ramps(&plain, ZoomMotion::Smooth), (EASE, EASE));
    }

    #[test]
    fn neighbouring_zooms_with_their_own_ramps_never_jump() {
        let zooms = [
            shaped(
                zoom(0, 2_000_000, 800, 600),
                Framing::Auto,
                Some((300_000, 900_000)),
            ),
            shaped(
                zoom(2_000_000, 4_000_000, 0, 0),
                Framing::TopRight,
                Some((700_000, 200_000)),
            ),
            shaped(
                zoom(4_100_000, 6_000_000, 0, 0),
                Framing::Center,
                Some((1_000_000, 1_000_000)),
            ),
        ];
        // Across every edge of a zoom the picture moves no more than within a microsecond.
        for edge in [0, 2_000_000, 4_000_000, 4_100_000, 6_000_000] {
            let at = |t: u64| punch_at(&zooms, Micros(t), ZoomMotion::Instant, SCREEN);
            let (before, after) = (at(edge.max(1) - 1), at(edge));
            assert!(
                (before.scale - after.scale).abs() < 1e-3,
                "scale jumps at {edge}"
            );
            if before.scale > 1.001 {
                assert_eq!(before.center, after.center, "center jumps at {edge}");
            }
        }
    }
}
