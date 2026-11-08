//! The `b-roll` template: a cutaway that covers the frame.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `file` | optional, relative to the video folder: a still (`.png`, `.jpg`), a clip (`.mp4`, `.mov`) or a diff snapshot (`.diff`). Without it the overlay is an empty slot waiting for a pick, and draws nothing |
//! | `start` | optional, the media time in the clip where the cutaway starts, 0 by default |
//!
//! A still fades in on the preset's springs, settling from a little closer, and pushes in slowly. A clip cuts in and out
//! hard: the frame's screen layer shows the clip ([`super::Cutaway`]) and nothing of the take
//! draws under it. A diff types out on a dark panel like `code-diff`. Files come from the
//! video's `sources/`, other takes' screen recordings, git diffs or the channel's `library/`.
//!
//! [`slots`] places empty slots over idle screen and at B markers.

use shot_core::{Micros, Overlay, ParamValue, TimeRange};

use super::{Cutaway, ParamKind, ParamSpec, Params, Stage, TemplateSpec, overlay};
use crate::code_diff::CodeDiffOverlay;
use crate::drawing::{Placement, Shape, Sheet};
use crate::layout::{Layer, Rect, Source};
use crate::materials;
use crate::motion::Part;

pub const TEMPLATE: &str = "b-roll";

pub const SPEC: TemplateSpec = TemplateSpec {
    name: TEMPLATE,
    about: "a still, clip or diff over the frame",
    params: &[
        ParamSpec::optional("file", ParamKind::Text, "the still, clip or diff"),
        ParamSpec::optional("start", ParamKind::Time, "where in the clip to start"),
    ],
    animated: true,
};

/// How far a still pushes in over its range.
const PUSH: f32 = 0.06;
/// How much closer a still starts before it settles, at full density.
const SETTLE: f32 = 0.05;
/// Idle screen at least this long gets a slot.
pub const IDLE_MIN: Micros = Micros(8_000_000);
/// How long a slot at a B marker lasts.
pub const MARKER_HOLD: Micros = Micros(4_000_000);

/// What a b-roll file is, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Still,
    Clip,
    Diff,
}

impl Media {
    pub fn of(file: &str) -> Option<Self> {
        let ext = file.rsplit_once('.')?.1.to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" => Some(Self::Still),
            "mp4" | "mov" | "m4v" => Some(Self::Clip),
            "diff" | "patch" => Some(Self::Diff),
            _ => None,
        }
    }
}

/// The file of a `b-roll` overlay and what it is, `None` for an empty slot or an unknown file.
pub fn file(overlay: &Overlay) -> Option<(&str, Media)> {
    if overlay.template.as_str() != TEMPLATE {
        return None;
    }
    let file = Params(overlay).text("file")?;
    Some((file, Media::of(file)?))
}

pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    let Some((path, media)) = file(overlay) else {
        return;
    };
    let canvas = stage.canvas;
    let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
    let params = Params(overlay);
    let elapsed = Micros(stage.t.0 - overlay.range.from().0);
    match media {
        Media::Clip => {
            *stage.cutaway = Some(Cutaway {
                file: path.to_owned(),
                time: Micros(params.time("start").unwrap_or(Micros(0)).0 + elapsed.0),
            });
            stage.scene.layers = vec![Layer {
                source: Source::Screen,
                frame: full,
                // The export and preview fit the clip once they know its size, see `fit_cutaway`.
                crop: Rect::UNIT,
                radius: 0.0,
                blur: 0.0,
                shutter: Rect::ZERO,
            }];
            // Nothing of the take shows through a hard cut.
            stage.drawing.under.clear();
            stage.drawing.over.clear();
            *stage.screen = None;
        }
        Media::Still => {
            let Some(image) = stage.look.images.get(path) else {
                return;
            };
            let motion = params.motion(stage.theme());
            let (width, height) = (image.width as f32, image.height as f32);
            let fit = (canvas.width as f32 / width).max(canvas.height as f32 / height);
            let place = |t: Micros| {
                let pose = motion.pose(overlay.range, Part::WHOLE, t);
                let since = t.0.saturating_sub(overlay.range.from().0);
                let progress = since as f32 / overlay.range.duration().0.max(1) as f32;
                // Settles in from a little closer, then pushes in slowly.
                let settle = (1.0 - pose.enter.clamp(0.0, 1.0)) * SETTLE * motion.travel.min(1.0);
                Placement {
                    center: [canvas.width as f32 / 2.0, canvas.height as f32 / 2.0],
                    scale: fit * (1.0 + PUSH * progress + settle),
                    tilt: [0.0, 0.0],
                    opacity: pose.presence(),
                }
            };
            let placement = materials::shutter(stage.t).map(place);
            if placement.iter().all(|p| p.opacity >= 1.0) {
                // A cutaway at full strength hides everything drawn before it.
                stage.drawing.over.clear();
                stage.drawing.sheets.clear();
                stage.drawing.shadows.clear();
            }
            // Drawn flat at its own size, so vello reads every texel exactly wherever the image
            // sits in its atlas, and scaled by the sheet's placement.
            stage.drawing.sheets.push(Sheet {
                size: [width, height],
                radius: 0.0,
                glass: None,
                shapes: vec![Shape::Image {
                    rect: Rect::new(0.0, 0.0, width, height),
                    radius: 0.0,
                    image: image.clone(),
                    crop: Rect::UNIT,
                    opacity: 1.0,
                }],
                placement,
            });
        }
        Media::Diff => {
            let Some(diff) = stage.look.diffs.get(path) else {
                return;
            };
            let theme = stage.theme();
            let motion = params.motion(theme);
            let presence = motion.pose(overlay.range, Part::WHOLE, stage.t).presence();
            stage.drawing.over.push(Shape::Rect {
                rect: full,
                radius: 0.0,
                color: theme.palette.background.faded(presence),
            });
            let panel = CodeDiffOverlay {
                range: overlay.range,
                diff: path.to_owned(),
                highlight_line: None,
                highlight_at: None,
            };
            let shapes = panel.draw(diff, stage.t, canvas, theme, &motion);
            stage.drawing.over.extend(shapes);
        }
    }
}

/// Fits the clip of a frame's cutaway, `size` pixels, to cover the canvas.
pub fn fit_cutaway(frame: &mut crate::frame::Frame, (width, height): (u32, u32)) {
    if frame.cutaway.is_none() {
        return;
    }
    for layer in &mut frame.scene.layers {
        layer.crop = cover(
            width as f32 / height.max(1) as f32,
            layer.frame.width() / layer.frame.height(),
            1.0,
        );
    }
}

/// The part of a source of aspect `source` that covers a frame of aspect `frame`, centered and
/// zoomed in by `zoom`, in fractions of the source.
pub fn cover(source: f32, frame: f32, zoom: f32) -> Rect {
    let (w, h) = if source > frame {
        (frame / source, 1.0)
    } else {
        (1.0, source / frame)
    };
    let (w, h) = (w / zoom, h / zoom);
    Rect::new(
        (1.0 - w) / 2.0,
        (1.0 - h) / 2.0,
        (1.0 + w) / 2.0,
        (1.0 + h) / 2.0,
    )
}

/// Empty `b-roll` slots: one over each idle stretch of at least [`IDLE_MIN`] and one from each
/// B marker for [`MARKER_HOLD`], ending early where the next slot starts. A slot that overlaps
/// an existing b-roll is left out, so the rule can run again.
pub fn slots(idle: &[TimeRange], markers: &[Micros], existing: &[Overlay]) -> Vec<Overlay> {
    let mut ranges: Vec<TimeRange> = idle
        .iter()
        .copied()
        .filter(|r| r.duration() >= IDLE_MIN)
        .collect();
    for &t in markers {
        if ranges.iter().any(|r| r.from() <= t && t < r.to()) {
            continue;
        }
        let next = ranges
            .iter()
            .map(|r| r.from())
            .chain(markers.iter().copied())
            .filter(|&s| s > t)
            .min();
        let end = Micros((t.0 + MARKER_HOLD.0).min(next.map_or(u64::MAX, |n| n.0)));
        ranges.extend(TimeRange::new(t, end));
    }
    ranges.sort_by_key(|r| r.from());
    ranges
        .into_iter()
        .filter(|r| {
            !existing
                .iter()
                .any(|o| o.template.as_str() == TEMPLATE && o.range.overlaps(*r))
        })
        .map(|r| overlay(TEMPLATE, r, Vec::<(&str, ParamValue)>::new()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    #[test]
    fn slots_cover_long_idle_stretches_and_every_b_marker() {
        let idle = [range(0, 5_000_000), range(10_000_000, 19_000_000)];
        let markers = [Micros(12_000_000), Micros(30_000_000), Micros(32_000_000)];
        let got: Vec<TimeRange> = slots(&idle, &markers, &[])
            .iter()
            .map(|o| o.range)
            .collect();
        assert_eq!(
            got,
            [
                range(10_000_000, 19_000_000),
                range(30_000_000, 32_000_000),
                range(32_000_000, 36_000_000),
            ]
        );
        let filled = vec![overlay(
            TEMPLATE,
            range(30_000_000, 32_000_000),
            vec![("file", ParamValue::Text("sources/a.png".into()))],
        )];
        assert_eq!(slots(&idle, &markers, &filled).len(), 2);
    }

    #[test]
    fn files_are_known_by_extension() {
        assert_eq!(Media::of("sources/shot.PNG"), Some(Media::Still));
        assert_eq!(Media::of("media/t02/screen.mp4"), Some(Media::Clip));
        assert_eq!(Media::of("sources/fix.diff"), Some(Media::Diff));
        assert_eq!(Media::of("sources/notes.md"), None);
    }
}
