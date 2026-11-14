use std::collections::HashSet;

use kdl::KdlValue;

use crate::kdl_io::{Bare, FromKdl, Node, Source, ToKdl, Writer, keyword_enum};
use crate::migrate::old::{self, OldBlock, Review, refused};
use crate::{
    CaptionStyle, Decision, Effect, EffectKey, ElementKind, Id, Layout, Micros, ParseError, Target,
    TimeRange, Transition,
};

/// `cut.kdl`: the edit. Everything is addressed by take id and the take's media time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cut {
    /// Overrides the format's caption style.
    pub captions: Option<CaptionStyle>,
    /// Transcript corrections for captions, in file order. `heard` is unique ignoring case.
    pub caption_fixes: Vec<CaptionFix>,
    /// Where caption lines break other than the automatic grouping, sorted by take and `at`,
    /// each (take, `at`) once.
    pub caption_breaks: Vec<CaptionBreak>,
    /// The music bed under the whole cut and its derived cuts.
    pub music: Option<Music>,
    /// Ratings and picks of the Takes view, by take (`t9` before `t10`), then `from`. The
    /// ranges of one take do not overlap.
    pub ratings: Vec<Rating>,
    /// The main cut, in output order.
    pub chapters: Vec<Chapter>,
    /// Changes to what the style rules gave single elements, in canonical order: by take in
    /// cut order, then `at`, then kind. Each (take, kind, `at`) once.
    pub overrides: Vec<Override>,
    pub derived: Vec<DerivedCut>,
}

/// An `override` node: what one element gets instead of what the rules gave it. It names the
/// element by take, kind and start time, the element's identity ([0019]). The build binds it
/// to the nearest element of that kind within [`Override::TOLERANCE`], so the override follows
/// a re-analysis that shifts the element a little, and `shot check` lists one that lost it.
///
/// [0019]: ../../../docs/decisions/0019-semantic-video-elements-and-style.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub take: Id,
    pub kind: ElementKind,
    pub at: Micros,
    /// `keep` or `cut`: the answer to the review queue. `keep` drops the rule's cut and the
    /// review item, `cut` applies the rule's cut whatever the confidence.
    pub decision: Option<Decision>,
    /// In canonical key order, one per key.
    pub effects: Vec<OverrideEffect>,
}

impl Override {
    /// How far an override's `at` may be from its element's `t`.
    pub const TOLERANCE: Micros = Micros(500_000);
}

/// One effect of an override: a replacement for the rules' effect of that key, or `off`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideEffect {
    Set(Effect),
    Off(EffectKey),
}

impl OverrideEffect {
    pub fn key(&self) -> EffectKey {
        match self {
            Self::Set(effect) => effect.key(),
            Self::Off(key) => key.clone(),
        }
    }
}

/// Replaces every run of transcript words that reads `heard` (ignoring case) with `said`
/// wherever captions are shown. The replacement takes the matched words' timing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CaptionFix {
    pub heard: String,
    pub said: String,
}

/// A person's verdict on one attempt of the Takes view: an `attempt` node of the `takes` block.
/// It names the attempt by its take and range, and binds to the attempt of that take that holds
/// the middle of its range ([`Cut::rating`]), so it follows a re-alignment that moves the
/// attempt's edges a little.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Rating {
    pub take: Id,
    /// The attempt, or the part of it a person marked and circled.
    pub range: TimeRange,
    /// 1 to 5.
    pub stars: Option<u8>,
    /// Circled: the attempt, or part, that "Assemble picks" uses for its beat.
    pub pick: bool,
}

impl Rating {
    /// The canonical order: by take, shorter ids first so `t9` comes before `t10`, then `from`.
    fn order(&self) -> (usize, &str, Micros) {
        (
            self.take.as_str().len(),
            self.take.as_str(),
            self.range.from(),
        )
    }
}

/// A caption line break a person set: `split` starts a new line at the word of `take` that
/// starts at `at`, `join` keeps that word on the line before it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CaptionBreak {
    pub take: Id,
    pub at: Micros,
    pub split: bool,
}

/// A music bed: a file relative to the video folder, its gain in dB (at most 0) and the gain
/// it drops to under speech, also in dB at most 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Music {
    pub file: String,
    pub gain: i16,
    pub duck: i16,
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

/// A point of the screen track in pixels, origin top left, like the event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeEdit {
    Keep(TimeRange),
    /// A person's cut, written `remove ... by=manual`.
    Remove(TimeRange),
}

impl RangeEdit {
    pub fn range(self) -> TimeRange {
        match self {
            Self::Keep(range) | Self::Remove(range) => range,
        }
    }

    /// The same edit over `range`.
    pub fn with_range(self, range: TimeRange) -> Self {
        match self {
            Self::Keep(_) => Self::Keep(range),
            Self::Remove(_) => Self::Remove(range),
        }
    }
}

/// A layout over a range of a take, with how the switches on its edges move when they differ
/// from the format's `layout-switch`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayoutChange {
    pub layout: Layout,
    pub range: TimeRange,
    /// The switch into this layout at `from`, written as `in`.
    pub enter: Option<Transition>,
    /// The switch out of it at `to`, written as `out`. A change starting at `to` has its own
    /// `in`, which wins.
    pub exit: Option<Transition>,
}

impl LayoutChange {
    /// A change with the format's switches on both edges.
    pub fn new(layout: Layout, range: TimeRange) -> Self {
        Self {
            layout,
            range,
            enter: None,
            exit: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Zoom {
    /// Scale in percent, at least 100.
    pub scale: u16,
    pub range: TimeRange,
    /// Where the zoom centers on the screen track. `None` centers as the layout does.
    pub center: Option<Point>,
    /// How it frames the picture and ramps in and out.
    pub shape: ZoomShape,
}

keyword_enum! {
    /// What a zoom frames: `auto` its point or the layout's center, the center, or a quarter
    /// of the screen. A quarter fills the frame, so it zooms to at least 200 percent.
    #[derive(Default)]
    pub enum Framing {
        #[default]
        Auto = "auto",
        Center = "center",
        TopLeft = "top-left",
        TopRight = "top-right",
        BottomLeft = "bottom-left",
        BottomRight = "bottom-right",
    }
}

impl Framing {
    /// The quarter's center as a fraction of the screen, `None` for `auto` and `center`.
    pub fn quarter(self) -> Option<[f32; 2]> {
        match self {
            Self::Auto | Self::Center => None,
            Self::TopLeft => Some([0.25, 0.25]),
            Self::TopRight => Some([0.75, 0.25]),
            Self::BottomLeft => Some([0.25, 0.75]),
            Self::BottomRight => Some([0.75, 0.75]),
        }
    }

    /// The lowest scale in percent the framing allows.
    pub fn min_scale(self) -> u16 {
        if self.quarter().is_some() { 200 } else { 100 }
    }
}

/// How a zoom frames and moves, on the `zoom` node and the `zoom` effect alike. Each part is
/// written only when it differs from the default: `auto` framing and the ramps `zoom-motion`
/// gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ZoomShape {
    pub framing: Framing,
    /// How long the zoom eases in, 0 for a hard switch. `None` follows `zoom-motion`.
    pub ramp_in: Option<Micros>,
    /// How long it eases out, 0 for a hard switch. `None` follows `zoom-motion`.
    pub ramp_out: Option<Micros>,
}

impl ZoomShape {
    /// Whether a ramp of its own eases the zoom in or out rather than switching.
    pub fn ramps_gradually(&self) -> bool {
        [self.ramp_in, self.ramp_out]
            .iter()
            .any(|r| r.is_some_and(|r| r.0 > 0))
    }

    /// The properties [`ZoomShape::read`] takes.
    pub(crate) const PROPS: [&str; 3] = ["framing", "ramp-in", "ramp-out"];

    pub(crate) fn read(node: Node<'_>) -> Result<Self, ParseError> {
        Ok(Self {
            framing: node.opt_prop("framing")?.unwrap_or_default(),
            ramp_in: node.opt_prop("ramp-in")?,
            ramp_out: node.opt_prop("ramp-out")?,
        })
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        if self.framing != Framing::Auto {
            w.prop("framing", &self.framing);
        }
        if let Some(ramp) = &self.ramp_in {
            w.prop("ramp-in", ramp);
        }
        if let Some(ramp) = &self.ramp_out {
            w.prop("ramp-out", ramp);
        }
    }
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
        Self::read(text, None)
    }

    /// Reads a `cut.kdl` written before style rules: the blocks play what the old file showed,
    /// and what the rules wrote into each block is apart, one [`OldBlock`] per block in order.
    pub(crate) fn parse_old(text: &str) -> Result<(Self, Vec<OldBlock>), ParseError> {
        let mut old = Vec::new();
        let cut = Self::read(text, Some(&mut old))?;
        Ok((cut, old))
    }

    fn read(text: &str, mut old: Option<&mut Vec<OldBlock>>) -> Result<Self, ParseError> {
        let src = Source {
            file: "cut.kdl",
            text,
        };
        let doc = src.parse()?;
        let mut cut = Self::default();
        let (mut captions, mut fixes, mut music) = (None, None, None);
        let mut ratings = None;
        let mut breaks = Vec::new();
        let mut derived_clips = Vec::new();
        let mut overrides = Vec::new();
        for node in src.versioned(&doc)? {
            match node.name() {
                "captions" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut captions, node.arg(0)?)?;
                }
                "caption-fixes" => {
                    let (read, read_breaks) = CaptionFix::read_all(node)?;
                    node.set_once(&mut fixes, read)?;
                    breaks = read_breaks;
                }
                "music" => node.set_once(&mut music, Music::read(node)?)?,
                "takes" => node.set_once(&mut ratings, Rating::read_all(node)?)?,
                "chapter" => cut.chapters.push(Chapter::read(node, old.as_deref_mut())?),
                "override" => overrides.push((Override::read(node)?, node)),
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
        cut.caption_fixes = fixes.unwrap_or_default();
        cut.caption_breaks = breaks;
        cut.music = music;
        cut.ratings = ratings.unwrap_or_default();
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
        let take_order: Vec<&Id> = {
            let mut order = Vec::new();
            for take in cut.chapters.iter().flat_map(|c| &c.takes) {
                if !order.contains(&&take.take) {
                    order.push(&take.take);
                }
            }
            order
        };
        for (o, node) in &overrides {
            if !takes.contains(&o.take) {
                return Err(node.error(format!("take `{}` is not in the main cut", o.take)));
            }
            let same =
                |other: &Override| other.take == o.take && other.kind == o.kind && other.at == o.at;
            if overrides.iter().filter(|(other, _)| same(other)).count() > 1 {
                return Err(node.error(format!(
                    "duplicate override of `{}` at {} in {}",
                    o.kind, o.at.0, o.take
                )));
            }
        }
        let position = |o: &Override| {
            let take = take_order.iter().position(|t| **t == o.take);
            (take, o.at, o.kind)
        };
        overrides.sort_by_key(|(o, _)| position(o));
        cut.overrides = overrides.into_iter().map(|(o, _)| o).collect();
        Ok(cut)
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        w.version();
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
        if !self.caption_fixes.is_empty() || !self.caption_breaks.is_empty() {
            separate(&mut w);
            w.node("caption-fixes").open();
            for fix in &self.caption_fixes {
                w.node("fix").arg(&fix.heard).arg(&fix.said).end();
            }
            for b in &self.caption_breaks {
                w.node(if b.split { "split" } else { "join" })
                    .arg(&b.take)
                    .prop("at", &b.at)
                    .end();
            }
            w.close();
        }
        if let Some(music) = &self.music {
            separate(&mut w);
            w.node("music")
                .arg(&music.file)
                .prop("gain", &music.gain)
                .prop("duck", &music.duck)
                .end();
        }
        if !self.ratings.is_empty() {
            separate(&mut w);
            w.node("takes").open();
            for rating in &self.ratings {
                write_range(w.node("attempt").arg(&rating.take), rating.range);
                if let Some(stars) = &rating.stars {
                    w.prop("stars", stars);
                }
                if rating.pick {
                    w.prop("pick", &true);
                }
                w.end();
            }
            w.close();
        }
        for chapter in &self.chapters {
            separate(&mut w);
            chapter.write(&mut w);
        }
        if !self.overrides.is_empty() {
            separate(&mut w);
            for o in &self.overrides {
                o.write(&mut w);
            }
        }
        for derived in &self.derived {
            separate(&mut w);
            derived.write(&mut w);
        }
        w.finish()
    }
}

impl Chapter {
    fn read(node: Node<'_>, mut old: Option<&mut Vec<OldBlock>>) -> Result<Self, ParseError> {
        node.expect_block(1, &[])?;
        let takes = node
            .children()
            .map(|child| match child.name() {
                "take" => match old.as_deref_mut() {
                    Some(blocks) => {
                        let mut block = OldBlock::default();
                        let edit = TakeEdit::read(child, Some(&mut block))?;
                        blocks.push(block);
                        Ok(edit)
                    }
                    None => TakeEdit::read(child, None),
                },
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
    /// Reads a take block. What the rules wrote before the lock goes to `old`, and without it
    /// is refused.
    fn read(node: Node<'_>, mut old: Option<&mut OldBlock>) -> Result<Self, ParseError> {
        node.expect_block(1, &[])?;
        let mut edit = Self {
            take: node.arg(0)?,
            ranges: Vec::new(),
            layouts: Vec::new(),
            zooms: Vec::new(),
            overlays: Vec::new(),
        };
        let (mut ranges, mut layouts, mut reviews, mut zooms) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
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
                    if let old::RemovedBy::Rule(rule) = child.prop("by")? {
                        old_block(&mut old, child, format!("`remove by={rule}`"))?
                            .removals
                            .push((edit.ranges.len(), rule));
                    }
                    edit.ranges.push(RangeEdit::Remove(range));
                }
                "layout" => {
                    child.expect_block(1, &["from", "to"])?;
                    let range = TimeRange::read(child)?;
                    layouts.push((range, child));
                    let mut change = LayoutChange::new(child.arg(0)?, range);
                    for edge in child.children() {
                        let slot = match edge.name() {
                            "in" => &mut change.enter,
                            "out" => &mut change.exit,
                            _ => return Err(edge.unknown()),
                        };
                        edge.set_once(slot, Transition::read(edge, &["length", "sound"])?)?;
                    }
                    edit.layouts.push(change);
                }
                name @ ("review" | "ignore") => {
                    child.expect(0, &["from", "to", "by"])?;
                    let range = TimeRange::read(child)?;
                    let review = Review {
                        range,
                        by: child.prop("by")?,
                        kept: name == "ignore",
                    };
                    old_block(&mut old, child, format!("`{name}`"))?
                        .reviews
                        .push(review);
                    reviews.push((range, child));
                }
                "zoom" => {
                    let props = ["from", "to", "x", "y", "by"];
                    child.expect(1, &[&props[..], &ZoomShape::PROPS].concat())?;
                    let scale: u16 = child.arg(0)?;
                    if scale < 100 {
                        return Err(child.error("zoom scale is a percentage of at least 100"));
                    }
                    let range = TimeRange::read(child)?;
                    let center = match (child.opt_prop("x")?, child.opt_prop("y")?) {
                        (Some(x), Some(y)) => Some(Point { x, y }),
                        (None, None) => None,
                        _ => return Err(child.error("`x` and `y` come together")),
                    };
                    if let Some(by) = child.opt_prop::<old::ZoomBy>("by")? {
                        old_block(&mut old, child, format!("`zoom by={by}`"))?
                            .zooms
                            .push((edit.zooms.len(), by));
                    }
                    zooms.push((range, child));
                    edit.zooms.push(Zoom {
                        scale,
                        range,
                        center,
                        shape: ZoomShape::read(child)?,
                    });
                }
                "overlay" => edit.overlays.push(Overlay::read(child)?),
                _ => return Err(child.unknown()),
            }
        }
        TimeRange::check_ordered(ranges)?;
        TimeRange::check_ordered(layouts)?;
        TimeRange::check_ordered(reviews.iter().copied())?;
        TimeRange::check_ordered(zooms)?;
        if let Some((_, child)) = reviews.iter().find(|(range, _)| !edit.keeps(*range)) {
            return Err(child.error(format!("`{}` must lie inside kept material", child.name())));
        }
        Ok(edit)
    }

    /// Whether every moment of `range` is kept material of this block.
    pub fn keeps(&self, range: TimeRange) -> bool {
        let mut covered = 0;
        for edit in &self.ranges {
            match edit {
                RangeEdit::Keep(kept) => {
                    covered += kept.intersect(range).map_or(0, |r| r.duration().0);
                }
                RangeEdit::Remove(removed) if removed.overlaps(range) => {
                    return false;
                }
                RangeEdit::Remove(_) => {}
            }
        }
        covered == range.duration().0
    }

    fn write(&self, w: &mut Writer) {
        w.node("take").arg(&self.take).open();
        for edit in &self.ranges {
            match edit {
                RangeEdit::Keep(range) => write_range(w.node("keep"), *range).end(),
                RangeEdit::Remove(range) => write_range(w.node("remove"), *range)
                    .prop("by", &Bare("manual"))
                    .end(),
            }
        }
        for change in &self.layouts {
            write_range(w.node("layout").arg(&change.layout), change.range);
            if change.enter.is_none() && change.exit.is_none() {
                w.end();
                continue;
            }
            w.open();
            for (name, switch) in [("in", &change.enter), ("out", &change.exit)] {
                if let Some(switch) = switch {
                    switch.write(w.node(name));
                }
            }
            w.close();
        }
        for zoom in &self.zooms {
            write_range(w.node("zoom").arg(&zoom.scale), zoom.range);
            if let Some(Point { x, y }) = zoom.center {
                w.prop("x", &x).prop("y", &y);
            }
            zoom.shape.write(w);
            w.end();
        }
        for overlay in &self.overlays {
            overlay.write(w);
        }
        w.close();
    }
}

impl CaptionFix {
    /// The fixes of a `caption-fixes` block, and its line breaks in canonical order.
    fn read_all(node: Node<'_>) -> Result<(Vec<Self>, Vec<CaptionBreak>), ParseError> {
        node.expect_block(0, &[])?;
        let mut fixes: Vec<Self> = Vec::new();
        let mut breaks: Vec<CaptionBreak> = Vec::new();
        for child in node.children() {
            if let name @ ("split" | "join") = child.name() {
                child.expect(1, &["at"])?;
                let b = CaptionBreak {
                    take: child.arg(0)?,
                    at: child.prop("at")?,
                    split: name == "split",
                };
                if breaks.iter().any(|o| o.take == b.take && o.at == b.at) {
                    return Err(
                        child.error(format!("duplicate line break at {} in {}", b.at.0, b.take))
                    );
                }
                breaks.push(b);
                continue;
            }
            if child.name() != "fix" {
                return Err(child.unknown());
            }
            child.expect(2, &[])?;
            let fix = Self {
                heard: child.arg(0)?,
                said: child.arg(1)?,
            };
            if fix.heard.trim().is_empty() || fix.said.trim().is_empty() {
                return Err(child.error("`fix` takes the heard words and the said words"));
            }
            let heard = fix.heard.to_lowercase();
            if fixes.iter().any(|f| f.heard.to_lowercase() == heard) {
                return Err(child.error(format!("duplicate fix for `{}`", fix.heard)));
            }
            fixes.push(fix);
        }
        breaks.sort_by(|a, b| (&a.take, a.at).cmp(&(&b.take, b.at)));
        Ok((fixes, breaks))
    }
}

impl Rating {
    fn read_all(node: Node<'_>) -> Result<Vec<Self>, ParseError> {
        node.expect_block(0, &[])?;
        let mut ratings: Vec<(Self, Node<'_>)> = Vec::new();
        for child in node.children() {
            if child.name() != "attempt" {
                return Err(child.unknown());
            }
            child.expect(1, &["from", "to", "stars", "pick"])?;
            let rating = Self {
                take: child.arg(0)?,
                range: TimeRange::read(child)?,
                stars: child.opt_prop("stars")?,
                pick: child.opt_prop("pick")?.unwrap_or(false),
            };
            if rating.stars.is_some_and(|stars| !(1..=5).contains(&stars)) {
                return Err(child.error("`stars` is 1 to 5"));
            }
            if rating.stars.is_none() && !rating.pick {
                return Err(child.error("an `attempt` has `stars`, `pick=#true` or both"));
            }
            ratings.push((rating, child));
        }
        ratings.sort_by(|(a, _), (b, _)| a.order().cmp(&b.order()));
        for pair in ratings.windows(2) {
            let [(a, _), (b, node)] = pair else {
                unreachable!("windows of two")
            };
            if a.take == b.take && a.range.overlaps(b.range) {
                return Err(node.error(format!(
                    "`attempt` ranges of take `{}` must not overlap",
                    b.take
                )));
            }
        }
        Ok(ratings.into_iter().map(|(r, _)| r).collect())
    }
}

impl Music {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect(1, &["gain", "duck"])?;
        let music = Self {
            file: node.arg(0)?,
            gain: node.prop("gain")?,
            duck: node.prop("duck")?,
        };
        if music.file.is_empty() {
            return Err(node.error("`music` names a file"));
        }
        if music.gain > 0 || music.duck > 0 {
            return Err(node.error("`gain` and `duck` are dB at most 0"));
        }
        Ok(music)
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

impl Override {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_block(2, &["at"])?;
        let kind: ElementKind = node.arg(1)?;
        let mut decision = None;
        let mut effects: Vec<OverrideEffect> = Vec::new();
        for child in node.children() {
            match child.name() {
                "keep" | "cut" => {
                    child.expect(0, &[])?;
                    let answer = if child.name() == "keep" {
                        Decision::Keep
                    } else {
                        Decision::Cut
                    };
                    if decision.replace(answer).is_some() {
                        return Err(child.error("an override answers `keep` or `cut` once"));
                    }
                }
                name if Effect::is_effect(name) => {
                    let args: Vec<KdlValue> = child.args()?;
                    let effect = match args.last().and_then(KdlValue::as_string) {
                        Some("off") => {
                            child.expect(args.len(), &[])?;
                            let key = match (name, args.len()) {
                                ("overlay", 2) => EffectKey::Overlay(child.arg(0)?),
                                ("layout", 1) => EffectKey::Layout,
                                ("zoom", 1) => EffectKey::Zoom,
                                ("sfx", 1) => EffectKey::Sfx,
                                ("transition", 1) => EffectKey::Transition,
                                ("music", 1) => EffectKey::Music,
                                ("captions", 1) => EffectKey::Captions,
                                _ => {
                                    return Err(child.error(format!("`{name} off` takes no value")));
                                }
                            };
                            OverrideEffect::Off(key)
                        }
                        _ => OverrideEffect::Set(Effect::read(child, Target::Kind(kind))?),
                    };
                    if effects.iter().any(|e| e.key() == effect.key()) {
                        return Err(child.error(format!("duplicate `{name}`")));
                    }
                    effects.push(effect);
                }
                _ => return Err(child.unknown()),
            }
        }
        if decision.is_none() && effects.is_empty() {
            return Err(node.error("an override changes at least one thing"));
        }
        effects.sort_by_key(OverrideEffect::key);
        Ok(Self {
            take: node.arg(0)?,
            kind,
            at: node.prop("at")?,
            decision,
            effects,
        })
    }

    fn write(&self, w: &mut Writer) {
        w.node("override")
            .arg(&self.take)
            .arg(&self.kind)
            .prop("at", &self.at)
            .open();
        match self.decision {
            Some(Decision::Keep) => w.node("keep").end(),
            Some(Decision::Cut) => w.node("cut").end(),
            None => {}
        }
        for effect in &self.effects {
            match effect {
                OverrideEffect::Set(effect) => effect.write(w),
                OverrideEffect::Off(key) => {
                    key.write(w);
                    w.arg(&Bare("off")).end();
                }
            }
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

/// The block's [`OldBlock`] for a node the rules wrote, or the refusal outside migration.
fn old_block<'b>(
    old: &'b mut Option<&mut OldBlock>,
    node: Node<'_>,
    what: String,
) -> Result<&'b mut OldBlock, ParseError> {
    old.as_deref_mut().ok_or_else(|| refused(node, &what))
}

fn write_range(w: &mut Writer, range: TimeRange) -> &mut Writer {
    w.prop("from", &range.from()).prop("to", &range.to())
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
            Self::Integer(n) => {
                if *n < 0 {
                    out.push('-');
                }
                crate::values::write_grouped(n.unsigned_abs(), out);
            }
            Self::Bool(b) => b.write_kdl(out),
        }
    }
}
