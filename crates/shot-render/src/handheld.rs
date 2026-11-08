//! A hand-held camera: the camera layer drifts as if a person held it, wherever a layout shows
//! the camera. Pure, and deterministic from the take and its media time, so preview and export
//! draw the same and a chunk renders the same twice.
//!
//! The drift is layered smooth noise: a slow wander, a slower sway, a faint tremor, and now and
//! then a small settle, as when a hand finds a new grip. Rotation follows its own slower noise.
//! Everything stays below about half a hertz except the tremor, so it never feels seasick. The
//! camera is overscaled just enough that no edge shows at the strongest drift, so a full-frame
//! `face` stays filled; the overscale depends on the strength alone and does not breathe.

use shot_core::{Id, Micros};

use crate::layout::{Rect, Scene, Source, Sources};

/// At strength 100, the farthest drift as a share of the shorter side of the shown camera.
const SHIFT: f32 = 0.016;
/// At strength 100, the largest rotation in radians, about 0.6 degrees.
const TURN: f32 = 0.0105;

/// Noise layers of the drift: frequency in hertz and weight.
const DRIFT: [(f32, f32); 3] = [(0.13, 1.0), (0.37, 0.45), (1.1, 0.06)];
/// Noise layers of the rotation, slower than the drift.
const ROLL: [(f32, f32); 2] = [(0.09, 1.0), (0.23, 0.35)];
/// A settle may happen once in each cell of this many seconds.
const SETTLE_EVERY: f32 = 9.0;
/// How long a settle takes, in seconds.
const SETTLE_FOR: f32 = 0.7;
/// The share of the drift a settle moves.
const SETTLE_WEIGHT: f32 = 0.3;

/// How the camera sits at one moment: its shift as a share of the shorter side of what it shows,
/// and its rotation in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sway {
    pub shift: [f32; 2],
    pub turn: f32,
}

/// The sway at media time `t` of a take seeded by `seed`, at `strength` 0 to 100.
pub fn sway(strength: u8, seed: u64, t: Micros) -> Sway {
    let amount = f32::from(strength.min(100)) / 100.0;
    let seconds = t.0 as f64 / 1e6;
    let axis = |channel: u64| {
        let noise = layered(seed, channel, seconds, &DRIFT);
        noise * (1.0 - SETTLE_WEIGHT) + settle(seed, channel, seconds) * SETTLE_WEIGHT
    };
    Sway {
        shift: [axis(0), axis(1)].map(|v| v * SHIFT * amount),
        turn: layered(seed, 2, seconds, &ROLL) * TURN * amount,
    }
}

/// How much a camera showing `width` by `height` pixels is scaled up at `strength`, so the
/// farthest shift and rotation keep its edges out of view.
pub fn overscale(strength: u8, width: f32, height: f32) -> f32 {
    let amount = f32::from(strength.min(100)) / 100.0;
    if amount == 0.0 {
        return 1.0;
    }
    let reach = 2.0 * SHIFT * amount * width.min(height);
    let (sin, cos) = (TURN * amount).sin_cos();
    let across = (width * cos + height * sin) / (width - reach);
    let down = (width * sin + height * cos) / (height - reach);
    across.max(down)
}

/// The seed of a take's drift, so each take moves its own way.
pub fn seed(take: Option<&Id>) -> u64 {
    take.map_or(0, |id| {
        id.as_str().bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
    })
}

/// Moves every camera layer of `scene` by the sway at `t`: overscaled, shifted and turned
/// inside its own crop, for sources of `sizes`.
pub fn apply(scene: &mut Scene, strength: u8, seed: u64, t: Micros, sizes: Sources<(u32, u32)>) {
    if strength == 0 {
        return;
    }
    let sway = sway(strength, seed, t);
    let (tex_w, tex_h) = sizes.camera;
    let (tex_w, tex_h) = (tex_w as f32, tex_h as f32);
    for layer in scene
        .layers
        .iter_mut()
        .filter(|l| l.source == Source::Camera)
    {
        let c = layer.crop;
        let (width, height) = (c.width() * tex_w, c.height() * tex_h);
        let scale = overscale(strength, width, height);
        let side = width.min(height);
        let center = [
            (c.left + c.right) / 2.0 + sway.shift[0] * side / tex_w,
            (c.top + c.bottom) / 2.0 + sway.shift[1] * side / tex_h,
        ];
        let (half_w, half_h) = (c.width() / 2.0 / scale, c.height() / 2.0 / scale);
        layer.crop = Rect::new(
            center[0] - half_w,
            center[1] - half_h,
            center[0] + half_w,
            center[1] + half_h,
        );
        layer.turn = sway.turn;
    }
}

/// Noise layers summed and brought back to -1..1.
fn layered(seed: u64, channel: u64, seconds: f64, layers: &[(f32, f32)]) -> f32 {
    let total: f32 = layers.iter().map(|(_, w)| w).sum();
    let sum: f32 = layers
        .iter()
        .enumerate()
        .map(|(i, &(hz, weight))| {
            noise(
                seed ^ (channel << 8 | i as u64).wrapping_mul(0x9e37_79b9),
                seconds * f64::from(hz),
            ) * weight
        })
        .sum();
    sum / total
}

/// Smooth value noise in -1..1: random values on whole numbers, eased between with a quintic so
/// speed and acceleration are continuous.
fn noise(seed: u64, x: f64) -> f32 {
    let cell = x.floor();
    let f = (x - cell) as f32;
    let ease = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let at = |i: i64| unit(seed, i as u64);
    let i = cell as i64;
    at(i) + (at(i + 1) - at(i)) * ease
}

/// A level that holds, then once in a cell moves to a new one over [`SETTLE_FOR`], in -1..1.
fn settle(seed: u64, channel: u64, seconds: f64) -> f32 {
    let seed = seed ^ (0x5e77_1e00 + channel).wrapping_mul(0xff51_afd7_ed55_8ccd);
    let cell = (seconds / f64::from(SETTLE_EVERY)).floor() as i64;
    let level = |k: i64| unit(seed, k as u64);
    // Somewhere in the first half of the cell, so the move ends well inside it.
    let start =
        (cell as f32 + 0.1 + 0.4 * (unit(seed ^ 1, cell as u64) * 0.5 + 0.5)) * SETTLE_EVERY;
    let f = ((seconds as f32 - start) / SETTLE_FOR).clamp(0.0, 1.0);
    let ease = f * f * (3.0 - 2.0 * f);
    level(cell - 1) + (level(cell) - level(cell - 1)) * ease
}

/// A random value in -1..1 for `i`, from SplitMix64.
fn unit(seed: u64, i: u64) -> f32 {
    let mut z = seed.wrapping_add(i.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layer;

    const FPS: u64 = 60;

    fn frames(strength: u8, seed: u64, seconds: u64) -> Vec<Sway> {
        (0..seconds * FPS)
            .map(|i| sway(strength, seed, Micros(i * 1_000_000 / FPS)))
            .collect()
    }

    #[test]
    fn the_same_take_and_moment_sway_the_same_and_takes_differ() {
        let t = Micros(12_345_678);
        assert_eq!(sway(60, 7, t), sway(60, 7, t));
        assert_ne!(sway(60, 7, t), sway(60, 8, t));
        assert_eq!(
            sway(0, 7, t),
            Sway {
                shift: [0.0, 0.0],
                turn: 0.0
            }
        );
    }

    #[test]
    fn the_drift_stays_in_bounds_and_moves_slowly() {
        let sways = frames(100, seed(Some(&Id::new("t01").unwrap())), 120);
        for s in &sways {
            assert!(s.shift.iter().all(|v| v.abs() <= SHIFT));
            assert!(s.turn.abs() <= TURN);
        }
        // It moves, and never faster than a slow pan: under 2 % of the side per second.
        let travel = |a: &Sway, b: &Sway| (b.shift[0] - a.shift[0]).hypot(b.shift[1] - a.shift[1]);
        let fastest = sways
            .windows(2)
            .map(|w| travel(&w[0], &w[1]))
            .fold(0.0, f32::max);
        assert!(fastest * FPS as f32 <= 0.02, "{fastest}");
        let spread = sways
            .iter()
            .map(|s| s.shift[0])
            .fold(0.0, |m: f32, v| m.max(v.abs()));
        assert!(spread > SHIFT * 0.3, "{spread}");
    }

    #[test]
    fn a_full_frame_camera_shows_no_edge_at_any_moment() {
        let sizes = Sources {
            screen: (3840, 2160),
            camera: (1920, 1080),
        };
        let full = Rect::new(0.0, 0.0, 3840.0, 2160.0);
        for strength in [30, 80, 100] {
            for i in 0..(60 * FPS) {
                let mut scene = Scene {
                    background: [0.0; 3],
                    layers: vec![Layer::new(Source::Camera, full, Rect::UNIT, 0.0)],
                };
                apply(&mut scene, strength, 3, Micros(i * 1_000_000 / FPS), sizes);
                let layer = scene.layers[0];
                // The crop's corners, turned around its center in pixels, stay in the source.
                let (w, h) = (1920.0, 1080.0);
                let c = layer.crop;
                let center = [(c.left + c.right) / 2.0 * w, (c.top + c.bottom) / 2.0 * h];
                let (sin, cos) = layer.turn.sin_cos();
                for (x, y) in [
                    (c.left, c.top),
                    (c.right, c.top),
                    (c.left, c.bottom),
                    (c.right, c.bottom),
                ] {
                    let (dx, dy) = (x * w - center[0], y * h - center[1]);
                    let (rx, ry) = (
                        center[0] + cos * dx - sin * dy,
                        center[1] + sin * dx + cos * dy,
                    );
                    assert!((-0.01..=w + 0.01).contains(&rx) && (-0.01..=h + 0.01).contains(&ry));
                }
            }
        }
    }

    #[test]
    fn overscale_is_slight_and_grows_with_strength() {
        assert_eq!(overscale(0, 1920.0, 1080.0), 1.0);
        let (low, high) = (
            overscale(30, 1920.0, 1080.0),
            overscale(100, 1920.0, 1080.0),
        );
        assert!(1.0 < low && low < high && high < 1.07, "{low} {high}");
    }
}
