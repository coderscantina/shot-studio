//! What an intro or outro draws besides its bumper ([0021]): the generated title, a segment on
//! the brand's background, or drawn over the bumper when both are on. The logo comes first,
//! played by the brand's `logo-reveal` Lottie when it declares one, else popping in on the
//! preset's springs, then the title in the preset's kinetic type and an accent rule under it.
//! Everything leaves on the springs before the bookend ends. Pure: it only adds shapes.
//!
//! [0021]: ../../../docs/decisions/0021-sound-transitions-and-bookends.md

use shot_core::{Micros, TimeRange};

use crate::drawing::{Drawing, Placement, Shape, Sheet};
use crate::kinetic::Kinetic;
use crate::layout::{Canvas, Rect};
use crate::materials;
use crate::motion::Part;
use crate::theme::{LOGO_REVEAL, Logo, Theme};

/// How long a generated title plays on its own, about 3 s.
pub const TITLE_LENGTH: Micros = Micros(3_000_000);

/// Draws the generated title `text` at `t` into a bookend `length` long. A bumper draws under
/// it; without one the caller fills the background.
pub fn title(
    theme: &Theme,
    text: &str,
    t: Micros,
    length: Micros,
    canvas: Canvas,
    drawing: &mut Drawing,
) {
    let Some(range) = TimeRange::new(Micros(0), length) else {
        return;
    };
    let s = canvas.width as f32 / 3840.0;
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let motion = theme.motion;
    let left = w * 0.1;
    if let Some(reveal) = theme.sticker(LOGO_REVEAL, None) {
        let (width, height) = reveal.size();
        let scale = w / width as f32;
        drawing.over.push(Shape::Lottie {
            lottie: reveal,
            origin: [0.0, (h - height as f32 * scale) / 2.0],
            scale,
            elapsed: t,
            opacity: 1.0,
            font: theme.fonts.body(),
        });
    } else if let Some(logo) = &theme.logo {
        let height = 260.0 * s;
        let width = height * logo.image.width as f32 / logo.image.height.max(1) as f32;
        let center = [left + width / 2.0, h * 0.5 - 300.0 * s];
        let place = |t| {
            let pose = motion.pose(range, Part::new(0, 3), t);
            (0.8 + 0.2 * pose.enter, pose.presence())
        };
        if let Some(sheet) = logo_sheet(logo, center, height, materials::shutter(t).map(place)) {
            drawing.sheets.push(sheet);
        }
    }
    let size = 170.0 * s;
    let text_range = TimeRange::new(Micros(motion.stagger.0), range.to()).unwrap_or(range);
    drawing.over.push(Shape::Kinetic(Box::new(Kinetic {
        origin: [left, h * 0.5 - size * 0.4],
        text: text.to_owned(),
        size,
        font: theme.fonts.heading(),
        color: theme.palette.text,
        width: Some(w * 0.8),
        reveal: theme.moves.reveal,
        unit: theme.moves.unit,
        motion,
        range: text_range,
        t: motion.freeze(text_range, text.split_whitespace().count(), t),
    })));
    let rule = motion.pose(range, Part::new(2, 3), t);
    if rule.visible() {
        let top = h * 0.5 + size * 1.25;
        drawing.over.push(Shape::Rect {
            rect: Rect::new(
                left,
                top,
                left + 360.0 * s * rule.enter.clamp(0.0, 1.2),
                top + 16.0 * s,
            ),
            radius: 8.0 * s,
            color: theme.palette.accent.faded(rule.presence()),
        });
    }
}

/// The logo as a sheet `height` canvas pixels tall at `center`, at each end of the shutter
/// scaled by the first value and faded by the second. Drawn flat at its own size, so vello reads
/// every texel exactly wherever the image sits in its atlas, and scaled by the sheet's placement.
pub(crate) fn logo_sheet(
    logo: &Logo,
    center: [f32; 2],
    height: f32,
    poses: [(f32, f32); 2],
) -> Option<Sheet> {
    if poses.iter().all(|(_, opacity)| *opacity <= 0.0) {
        return None;
    }
    let (width, image_height) = (logo.image.width as f32, logo.image.height as f32);
    let fit = height / image_height.max(1.0);
    Some(Sheet {
        size: [width, image_height],
        radius: 0.0,
        glass: None,
        shapes: vec![Shape::Image {
            rect: Rect::new(0.0, 0.0, width, image_height),
            radius: 0.0,
            image: logo.image.clone(),
            crop: Rect::UNIT,
            opacity: 1.0,
        }],
        placement: poses.map(|(scale, opacity)| Placement {
            center,
            scale: fit * scale,
            tilt: [0.0, 0.0],
            opacity,
        }),
    })
}
