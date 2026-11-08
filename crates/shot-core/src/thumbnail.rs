//! `thumbnail.kdl`: the video's thumbnail designs. Each variant is a frame of the cut, the face
//! cut out of it or framed, a headline and stickers on a 1280x720 canvas; the one marked
//! `upload` becomes the `main.png` Ship uploads.

use crate::kdl_io::{FromKdl, Node, Source, Writer, keyword_enum};
use crate::{Id, Micros, ParseError};

/// The canvas the coordinates of a design are in: YouTube's thumbnail size.
pub const CANVAS: (u16, u16) = (1280, 720);
/// The most lines a headline has.
pub const HEADLINE_LINES: usize = 2;

keyword_enum! {
    /// Where the parts of a variant go.
    pub enum ThumbnailPreset {
        /// The face cut out on the right, the headline on the left.
        FaceRight = "face-right",
        FaceLeft = "face-left",
        /// The camera frame fills the canvas, the headline across the bottom.
        BigFace = "big-face",
        /// The screen fills the canvas, the headline on top, the face small in a corner.
        Screen = "screen",
    }
}

keyword_enum! {
    /// What fills the canvas behind the face.
    pub enum Backdrop {
        Screen = "screen",
        Camera = "camera",
        /// The brand's background color.
        Brand = "brand",
    }
}

keyword_enum! {
    /// How the face shows.
    pub enum FaceStyle {
        /// Cut out of its background with an outline.
        Cutout = "cutout",
        /// The camera frame in a rounded box with an outline.
        Framed = "framed",
        Off = "off",
    }
}

keyword_enum! {
    /// The brand color of the headline.
    pub enum HeadlineColor {
        Text = "text",
        Accent = "accent",
        /// The first line in the text color, the second in the accent.
        Both = "both",
    }
}

keyword_enum! {
    /// What sets the headline off from the picture behind it.
    pub enum HeadlineStyle {
        Outline = "outline",
        Shadow = "shadow",
    }
}

keyword_enum! {
    pub enum StickerShape {
        Arrow = "arrow",
        Ring = "ring",
        Emoji = "emoji",
    }
}

/// All the variants of a video's thumbnail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThumbnailDesign {
    /// The variant Ship uploads. Names one of `variants` when there are any.
    pub upload: Option<Id>,
    pub variants: Vec<Variant>,
}

/// One thumbnail to compare with the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: Id,
    pub preset: ThumbnailPreset,
    /// The moment the background and the face come from. `None` until one is picked.
    pub frame: Option<FramePick>,
    pub backdrop: Backdrop,
    /// Percent the backdrop is darkened by, 0 to [`MAX_DIM`].
    pub dim: u8,
    pub face: FaceStyle,
    /// The face's height in percent of the preset's, 40 to 200.
    pub face_size: u16,
    pub headline: Option<Headline>,
    pub stickers: Vec<Sticker>,
}

/// A moment of a take: its media time, as `cut.kdl` addresses recordings.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FramePick {
    pub take: Id,
    pub at: Micros,
}

/// One or two lines, auto-fitted to the preset's headline box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headline {
    pub lines: Vec<String>,
    pub color: HeadlineColor,
    pub style: HeadlineStyle,
}

/// An arrow, a ring or a bundled emoji, centered on `x`, `y` in canvas pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sticker {
    pub shape: StickerShape,
    /// The emoji, for [`StickerShape::Emoji`].
    pub emoji: Option<String>,
    pub x: i16,
    pub y: i16,
    /// Height in canvas pixels.
    pub size: u16,
    /// Clockwise rotation in degrees.
    pub turn: i16,
}

pub const MAX_DIM: u8 = 90;
pub const FACE_SIZE: std::ops::RangeInclusive<u16> = 40..=200;
pub const STICKER_SIZE: std::ops::RangeInclusive<u16> = 24..=720;

impl Variant {
    /// A variant with the preset's backdrop and face, no frame, headline or stickers.
    pub fn new(name: Id, preset: ThumbnailPreset) -> Self {
        let mut variant = Self {
            name,
            preset,
            frame: None,
            backdrop: Backdrop::Screen,
            dim: 0,
            face: FaceStyle::Cutout,
            face_size: 100,
            headline: None,
            stickers: Vec::new(),
        };
        variant.apply(preset);
        variant
    }

    /// Switches to `preset` with its backdrop and face, keeping the frame, text and stickers.
    pub fn apply(&mut self, preset: ThumbnailPreset) {
        self.preset = preset;
        (self.backdrop, self.dim, self.face) = match preset {
            ThumbnailPreset::FaceRight | ThumbnailPreset::FaceLeft => {
                (Backdrop::Screen, 45, FaceStyle::Cutout)
            }
            ThumbnailPreset::BigFace => (Backdrop::Camera, 10, FaceStyle::Off),
            ThumbnailPreset::Screen => (Backdrop::Screen, 20, FaceStyle::Framed),
        };
        self.face_size = 100;
    }
}

impl ThumbnailDesign {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let src = Source {
            file: "thumbnail.kdl",
            text,
        };
        let doc = src.parse()?;
        let mut design = Self::default();
        let mut upload: Option<(Id, Node<'_>)> = None;
        for node in src.nodes(&doc) {
            match node.name() {
                "upload" => {
                    node.expect(1, &[])?;
                    if upload.replace((node.arg(0)?, node)).is_some() {
                        return Err(node.error("duplicate `upload`"));
                    }
                }
                "variant" => {
                    let variant = Variant::read(node)?;
                    if design.variants.iter().any(|v| v.name == variant.name) {
                        return Err(node.error(format!("duplicate variant `{}`", variant.name)));
                    }
                    design.variants.push(variant);
                }
                _ => return Err(node.unknown()),
            }
        }
        match upload {
            Some((name, node)) if !design.variants.iter().any(|v| v.name == name) => {
                return Err(node.error(format!("`upload` names no variant `{name}`")));
            }
            Some((name, _)) => design.upload = Some(name),
            None if !design.variants.is_empty() => {
                return Err(src.error(0, "missing `upload`"));
            }
            None => {}
        }
        Ok(design)
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        if let Some(upload) = &self.upload {
            w.node("upload").arg(upload).end();
        }
        for variant in &self.variants {
            variant.write(&mut w);
        }
        w.finish()
    }

    /// The variant Ship uploads.
    pub fn uploaded(&self) -> Option<&Variant> {
        let name = self.upload.as_ref()?;
        self.variants.iter().find(|v| &v.name == name)
    }

    /// The first free variant name: `a`, `b`, ... `z`, then `v27` and up.
    pub fn free_name(&self) -> Id {
        let taken = |name: &str| self.variants.iter().any(|v| v.name.as_str() == name);
        ('a'..='z')
            .map(String::from)
            .chain((27..).map(|n| format!("v{n}")))
            .find(|name| !taken(name))
            .and_then(Id::new)
            .expect("letters are valid ids")
    }
}

impl Variant {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_block(1, &["preset"])?;
        let mut variant = Self::new(node.arg(0)?, node.prop("preset")?);
        let (mut frame, mut backdrop, mut face, mut headline) = (None, None, None, None);
        for child in node.children() {
            match child.name() {
                "frame" => {
                    child.expect(1, &["at"])?;
                    let pick = FramePick {
                        take: child.arg(0)?,
                        at: child.prop("at")?,
                    };
                    child.set_once(&mut frame, pick)?;
                }
                "background" => {
                    child.expect(1, &["dim"])?;
                    let dim: u8 = child.prop("dim")?;
                    if dim > MAX_DIM {
                        return Err(child.error(format!("`dim` is at most {MAX_DIM}")));
                    }
                    child.set_once(&mut backdrop, (child.arg(0)?, dim))?;
                }
                "face" => {
                    child.expect(1, &["size"])?;
                    let size: u16 = child.prop("size")?;
                    if !FACE_SIZE.contains(&size) {
                        return Err(child.error("`size` is 40 to 200 percent"));
                    }
                    child.set_once(&mut face, (child.arg(0)?, size))?;
                }
                "headline" => {
                    child.expect_any(&["color", "style"])?;
                    let lines: Vec<String> = child.args()?;
                    if lines.is_empty()
                        || lines.len() > HEADLINE_LINES
                        || lines.iter().any(|l| l.trim().is_empty())
                    {
                        return Err(child.error("a headline is one or two lines of text"));
                    }
                    let read = Headline {
                        lines,
                        color: child.prop("color")?,
                        style: child.prop("style")?,
                    };
                    child.set_once(&mut headline, read)?;
                }
                "sticker" => variant.stickers.push(Sticker::read(child)?),
                _ => return Err(child.unknown()),
            }
        }
        variant.frame = frame;
        if let Some((kind, dim)) = backdrop {
            (variant.backdrop, variant.dim) = (kind, dim);
        }
        if let Some((style, size)) = face {
            (variant.face, variant.face_size) = (style, size);
        }
        variant.headline = headline;
        Ok(variant)
    }

    fn write(&self, w: &mut Writer) {
        w.node("variant")
            .arg(&self.name)
            .prop("preset", &self.preset)
            .open();
        if let Some(frame) = &self.frame {
            w.node("frame").arg(&frame.take).prop("at", &frame.at).end();
        }
        w.node("background")
            .arg(&self.backdrop)
            .prop("dim", &self.dim)
            .end();
        w.node("face")
            .arg(&self.face)
            .prop("size", &self.face_size)
            .end();
        if let Some(headline) = &self.headline {
            w.node("headline");
            for line in &headline.lines {
                w.arg(line);
            }
            w.prop("color", &headline.color)
                .prop("style", &headline.style)
                .end();
        }
        for sticker in &self.stickers {
            w.node("sticker").arg(&sticker.shape);
            if let Some(emoji) = &sticker.emoji {
                w.arg(emoji);
            }
            w.prop("x", &sticker.x)
                .prop("y", &sticker.y)
                .prop("size", &sticker.size)
                .prop("turn", &sticker.turn)
                .end();
        }
        w.close();
    }
}

impl Sticker {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_any(&["x", "y", "size", "turn"])?;
        let shape: StickerShape = node.arg(0)?;
        let rest: Vec<String> = node.args::<kdl::KdlValue>()?[1..]
            .iter()
            .map(|v| String::from_kdl(v).map_err(|e| node.error(e)))
            .collect::<Result<_, _>>()?;
        let emoji = match (shape, rest.as_slice()) {
            (StickerShape::Emoji, [emoji]) => Some(emoji.clone()),
            (StickerShape::Emoji, _) => return Err(node.error("an emoji sticker names its emoji")),
            (_, []) => None,
            (_, _) => return Err(node.error("only an emoji sticker has a second argument")),
        };
        let size: u16 = node.prop("size")?;
        if !STICKER_SIZE.contains(&size) {
            return Err(node.error("a sticker is 24 to 720 pixels tall"));
        }
        Ok(Self {
            shape,
            emoji,
            x: node.prop("x")?,
            y: node.prop("y")?,
            size,
            turn: node.prop("turn")?,
        })
    }
}
