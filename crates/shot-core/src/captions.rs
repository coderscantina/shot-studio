//! How burned-in captions look: the `captions` node of the brand and the look properties of a
//! format's `captions <style>` node. The style keyword is the preset, which sets every field;
//! the brand's node and then the video's format change single fields of it.

use crate::kdl_io::{Node, Writer, keyword_enum};
use crate::{CaptionStyle, Color, ParseError};

keyword_enum! {
    /// Which of the brand's fonts captions draw in.
    pub enum CaptionFont {
        Heading = "heading",
        Body = "body",
        Mono = "mono",
    }
}

keyword_enum! {
    /// What sits behind the words: a rounded box, an outline around each letter, or nothing.
    pub enum CaptionBack {
        Box = "box",
        Outline = "outline",
        None = "none",
    }
}

keyword_enum! {
    pub enum CaptionPosition {
        Bottom = "bottom",
        Middle = "middle",
        Top = "top",
    }
}

keyword_enum! {
    pub enum CaptionCase {
        /// The words as the transcript and the caption fixes spell them.
        AsSaid = "as-said",
        Upper = "upper",
    }
}

/// How the word being said stands out, fixed by the preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Emphasis {
    /// Every word alike.
    None,
    /// The word on a pill in the highlight color, popping in.
    Pill,
    /// Every word said so far in the highlight color.
    Fill,
    /// The word in the highlight color, popping in.
    Color,
}

/// Changes to a preset's look, each field optional. The same properties on the brand's
/// `captions` node and on a format's `captions <style>` node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct CaptionLook {
    pub font: Option<CaptionFont>,
    /// Percent of the preset's size, 50 to 200.
    pub size: Option<u16>,
    /// 100 to 900.
    pub weight: Option<u16>,
    pub color: Option<Color>,
    /// The color of the word being said. Unset, the brand's accent.
    pub highlight: Option<Color>,
    pub back: Option<CaptionBack>,
    pub position: Option<CaptionPosition>,
    /// Moves the caption down by this percent of the frame's height, up when negative, -30
    /// to 30.
    pub nudge: Option<i16>,
    /// At most this many words show at once, 1 to 12.
    pub words: Option<u8>,
    pub case: Option<CaptionCase>,
}

/// The look's properties in canonical order.
pub(crate) const LOOK_PROPS: [&str; 10] = [
    "font",
    "size",
    "weight",
    "color",
    "highlight",
    "back",
    "position",
    "nudge",
    "words",
    "case",
];

/// A preset with the brand's and the video's changes laid over it: every field set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptionDesign {
    pub style: CaptionStyle,
    pub emphasis: Emphasis,
    pub font: CaptionFont,
    pub size: u16,
    pub weight: u16,
    pub color: Color,
    /// `None` draws in the brand's accent.
    pub highlight: Option<Color>,
    pub back: CaptionBack,
    pub position: CaptionPosition,
    pub nudge: i16,
    pub words: u8,
    pub case: CaptionCase,
}

const WHITE: Color = Color([0xff, 0xff, 0xff]);
const YELLOW: Color = Color([0xff, 0xd8, 0x4d]);

impl CaptionDesign {
    /// The preset `style` with nothing changed.
    pub fn preset(style: CaptionStyle) -> Self {
        let base = Self {
            style,
            emphasis: Emphasis::Pill,
            font: CaptionFont::Heading,
            size: 100,
            weight: 700,
            color: WHITE,
            highlight: None,
            back: CaptionBack::Box,
            position: CaptionPosition::Bottom,
            nudge: 0,
            words: 7,
            case: CaptionCase::AsSaid,
        };
        match style {
            CaptionStyle::WordPop => base,
            CaptionStyle::Clean | CaptionStyle::Off => Self {
                emphasis: Emphasis::None,
                font: CaptionFont::Body,
                weight: 600,
                ..base
            },
            CaptionStyle::Karaoke => Self {
                emphasis: Emphasis::Fill,
                weight: 800,
                highlight: Some(YELLOW),
                words: 5,
                ..base
            },
            CaptionStyle::Bold => Self {
                emphasis: Emphasis::Color,
                size: 125,
                weight: 900,
                highlight: Some(YELLOW),
                back: CaptionBack::Outline,
                words: 3,
                case: CaptionCase::Upper,
                ..base
            },
        }
    }
}

impl CaptionLook {
    /// Reads the look's properties of `node`, which the caller has checked for unknown ones.
    pub(crate) fn read(node: Node<'_>) -> Result<Self, ParseError> {
        let look = Self {
            font: node.opt_prop("font")?,
            size: node.opt_prop("size")?,
            weight: node.opt_prop("weight")?,
            color: node.opt_prop("color")?,
            highlight: node.opt_prop("highlight")?,
            back: node.opt_prop("back")?,
            position: node.opt_prop("position")?,
            nudge: node.opt_prop("nudge")?,
            words: node.opt_prop("words")?,
            case: node.opt_prop("case")?,
        };
        let outside = |value: Option<i32>, low: i32, high: i32| {
            value.is_some_and(|v| !(low..=high).contains(&v))
        };
        if outside(look.size.map(i32::from), 50, 200) {
            return Err(node.error("`size` is a percentage from 50 to 200"));
        }
        if outside(look.weight.map(i32::from), 100, 900) {
            return Err(node.error("`weight` is from 100 to 900"));
        }
        if outside(look.nudge.map(i32::from), -30, 30) {
            return Err(node.error("`nudge` is a percentage from -30 to 30"));
        }
        if outside(look.words.map(i32::from), 1, 12) {
            return Err(node.error("`words` is from 1 to 12"));
        }
        Ok(look)
    }

    /// Writes the set properties onto the node being written.
    pub(crate) fn write(&self, w: &mut Writer) {
        fn opt(w: &mut Writer, key: &str, value: Option<impl crate::kdl_io::ToKdl>) {
            if let Some(value) = value {
                w.prop(key, &value);
            }
        }
        opt(w, "font", self.font);
        opt(w, "size", self.size);
        opt(w, "weight", self.weight);
        opt(w, "color", self.color);
        opt(w, "highlight", self.highlight);
        opt(w, "back", self.back);
        opt(w, "position", self.position);
        opt(w, "nudge", self.nudge);
        opt(w, "words", self.words);
        opt(w, "case", self.case);
    }

    /// The set properties as written, for chunk keys.
    pub fn key(&self) -> String {
        let mut w = Writer::default();
        w.node("captions");
        self.write(&mut w);
        w.finish()
    }

    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// `over` laid on this look, field by field.
    pub fn merge(&self, over: &Self) -> Self {
        Self {
            font: over.font.or(self.font),
            size: over.size.or(self.size),
            weight: over.weight.or(self.weight),
            color: over.color.or(self.color),
            highlight: over.highlight.or(self.highlight),
            back: over.back.or(self.back),
            position: over.position.or(self.position),
            nudge: over.nudge.or(self.nudge),
            words: over.words.or(self.words),
            case: over.case.or(self.case),
        }
    }

    /// The fields this look sets to something else than `base`: the layer that, laid on
    /// `base`, gives this look's values.
    pub fn diff(&self, base: &Self) -> Self {
        fn differs<T: PartialEq + Copy>(a: Option<T>, b: Option<T>) -> Option<T> {
            if a == b { None } else { a }
        }
        Self {
            font: differs(self.font, base.font),
            size: differs(self.size, base.size),
            weight: differs(self.weight, base.weight),
            color: differs(self.color, base.color),
            highlight: differs(self.highlight, base.highlight),
            back: differs(self.back, base.back),
            position: differs(self.position, base.position),
            nudge: differs(self.nudge, base.nudge),
            words: differs(self.words, base.words),
            case: differs(self.case, base.case),
        }
    }

    /// The preset `style` with this look's changes.
    pub fn design(&self, style: CaptionStyle) -> CaptionDesign {
        let preset = CaptionDesign::preset(style);
        CaptionDesign {
            style,
            emphasis: preset.emphasis,
            font: self.font.unwrap_or(preset.font),
            size: self.size.unwrap_or(preset.size),
            weight: self.weight.unwrap_or(preset.weight),
            color: self.color.unwrap_or(preset.color),
            highlight: self.highlight.or(preset.highlight),
            back: self.back.unwrap_or(preset.back),
            position: self.position.unwrap_or(preset.position),
            nudge: self.nudge.unwrap_or(preset.nudge),
            words: self.words.unwrap_or(preset.words),
            case: self.case.unwrap_or(preset.case),
        }
    }

    /// The fields of `design` that differ from its preset: the look that gives `design`.
    pub fn of(design: &CaptionDesign) -> Self {
        let preset = CaptionDesign::preset(design.style);
        fn differs<T: PartialEq>(a: T, b: T) -> Option<T> {
            (a != b).then_some(a)
        }
        Self {
            font: differs(design.font, preset.font),
            size: differs(design.size, preset.size),
            weight: differs(design.weight, preset.weight),
            color: differs(design.color, preset.color),
            highlight: if design.highlight == preset.highlight {
                None
            } else {
                design.highlight
            },
            back: differs(design.back, preset.back),
            position: differs(design.position, preset.position),
            nudge: differs(design.nudge, preset.nudge),
            words: differs(design.words, preset.words),
            case: differs(design.case, preset.case),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_changes_single_fields_of_the_preset_and_merges_field_by_field() {
        let brand = CaptionLook {
            size: Some(110),
            highlight: Some(Color([1, 2, 3])),
            ..CaptionLook::default()
        };
        let video = CaptionLook {
            size: Some(90),
            case: Some(CaptionCase::Upper),
            ..CaptionLook::default()
        };
        let design = brand.merge(&video).design(CaptionStyle::Karaoke);
        assert_eq!(design.size, 90);
        assert_eq!(design.highlight, Some(Color([1, 2, 3])));
        assert_eq!(design.case, CaptionCase::Upper);
        assert_eq!(design.emphasis, Emphasis::Fill);
        assert_eq!(design.words, 5, "the preset's");
        assert_eq!(
            CaptionLook::of(&design),
            brand.merge(&video),
            "the look of a design gives the design back"
        );
    }
}
