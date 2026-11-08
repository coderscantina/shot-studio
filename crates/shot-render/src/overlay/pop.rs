//! The `pop` template: an emoji, a word or a channel sticker that pops in on the punchline.
//!
//! | Parameter | Value |
//! | --- | --- |
//! | `text` | the emoji or word. Required unless `sticker` is set |
//! | `sticker` | optional, a Lottie template the brand declares, drawn instead, its `text` slot set to `text` |
//! | `x`, `y` | optional, the center in pixels of the 4K output frame, by default right of the middle in the upper third |
//! | `size` | optional, its height in pixels of the 4K output frame, 360 by default |
//!
//! An emoji of the bundled animated set ([`EMOJI`], from Noto's animated emoji) plays its
//! animation on a loop, and a word draws in the brand's heading font on an accent pill. Other
//! emoji are refused: the system's emoji glyphs differ between Macs, and vello samples their
//! bitmaps differently depending on what it drew before. Everything scales in on the preset's entrance
//! spring, past full size where the preset bounces, and shrinks away on exit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use shot_core::{Micros, Overlay};

use super::{ParamKind, ParamSpec, Params, Stage, TemplateError, TemplateSpec};
use crate::drawing::Shape;
use crate::layout::Rect;
use crate::lottie::{Filled, LottieTemplate};
use crate::motion::{Part, Reveal};

pub const TEMPLATE: &str = "pop";

pub const SPEC: TemplateSpec = TemplateSpec {
    name: TEMPLATE,
    about: "an emoji, a word or a sticker popping in",
    params: &[
        ParamSpec::optional("text", ParamKind::Text, "the emoji or word"),
        ParamSpec::optional("sticker", ParamKind::Text, "a brand sticker instead"),
        ParamSpec::optional("x", ParamKind::Integer, "center, 4K output pixels"),
        ParamSpec::optional("y", ParamKind::Integer, "center, 4K output pixels"),
        ParamSpec::optional("size", ParamKind::Integer, "height, 4K output pixels"),
    ],
    animated: true,
};

/// The default height, in 4K pixels.
const SIZE: f32 = 360.0;

/// The bundled animated emoji: Noto's animated emoji, curated to those velato draws exactly.
/// See `sources/manifest.json` for their license and the one change made to four of them.
pub const EMOJI: [(&str, &[u8]); 11] = [
    ("👍", include_bytes!("../../assets/emoji/1f44d.json")),
    ("👏", include_bytes!("../../assets/emoji/1f44f.json")),
    ("😍", include_bytes!("../../assets/emoji/1f60d.json")),
    ("🥳", include_bytes!("../../assets/emoji/1f973.json")),
    ("✅", include_bytes!("../../assets/emoji/2705.json")),
    ("✨", include_bytes!("../../assets/emoji/2728.json")),
    ("❤", include_bytes!("../../assets/emoji/2764_fe0f.json")),
    ("🔥", include_bytes!("../../assets/emoji/1f525.json")),
    ("😂", include_bytes!("../../assets/emoji/1f602.json")),
    ("👀", include_bytes!("../../assets/emoji/1f440.json")),
    ("🎉", include_bytes!("../../assets/emoji/1f389.json")),
];

/// The animation of a bundled emoji, parsed on first use. Variation selectors are ignored, so
/// "❤️" and "❤" are the same heart.
pub fn animated(text: &str) -> Option<Arc<Filled>> {
    static PARSED: OnceLock<Mutex<HashMap<&'static str, Arc<Filled>>>> = OnceLock::new();
    let bare: String = text.chars().filter(|c| *c != '\u{FE0F}').collect();
    let &(emoji, bytes) = EMOJI.iter().find(|(emoji, _)| *emoji == bare)?;
    let mut parsed = PARSED.get_or_init(Mutex::default).lock().ok()?;
    if let Some(filled) = parsed.get(emoji) {
        return Some(filled.clone());
    }
    let filled = LottieTemplate::load(bytes, &[]).ok()?.fill(&[]).ok()?;
    let filled = Arc::new(filled);
    parsed.insert(emoji, filled.clone());
    Some(filled)
}

/// Whether `text` reads as an emoji rather than a word: it has no letters or digits.
fn is_emoji(text: &str) -> bool {
    !text.chars().any(char::is_alphanumeric)
}

/// A pop needs `text` or `sticker`, and an emoji `text` must be one of [`EMOJI`].
pub(crate) fn check(overlay: &Overlay) -> Result<(), TemplateError> {
    let p = Params(overlay);
    let text = p.text("text");
    if text.is_none() && p.text("sticker").is_none() {
        return Err(TemplateError::Missing {
            template: TEMPLATE.to_owned(),
            param: "text".to_owned(),
        });
    }
    if p.text("sticker").is_none()
        && let Some(text) = text
        && is_emoji(text)
        && animated(text).is_none()
    {
        return Err(TemplateError::Kind {
            template: TEMPLATE.to_owned(),
            param: "text".to_owned(),
            kind: "a word or one of the bundled emoji 👍 👏 😍 🥳 ✅ ✨ ❤️ 🔥 😂 👀 🎉",
        });
    }
    Ok(())
}

pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    let p = Params(overlay);
    let theme = stage.theme();
    let (s, t) = (stage.scale(), stage.t);
    let motion = p.motion(theme);
    let pose = motion.pose(overlay.range, Part::WHOLE, t);
    if !pose.visible() {
        return;
    }
    let scale = match theme.moves.reveal {
        Reveal::Pop | Reveal::Mask => (0.2 + 0.8 * pose.enter) * (1.0 - 0.8 * pose.exit),
        Reveal::Rise | Reveal::Fade => (0.85 + 0.15 * pose.enter) * (1.0 - 0.3 * pose.exit),
    }
    .max(0.0);
    let opacity = (pose.enter * 2.5).clamp(0.0, 1.0) * (1.0 - pose.exit);
    // In 4K pixels, scaled to the canvas.
    let px = |name: &str, default: f32| p.int(name).map_or(default, |v| v as f32) * s;
    let center = [px("x", 3840.0 * 0.72), px("y", 2160.0 * 0.32)];
    let size = px("size", SIZE) * scale;
    let text = p.text("text");
    let elapsed = Micros(t.0 - overlay.range.from().0);
    let lottie = match p.text("sticker") {
        Some(sticker) => theme.sticker(sticker, text).map(|l| (l, false)),
        None => text.and_then(animated).map(|l| (l, true)),
    };
    if let Some((lottie, looping)) = lottie {
        let (width, height) = lottie.size();
        let fit = size / height as f32;
        let duration = lottie.duration().0.max(1);
        let elapsed = if looping {
            Micros(elapsed.0 % duration)
        } else {
            elapsed
        };
        stage.drawing.over.push(Shape::Lottie {
            lottie,
            origin: [
                center[0] - width as f32 * fit / 2.0,
                center[1] - height as f32 * fit / 2.0,
            ],
            scale: fit,
            elapsed,
            opacity,
            font: theme.fonts.body(),
        });
        return;
    }
    let Some(text) = text else {
        return;
    };
    if is_emoji(text) {
        return;
    }
    let palette = &theme.palette;
    let letters = size * 0.42;
    let width = text.chars().count() as f32 * letters * 0.62 + letters * 1.4;
    let height = letters * 1.7;
    stage.drawing.over.push(Shape::Rect {
        rect: Rect::new(
            center[0] - width / 2.0,
            center[1] - height / 2.0,
            center[0] + width / 2.0,
            center[1] + height / 2.0,
        ),
        radius: height * 0.3,
        color: palette.accent.faded(opacity),
    });
    stage.drawing.over.push(Shape::Centered {
        center,
        text: text.to_owned(),
        size: letters,
        font: theme.fonts.heading(),
        color: palette.on_accent().faded(opacity),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_emoji_pass_the_import_check_and_others_are_refused() {
        for (emoji, bytes) in EMOJI {
            assert_eq!(crate::lottie::check(bytes), [], "{emoji}");
            let lottie = animated(emoji).unwrap();
            assert!(lottie.duration() > Micros(300_000), "{emoji}");
        }
        assert!(animated("❤️").is_some());
        assert!(animated("🦀").is_none());
        let pop = |text: &str| {
            super::super::overlay(
                TEMPLATE,
                shot_core::TimeRange::new(Micros(0), Micros(1)).unwrap(),
                vec![("text", shot_core::ParamValue::Text(text.into()))],
            )
        };
        assert!(super::super::check(&pop("🔥")).is_ok());
        assert!(super::super::check(&pop("Ship it")).is_ok());
        assert!(matches!(
            super::super::check(&pop("🦀")),
            Err(TemplateError::Kind { .. })
        ));
    }
}
