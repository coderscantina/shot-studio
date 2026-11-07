//! The punch-in at a moment: which zoom covers it, where it centers, and with `zoom-motion
//! smooth` how it eases in, out and between centers. Pure.
//!
//! Smooth motion happens inside each zoom's own range, so a frame depends only on the zooms of
//! its segment: a zoom eases in over its first [`EASE`] from the zoom that ends where it starts,
//! or from no zoom, and eases out over its last [`EASE`] unless the next zoom starts where it
//! ends and takes over.

use shot_core::{Micros, Zoom, ZoomMotion};

/// How long a smooth zoom takes to move in or out, at most a third of the zoom.
pub const EASE: Micros = Micros(450_000);

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
    let ease = EASE.0.min(zoom.range.duration().0 / 3).max(1) as f32;
    let resting = Punch {
        scale: 1.0,
        ..target
    };
    let previous = index
        .checked_sub(1)
        .map(|i| &zooms[i])
        .filter(|p| p.range.to() == zoom.range.from());
    let from = previous.map_or(resting, |p| Punch::of(p, screen));
    let into = (t.0 - zoom.range.from().0) as f32 / ease;
    let mut punch = from.mix(target, smoothstep(into));
    let hands_over = zooms
        .get(index + 1)
        .is_some_and(|n| n.range.from() == zoom.range.to());
    if !hands_over {
        let left = (zoom.range.to().0 - t.0) as f32 / ease;
        punch = resting.mix(punch, smoothstep(left));
    }
    punch
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
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
    fn smooth_zooms_ease_in_out_and_pan_to_the_next_center() {
        let zooms = [
            zoom(1_000_000, 3_000_000, 800, 600),
            zoom(3_000_000, 5_000_000, 2000, 600),
        ];
        let at = |t| punch_at(&zooms, Micros(t), ZoomMotion::Smooth, SCREEN);
        assert_eq!(at(1_000_000).scale, 1.0);
        assert!((at(1_225_000).scale - 1.5).abs() < 1e-4);
        // The first zoom hands over without zooming out: full scale up to its end.
        assert_eq!(at(2_999_999).scale, 2.0);
        assert_eq!(at(3_000_000).center, [800.0, 600.0]);
        assert_eq!(at(3_225_000).center, [1400.0, 600.0]);
        assert_eq!(at(4_000_000).center, [2000.0, 600.0]);
        // The last one eases out to no zoom.
        assert!((at(4_775_000).scale - 1.5).abs() < 1e-4);
        assert_eq!(at(5_000_000), Punch::none(SCREEN));
    }
}
