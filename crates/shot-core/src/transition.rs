//! One transition type for chapter edges and layout switches ([0036]): `<kind> length=<time>
//! sound=<sound>`, written on a chapter rule's `transition` effect, a `layout` range's `in` and
//! `out` and the format's `layout-switch`.
//!
//! Where the window sits is a property of the edge, not of the transition: centered on a chapter
//! edge, starting on a layout edge. A chapter edge without `sound` plays the brand's
//! `transition` keys, a layout switch without one plays nothing.
//!
//! [0036]: ../../../docs/decisions/0036-one-transition-type.md

use kdl::KdlValue;

use crate::kdl_io::{FromKdl, Node, ToKdl, Writer, keyword_enum};
use crate::{Id, Micros, ParseError};

keyword_enum! {
    /// How a transition draws.
    pub enum TransitionKind {
        Cut = "cut",
        Dissolve = "dissolve",
        /// Through the brand background.
        Dip = "dip",
        /// The brand's Lottie stinger, covering the cut at its peak.
        Stinger = "stinger",
        MaskWipe = "mask-wipe",
        /// The old side punches in, the new one punches out, motion blurred.
        ZoomThrough = "zoom-through",
        /// Every slot springs from its place in the old layout to its place in the new one.
        /// Needs the same take on both sides, so only on a layout edge.
        Morph = "morph",
    }
}

impl TransitionKind {
    /// Whether a chapter edge can draw it: every kind but a morph.
    pub fn joins_chapters(self) -> bool {
        self != Self::Morph
    }
}

/// What a transition plays: the brand's `transition` sounds by phase, or one sound by name at
/// the window's start.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TransitionSound {
    Brand,
    Named(Id),
}

impl FromKdl for TransitionSound {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let id = Id::from_kdl(value)?;
        Ok(if id.as_str() == "brand" {
            Self::Brand
        } else {
            Self::Named(id)
        })
    }
}

impl ToKdl for TransitionSound {
    fn write_kdl(&self, out: &mut String) {
        match self {
            Self::Brand => out.push_str("brand"),
            Self::Named(id) => id.write_kdl(out),
        }
    }
}

/// How two pictures join, on a chapter edge or a layout switch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Transition {
    pub kind: TransitionKind,
    pub length: Micros,
    pub sound: Option<TransitionSound>,
}

impl Transition {
    /// The length when none is written.
    pub const LENGTH: Micros = Micros(500_000);

    /// Every layout switch without a written one, unless the format sets another default.
    pub const SWITCH: Self = Self::new(TransitionKind::Morph);

    /// `kind` with the default length and no sound.
    pub const fn new(kind: TransitionKind) -> Self {
        Self {
            kind,
            length: Self::LENGTH,
            sound: None,
        }
    }

    /// Reads `<kind> length=<time> sound=<sound>`; `props` are the node's allowed properties.
    pub(crate) fn read(node: Node<'_>, props: &[&str]) -> Result<Self, ParseError> {
        node.expect(1, props)?;
        let length = node.opt_prop("length")?.unwrap_or(Self::LENGTH);
        if length == Micros(0) {
            return Err(node.error("`length` is longer than 0"));
        }
        Ok(Self {
            kind: node.arg(0)?,
            length,
            sound: node.opt_prop("sound")?,
        })
    }

    /// Writes the arguments and properties after the node's name, defaults left out.
    pub(crate) fn write(&self, w: &mut Writer) {
        w.arg(&self.kind);
        if self.length != Self::LENGTH {
            w.prop("length", &self.length);
        }
        if let Some(sound) = &self.sound {
            w.prop("sound", sound);
        }
        w.end();
    }
}
