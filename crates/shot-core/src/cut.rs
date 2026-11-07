use std::collections::HashSet;

use kdl::KdlValue;

use crate::kdl_io::{FromKdl, Node, Source, ToKdl, Writer, keyword_enum};
use crate::{CaptionStyle, Id, Layout, ParseError, RuleKind, TimeRange};

/// `cut.kdl`: the edit. Everything is addressed by take id and the take's media time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cut {
    /// Overrides the format's caption style.
    pub captions: Option<CaptionStyle>,
    /// The main cut, in output order.
    pub chapters: Vec<Chapter>,
    pub derived: Vec<DerivedCut>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub title: String,
    /// Takes in output order. A take may appear more than once.
    pub takes: Vec<TakeEdit>,
}

/// The edit of one take within a chapter. All ranges are in that take's media time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeEdit {
    pub take: Id,
    /// Kept and removed ranges, in order and not overlapping.
    pub ranges: Vec<RangeEdit>,
    /// Layout changes, in order and not overlapping. Elsewhere the format's default applies.
    pub layouts: Vec<LayoutChange>,
    /// Punch-in zooms, in order and not overlapping.
    pub zooms: Vec<Zoom>,
    /// Overlays in drawing order: later ones draw on top.
    pub overlays: Vec<Overlay>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeEdit {
    Keep(TimeRange),
    Remove { range: TimeRange, by: RemovedBy },
}

impl RangeEdit {
    pub fn range(self) -> TimeRange {
        match self {
            Self::Keep(range) | Self::Remove { range, .. } => range,
        }
    }
}

/// Who removed a range: an auto-cut rule or a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemovedBy {
    Rule(RuleKind),
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutChange {
    pub layout: Layout,
    pub range: TimeRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Zoom {
    /// Scale in percent, at least 100.
    pub scale: u16,
    pub range: TimeRange,
}

/// A typed overlay template (link card, highlight, b-roll, ...) with its parameters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Overlay {
    pub template: Id,
    pub range: TimeRange,
    /// Parameters in file order, names unique.
    pub params: Vec<(Id, ParamValue)>,
}

/// An overlay parameter. Floats are left out so every value round-trips exactly; use integers
/// in a stated unit (percent, pixels, microseconds) instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParamValue {
    Text(String),
    Integer(i64),
    Bool(bool),
}

keyword_enum! {
    /// A derived cut's kind, which fixes its aspect ratio.
    pub enum DerivedKind {
        /// 9:16, reframed on the face.
        Short = "short",
        /// 1:1.
        XClip = "x-clip",
    }
}

/// A short or X clip: a selection of the main cut's kept material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedCut {
    pub kind: DerivedKind,
    pub name: Id,
    /// Overrides the main cut's caption style. Derived cuts burn captions in.
    pub captions: Option<CaptionStyle>,
    /// Source ranges in output order. Only material the main cut keeps is used.
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub take: Id,
    pub range: TimeRange,
}

impl Cut {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let src = Source {
            file: "cut.kdl",
            text,
        };
        let doc = src.parse()?;
        let mut cut = Self::default();
        let mut captions = None;
        let mut derived_clips = Vec::new();
        for node in src.nodes(&doc) {
            match node.name() {
                "captions" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut captions, node.arg(0)?)?;
                }
                "chapter" => cut.chapters.push(Chapter::read(node)?),
                name @ ("short" | "x-clip") => {
                    let kind = if name == "short" {
                        DerivedKind::Short
                    } else {
                        DerivedKind::XClip
                    };
                    let derived = DerivedCut::read(node, kind, &mut derived_clips)?;
                    if cut.derived.iter().any(|d| d.name == derived.name) {
                        return Err(node.error(format!("duplicate derived cut `{}`", derived.name)));
                    }
                    cut.derived.push(derived);
                }
                _ => return Err(node.unknown()),
            }
        }
        cut.captions = captions;
        let takes: HashSet<&Id> = cut
            .chapters
            .iter()
            .flat_map(|c| &c.takes)
            .map(|t| &t.take)
            .collect();
        if let Some((take, node)) = derived_clips
            .into_iter()
            .find(|(take, _)| !takes.contains(take))
        {
            return Err(node.error(format!("take `{take}` is not in the main cut")));
        }
        Ok(cut)
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        let mut first = true;
        let mut separate = |w: &mut Writer| {
            if !std::mem::take(&mut first) {
                w.blank();
            }
        };
        if let Some(captions) = &self.captions {
            separate(&mut w);
            w.node("captions").arg(captions).end();
        }
        for chapter in &self.chapters {
            separate(&mut w);
            chapter.write(&mut w);
        }
        for derived in &self.derived {
            separate(&mut w);
            derived.write(&mut w);
        }
        w.finish()
    }
}

impl Chapter {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_block(1, &[])?;
        let takes = node
            .children()
            .map(|child| match child.name() {
                "take" => TakeEdit::read(child),
                _ => Err(child.unknown()),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            title: node.arg(0)?,
            takes,
        })
    }

    fn write(&self, w: &mut Writer) {
        w.node("chapter").arg(&self.title).open();
        for take in &self.takes {
            take.write(w);
        }
        w.close();
    }
}

impl TakeEdit {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_block(1, &[])?;
        let mut edit = Self {
            take: node.arg(0)?,
            ranges: Vec::new(),
            layouts: Vec::new(),
            zooms: Vec::new(),
            overlays: Vec::new(),
        };
        let (mut ranges, mut layouts, mut zooms) = (Vec::new(), Vec::new(), Vec::new());
        for child in node.children() {
            match child.name() {
                "keep" => {
                    child.expect(0, &["from", "to"])?;
                    let range = TimeRange::read(child)?;
                    ranges.push((range, child));
                    edit.ranges.push(RangeEdit::Keep(range));
                }
                "remove" => {
                    child.expect(0, &["from", "to", "by"])?;
                    let range = TimeRange::read(child)?;
                    ranges.push((range, child));
                    edit.ranges.push(RangeEdit::Remove {
                        range,
                        by: child.prop("by")?,
                    });
                }
                "layout" => {
                    child.expect(1, &["from", "to"])?;
                    let range = TimeRange::read(child)?;
                    layouts.push((range, child));
                    edit.layouts.push(LayoutChange {
                        layout: child.arg(0)?,
                        range,
                    });
                }
                "zoom" => {
                    child.expect(1, &["from", "to"])?;
                    let scale: u16 = child.arg(0)?;
                    if scale < 100 {
                        return Err(child.error("zoom scale is a percentage of at least 100"));
                    }
                    let range = TimeRange::read(child)?;
                    zooms.push((range, child));
                    edit.zooms.push(Zoom { scale, range });
                }
                "overlay" => edit.overlays.push(Overlay::read(child)?),
                _ => return Err(child.unknown()),
            }
        }
        TimeRange::check_ordered(ranges)?;
        TimeRange::check_ordered(layouts)?;
        TimeRange::check_ordered(zooms)?;
        Ok(edit)
    }

    fn write(&self, w: &mut Writer) {
        w.node("take").arg(&self.take).open();
        for edit in &self.ranges {
            match edit {
                RangeEdit::Keep(range) => write_range(w.node("keep"), *range).end(),
                RangeEdit::Remove { range, by } => {
                    write_range(w.node("remove"), *range).prop("by", by).end()
                }
            }
        }
        for change in &self.layouts {
            write_range(w.node("layout").arg(&change.layout), change.range).end();
        }
        for zoom in &self.zooms {
            write_range(w.node("zoom").arg(&zoom.scale), zoom.range).end();
        }
        for overlay in &self.overlays {
            overlay.write(w);
        }
        w.close();
    }
}

impl Overlay {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_block(1, &["from", "to"])?;
        let mut params: Vec<(Id, ParamValue)> = Vec::new();
        for param in node.children() {
            param.expect(1, &[])?;
            let name =
                Id::new(param.name()).ok_or_else(|| param.error("parameter names are ids"))?;
            if params.iter().any(|(n, _)| *n == name) {
                return Err(param.error(format!("duplicate parameter `{name}`")));
            }
            params.push((name, param.arg(0)?));
        }
        Ok(Self {
            template: node.arg(0)?,
            range: TimeRange::read(node)?,
            params,
        })
    }

    fn write(&self, w: &mut Writer) {
        write_range(w.node("overlay").arg(&self.template), self.range);
        if self.params.is_empty() {
            w.end();
            return;
        }
        w.open();
        for (name, value) in &self.params {
            w.node(name.as_str()).arg(value).end();
        }
        w.close();
    }
}

impl DerivedCut {
    fn read<'a>(
        node: Node<'a>,
        kind: DerivedKind,
        clips_seen: &mut Vec<(Id, Node<'a>)>,
    ) -> Result<Self, ParseError> {
        node.expect_block(1, &[])?;
        let mut captions = None;
        let mut clips = Vec::new();
        for child in node.children() {
            match child.name() {
                "captions" => {
                    child.expect(1, &[])?;
                    child.set_once(&mut captions, child.arg(0)?)?;
                }
                "clip" => {
                    child.expect(1, &["from", "to"])?;
                    let clip = Clip {
                        take: child.arg(0)?,
                        range: TimeRange::read(child)?,
                    };
                    clips_seen.push((clip.take.clone(), child));
                    clips.push(clip);
                }
                _ => return Err(child.unknown()),
            }
        }
        if clips.is_empty() {
            return Err(node.error(format!("`{}` needs at least one `clip`", node.name())));
        }
        Ok(Self {
            kind,
            name: node.arg(0)?,
            captions,
            clips,
        })
    }

    fn write(&self, w: &mut Writer) {
        w.node(self.kind.as_str()).arg(&self.name).open();
        if let Some(captions) = &self.captions {
            w.node("captions").arg(captions).end();
        }
        for clip in &self.clips {
            write_range(w.node("clip").arg(&clip.take), clip.range).end();
        }
        w.close();
    }
}

fn write_range(w: &mut Writer, range: TimeRange) -> &mut Writer {
    w.prop("from", &range.from()).prop("to", &range.to())
}

impl FromKdl for RemovedBy {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        match value.as_string() {
            Some("manual") => Ok(Self::Manual),
            _ => RuleKind::from_kdl(value)
                .map(Self::Rule)
                .map_err(|e| format!("{e}, or manual")),
        }
    }
}

impl ToKdl for RemovedBy {
    fn write_kdl(&self, out: &mut String) {
        out.push_str(match self {
            Self::Rule(rule) => rule.as_str(),
            Self::Manual => "manual",
        });
    }
}

impl FromKdl for ParamValue {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        match value {
            KdlValue::String(s) => Ok(Self::Text(s.clone())),
            KdlValue::Integer(n) => i64::try_from(*n)
                .map(Self::Integer)
                .map_err(|_| format!("{n} is out of range")),
            KdlValue::Bool(b) => Ok(Self::Bool(*b)),
            KdlValue::Float(_) => {
                Err("floats are not supported, use an integer in a stated unit".to_owned())
            }
            KdlValue::Null => Err("#null is not a parameter value".to_owned()),
        }
    }
}

impl ToKdl for ParamValue {
    fn write_kdl(&self, out: &mut String) {
        match self {
            Self::Text(s) => s.write_kdl(out),
            Self::Integer(n) => n.write_kdl(out),
            Self::Bool(b) => b.write_kdl(out),
        }
    }
}
