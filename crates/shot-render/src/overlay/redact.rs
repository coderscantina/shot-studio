//! The `blur` and `bleep` templates: hiding what the screen shows and what the voice says.
//!
//! `blur` averages a rectangle of the screen recording over blocks, so no detail smaller than a
//! block survives. It follows the layout and zoom like a highlight, and switches on and off
//! hard: a fade would show what it hides.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `x`, `y`, `width`, `height` | the rectangle in pixels of the screen track |
//! | `strength` | optional, the block size in screen pixels, 32 by default |
//!
//! `bleep` replaces the voice over its range with a 1 kHz tone, after the voice chain so the
//! denoiser never learns the tone as noise. It has no parameters and draws nothing.
//!
//! Both are placed from the transcript: [`phrase_range`] finds the words, and
//! [`super::highlight::find_on_screen`] where the same words show on screen.

use shot_core::{Overlay, TimeRange, Word};

use super::{ParamKind, ParamSpec, Params, Stage, TemplateSpec};
use crate::drawing::intersect;
use crate::layout::{Layer, Rect};

pub const BLUR_TEMPLATE: &str = "blur";
pub const BLEEP_TEMPLATE: &str = "bleep";

pub const BLUR: TemplateSpec = TemplateSpec {
    name: BLUR_TEMPLATE,
    about: "hides a rectangle of the screen",
    params: &[
        ParamSpec::required("x", ParamKind::Integer, "left edge, screen pixels"),
        ParamSpec::required("y", ParamKind::Integer, "top edge, screen pixels"),
        ParamSpec::required("width", ParamKind::Integer, "screen pixels"),
        ParamSpec::required("height", ParamKind::Integer, "screen pixels"),
        ParamSpec::optional("strength", ParamKind::Integer, "block size, screen pixels"),
    ],
    animated: false,
};

pub const BLEEP: TemplateSpec = TemplateSpec {
    name: BLEEP_TEMPLATE,
    about: "a tone over the words",
    params: &[],
    animated: false,
};

/// The default block size of a blur in screen pixels.
const STRENGTH: i64 = 32;

pub(crate) fn apply_blur(overlay: &Overlay, stage: &mut Stage) {
    let Some((layer, view)) = stage.screen.as_ref() else {
        return;
    };
    let p = Params(overlay);
    let (Some(x), Some(y), Some(w), Some(h)) =
        (p.int("x"), p.int("y"), p.int("width"), p.int("height"))
    else {
        return;
    };
    let wanted = view.map_rect(Rect::new(
        x as f32,
        y as f32,
        (x + w) as f32,
        (y + h) as f32,
    ));
    let Some(frame) = intersect(wanted, layer.frame) else {
        return;
    };
    let crop_at = |cx: f32, cy: f32| {
        [
            layer.crop.left + (cx - layer.frame.left) / layer.frame.width() * layer.crop.width(),
            layer.crop.top + (cy - layer.frame.top) / layer.frame.height() * layer.crop.height(),
        ]
    };
    let [left, top] = crop_at(frame.left, frame.top);
    let [right, bottom] = crop_at(frame.right, frame.bottom);
    let strength = p.int("strength").unwrap_or(STRENGTH).max(1) as f32;
    stage.scene.layers.push(Layer {
        source: layer.source,
        frame,
        crop: Rect::new(left, top, right, bottom),
        radius: 0.0,
        blur: (strength * view.scale()).max(2.0),
    });
}

/// The ranges of every `bleep` in `overlays`.
pub fn bleeps<'a>(overlays: impl IntoIterator<Item = &'a Overlay>) -> Vec<TimeRange> {
    overlays
        .into_iter()
        .filter(|o| o.template.as_str() == BLEEP_TEMPLATE)
        .map(|o| o.range)
        .collect()
}

/// The range of the first run of `words` reading `phrase`, ignoring case and punctuation.
pub fn phrase_range(words: &[Word], phrase: &str) -> Option<TimeRange> {
    let key = |text: &str| -> String {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let wanted: Vec<String> = phrase
        .split_whitespace()
        .map(key)
        .filter(|w| !w.is_empty())
        .collect();
    if wanted.is_empty() {
        return None;
    }
    let run = words
        .windows(wanted.len())
        .find(|run| run.iter().zip(&wanted).all(|(w, k)| key(&w.text) == *k))?;
    TimeRange::new(run[0].range.from(), run[run.len() - 1].range.to())
}

/// A tone sample for a bleep at output sample `n`: 1 kHz at -20 dBFS, about as loud as the
/// voice before the loudness finish, faded over 5 ms at its edges `length` samples apart.
pub fn tone(n: u64, offset: u64, length: u64) -> f32 {
    const RATE: f32 = 48_000.0;
    const FADE: u64 = 240;
    let edge = offset.min(length.saturating_sub(offset + 1)).min(FADE) as f32 / FADE as f32;
    let phase = n as f32 * 1_000.0 / RATE;
    0.1 * edge * (std::f32::consts::TAU * phase.fract()).sin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::Micros;

    #[test]
    fn a_phrase_spans_its_words() {
        let words: Vec<Word> = ["my", "password", "is", "hunter2."]
            .iter()
            .enumerate()
            .map(|(i, w)| Word {
                text: (*w).to_owned(),
                range: TimeRange::new(
                    Micros(i as u64 * 500_000),
                    Micros(i as u64 * 500_000 + 400_000),
                )
                .unwrap(),
                confidence: 90,
            })
            .collect();
        let range = phrase_range(&words, "is Hunter2").unwrap();
        assert_eq!(
            (range.from(), range.to()),
            (Micros(1_000_000), Micros(1_900_000))
        );
        assert_eq!(phrase_range(&words, "swordfish"), None);
    }
}
