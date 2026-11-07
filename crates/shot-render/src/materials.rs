//! The rules materials follow, shared by preview and export: the one light that tilted sheets
//! and shadows agree on, parallax, and adaptive motion blur. Pure; the material pass draws.
//!
//! Motion blur samples a shutter of [`SHUTTER`] around the frame, half a 60 fps frame: a sheet
//! is placed at the shutter's opening and closing ([`shutter`]) and a zoom's crop is taken at
//! both. How many samples depends on how far the moving thing travels on screen during the
//! shutter, measured in pixels of a 4K canvas so a preview and an export take the same number:
//! 1 when still, up to 8 on fast moves ([`samples`]).

use shot_core::Micros;

use crate::drawing::Placement;
use crate::layout::Canvas;

/// How long the virtual shutter stays open around each frame.
pub const SHUTTER: Micros = Micros(8_333);
/// The most motion blur samples a move takes.
pub const MAX_SAMPLES: usize = 8;
/// On-screen travel per sample, in 4K pixels.
const STEP: f32 = 3.0;

/// How many samples a move of `travel` pixels of a `canvas` during the shutter takes.
pub fn samples(travel: f32, canvas: Canvas) -> usize {
    let uhd = travel * 3840.0 / canvas.width as f32;
    ((uhd / STEP).ceil() as usize).clamp(1, MAX_SAMPLES)
}

/// The moments the shutter around `t` opens and closes.
pub fn shutter(t: Micros) -> [Micros; 2] {
    let half = SHUTTER.0 / 2;
    [Micros(t.0.saturating_sub(half)), Micros(t.0 + half)]
}

/// The scene's one light: tilted sheets brighten where they face it and shadows fall away from
/// it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    /// Unit vector towards the light: x right, y down, z towards the viewer.
    pub direction: [f32; 3],
}

/// Above the top left, in front of the screen.
pub const LIGHT: Light = Light {
    direction: [-0.38, -0.64, 0.67],
};

impl Light {
    /// Where the shadow of something `elevation` pixels in front of the canvas falls, relative to
    /// it.
    pub fn shadow_offset(self, elevation: f32) -> [f32; 2] {
        let [x, y, z] = self.direction;
        [-x / z * elevation, -y / z * elevation]
    }

    /// How much brighter (positive) or darker a sheet tilted by `tilt` degrees looks than a flat
    /// one: the change in how directly it faces the light.
    pub fn sheen(self, tilt: [f32; 2]) -> f32 {
        let [x, y, z] = self.direction;
        let normal = normal(tilt);
        normal[0] * x + normal[1] * y + normal[2] * z - z
    }
}

/// The normal of a sheet tilted by `tilt` degrees about the horizontal, then the vertical axis.
fn normal([about_x, about_y]: [f32; 2]) -> [f32; 3] {
    let (sx, cx) = about_x.to_radians().sin_cos();
    let (sy, cy) = about_y.to_radians().sin_cos();
    // Leaning the top away turns the normal up (negative y), the right side away turns it right.
    [sy * cx, -sx, cx * cy]
}

/// A placement moved by parallax: something `depth` times as far from the viewer as the canvas
/// moves `depth` times less when the camera drifts by `camera` pixels. Nearer things have a depth
/// below 1 and move more.
pub fn parallax(placement: Placement, depth: f32, camera: [f32; 2]) -> Placement {
    let factor = 1.0 / depth.max(0.1);
    Placement {
        center: [
            placement.center[0] + camera[0] * factor,
            placement.center[1] + camera[1] * factor,
        ],
        ..placement
    }
}

/// Where a camera drifting `speed` pixels per second along `direction` is at `t`, passing its
/// rest position at `middle`: the slow move that gives parallax while something holds.
pub fn drift(t: Micros, middle: Micros, speed: f32, direction: [f32; 2]) -> [f32; 2] {
    let seconds = (t.0 as f64 - middle.0 as f64) as f32 / 1e6;
    [
        direction[0] * speed * seconds,
        direction[1] * speed * seconds,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_grow_with_on_screen_speed_the_same_in_preview_and_export() {
        assert_eq!(samples(0.0, Canvas::UHD), 1);
        assert_eq!(samples(2.9, Canvas::UHD), 1);
        assert_eq!(samples(9.0, Canvas::UHD), 3);
        assert_eq!(samples(500.0, Canvas::UHD), MAX_SAMPLES);
        let preview = Canvas {
            width: 1920,
            height: 1080,
        };
        assert_eq!(samples(4.5, preview), samples(9.0, Canvas::UHD));
    }

    #[test]
    fn sheets_facing_the_light_brighten_and_shadows_fall_away_from_it() {
        assert_eq!(LIGHT.sheen([0.0, 0.0]), 0.0);
        // The light is above: leaning the top back faces it more.
        assert!(LIGHT.sheen([15.0, 0.0]) > 0.0);
        assert!(LIGHT.sheen([-15.0, 0.0]) < 0.0);
        let [dx, dy] = LIGHT.shadow_offset(40.0);
        assert!(dx > 0.0 && dy > 0.0 && dy > dx);
    }
}
