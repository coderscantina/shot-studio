//! Style rules: `on <selector> { <effects> }` nodes shared by `brand.kdl`, a channel's
//! `formats/<format>.kdl` and a video's `format.kdl`.
//!
//! Selectors and effects are closed sets. A rule's selector is its identity across the cascade:
//! a later layer replaces an earlier rule with the same selector or switches it off with
//! `on <selector> off`. When several rules give one element the same effect, the selector with
//! more filters wins, then the later layer, then the later rule in the file.

use std::fmt;

use kdl::KdlValue;

use crate::kdl_io::{Bare, FromKdl, Node, Source, ToKdl, Writer, keyword_enum};
use crate::{
    Attr, AttrValue, CaptionStyle, ElementKind, Id, Layout, Micros, ParamValue, ParseError,
};

keyword_enum! {
    /// Where a rule or an override came from, lowest layer first.
    pub enum Layer {
        /// A style imported into the brand from git.
        Import = "import",
        Brand = "brand",
        /// The channel's `formats/<format>.kdl`.
        Format = "format",
        /// The video's `format.kdl`.
        Video = "video",
        /// An `override` in `cut.kdl`.
        Override = "override",
    }
}

/// What a selector picks: an element kind, or a moment of the cut's structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    Kind(ElementKind),
    /// The first kept moment of each chapter.
    ChapterStart,
    /// The last kept moment of each chapter.
    ChapterEnd,
    VideoStart,
    VideoEnd,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kind(kind) => kind.as_str(),
            Self::ChapterStart => "chapter-start",
            Self::ChapterEnd => "chapter-end",
            Self::VideoStart => "video-start",
            Self::VideoEnd => "video-end",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "chapter-start" => Self::ChapterStart,
            "chapter-end" => Self::ChapterEnd,
            "video-start" => Self::VideoStart,
            "video-end" => Self::VideoEnd,
            kind => Self::Kind(ElementKind::parse(kind)?),
        })
    }

    /// Whether the target is a moment rather than a span.
    pub fn is_point(self) -> bool {
        match self {
            Self::Kind(kind) => kind.is_point(),
            _ => true,
        }
    }

    /// The attributes an element of this target can carry.
    fn attrs(self) -> Vec<Attr> {
        match self {
            Self::Kind(kind) => {
                let (required, optional) = kind.attrs();
                required.iter().chain(optional).copied().collect()
            }
            _ => Vec::new(),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromKdl for Target {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let text = value
            .as_string()
            .ok_or_else(|| "expected an element kind".to_owned())?;
        Self::parse(text).ok_or_else(|| format!("unknown element kind `{text}`"))
    }
}

impl ToKdl for Target {
    fn write_kdl(&self, out: &mut String) {
        out.push_str(self.as_str());
    }
}

/// Narrows a selector. Written as properties of the `on` node, in this order: attributes in
/// their canonical order, then `min`, then `confidence`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Filter {
    /// The element's attribute equals the value.
    Attr(Attr, AttrValue),
    /// The element spans at least this long.
    Min(Micros),
    /// The element's confidence is at least this percent. Facts without one always pass.
    Confidence(u8),
}

/// `on <target> <filters>`: which elements a rule is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Selector {
    pub target: Target,
    /// In canonical order, each attribute at most once.
    pub filters: Vec<Filter>,
}

impl Selector {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            filters: Vec::new(),
        }
    }

    /// How many filters narrow it. A more specific selector wins a shared effect.
    pub fn specificity(&self) -> usize {
        self.filters.len()
    }

    /// Parses the canonical text, as the lock writes it: `silence min=500_000`.
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let text = format!("on {text}");
        let src = Source {
            file: "selector",
            text: &text,
        };
        let doc = src.parse()?;
        let mut nodes = src.nodes(&doc);
        let (Some(node), None) = (nodes.next(), nodes.next()) else {
            return Err(src.error(0, "expected one selector"));
        };
        let (selector, off) = Self::read(node)?;
        if off {
            return Err(node.error("a selector has no `off`"));
        }
        Ok(selector)
    }

    /// Reads an `on` node (or, for [`Selector::parse`], the bare selector). Returns the
    /// selector and whether the rule is switched off.
    pub(crate) fn read(node: Node<'_>) -> Result<(Self, bool), ParseError> {
        let props: Vec<&str> = Attr::ALL
            .iter()
            .map(|a| a.as_str())
            .chain(["min", "confidence"])
            .collect();
        node.expect_any_block(&props)?;
        let args: Vec<String> = node.args()?;
        let (first, off) = match args.as_slice() {
            [] => return Err(node.error("`on` names an element kind")),
            [first] => (first, false),
            [first, second] if second == "off" => (first, true),
            _ => return Err(node.error("`on` takes a selector and optionally `off`")),
        };
        let target = Target::parse(first)
            .ok_or_else(|| node.error(format!("unknown element kind `{first}`")))?;
        let allowed = target.attrs();
        let mut filters = Vec::new();
        for attr in Attr::ALL {
            let Some(value) = node.opt_prop::<KdlValue>(attr.as_str())? else {
                continue;
            };
            if !allowed.contains(attr) {
                return Err(node.error(format!("`{target}` has no attribute `{attr}`")));
            }
            let value = if attr.is_integer() {
                AttrValue::Integer(i64::from_kdl(&value).map_err(|e| node.error(e))?)
            } else {
                AttrValue::Text(String::from_kdl(&value).map_err(|e| node.error(e))?)
            };
            filters.push(Filter::Attr(*attr, value));
        }
        if let Some(min) = node.opt_prop::<Micros>("min")? {
            if target.is_point() {
                return Err(node.error(format!("`{target}` is a moment and has no `min`")));
            }
            filters.push(Filter::Min(min));
        }
        if let Some(confidence) = node.opt_prop::<u8>("confidence")? {
            if confidence > 100 {
                return Err(node.error("`confidence` is a percent from 0 to 100"));
            }
            if !matches!(target, Target::Kind(_)) {
                return Err(node.error(format!("`{target}` has no confidence")));
            }
            filters.push(Filter::Confidence(confidence));
        }
        Ok((Self { target, filters }, off))
    }

    pub(crate) fn write(&self, w: &mut Writer, off: bool) {
        w.arg(&self.target);
        for filter in &self.filters {
            match filter {
                // Id-like text is written bare, so `host=github` reads the same in a rule and
                // in the lock's `rule="..."`.
                Filter::Attr(attr, AttrValue::Text(s)) => match Id::new(s.as_str()) {
                    Some(id) => w.prop(attr.as_str(), &id),
                    None => w.prop(attr.as_str(), s),
                },
                Filter::Attr(attr, AttrValue::Integer(n)) => w.prop(attr.as_str(), n),
                Filter::Min(min) => w.prop("min", min),
                Filter::Confidence(c) => w.prop("confidence", c),
            };
        }
        if off {
            w.arg(&Bare("off"));
        }
    }
}

impl fmt::Display for Selector {
    /// The canonical text without the `on`, as the lock records it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut w = Writer::default();
        w.node("");
        self.write(&mut w, false);
        f.write_str(w.finish().trim_start())
    }
}

keyword_enum! {
    /// How a chapter starts ([0021], [0023]).
    ///
    /// [0021]: ../../../docs/decisions/0021-sound-transitions-and-bookends.md
    /// [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md
    pub enum TransitionKind {
        Cut = "cut",
        Dissolve = "dissolve",
        Dip = "dip",
        /// The brand's Lottie stinger, covering the cut at its peak.
        Stinger = "stinger",
        MaskWipe = "mask-wipe",
        ZoomThrough = "zoom-through",
    }
}

/// The default `length` of a transition.
pub const TRANSITION_LENGTH: Micros = Micros(500_000);

/// What a rule does to each element it selects. One effect per [`EffectKey`] in a rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Pacing: removes the element, keeping `breath` on each side that borders material. An
    /// element less sure than `confidence` percent goes to the review queue instead.
    Cut { breath: Micros, confidence: u8 },
    /// A layout over the element's span.
    Layout(Layout),
    /// A punch-in of `scale` percent from `lead` before the element until `hold` after its
    /// start or its end, whichever is later, centered on the element's point when it has one.
    Zoom {
        scale: u16,
        hold: Micros,
        lead: Micros,
    },
    /// A template overlay over the element's span, or for `length` from its start.
    Overlay {
        template: Id,
        length: Option<Micros>,
        /// Element attributes copied into parameters of the same name, canonical order.
        copies: Vec<Attr>,
        /// Literal parameters in file order, names unique and not among `copies`.
        params: Vec<(Id, ParamValue)>,
    },
    /// A sound at the element's start, `gain` dB at most 0.
    Sfx { sound: Id, gain: i16 },
    /// How the chapter starting here joins the one before. Only on `chapter-start` and
    /// `video-start`.
    Transition {
        kind: TransitionKind,
        length: Micros,
    },
    /// A music cue starting at the element: a file relative to the video folder, its gain and
    /// the gain under speech, both dB at most 0.
    Music { file: String, gain: i16, duck: i16 },
    /// A caption style over the element's span.
    Captions(CaptionStyle),
}

/// Identifies one effect within a rule or an override: the kind, and the template for overlays.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EffectKey {
    Cut,
    Layout,
    Zoom,
    Overlay(Id),
    Sfx,
    Transition,
    Music,
    Captions,
}

impl EffectKey {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Cut => "cut",
            Self::Layout => "layout",
            Self::Zoom => "zoom",
            Self::Overlay(_) => "overlay",
            Self::Sfx => "sfx",
            Self::Transition => "transition",
            Self::Music => "music",
            Self::Captions => "captions",
        }
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.node(self.name());
        if let Self::Overlay(template) = self {
            w.arg(template);
        }
    }
}

impl Effect {
    pub fn key(&self) -> EffectKey {
        match self {
            Self::Cut { .. } => EffectKey::Cut,
            Self::Layout(_) => EffectKey::Layout,
            Self::Zoom { .. } => EffectKey::Zoom,
            Self::Overlay { template, .. } => EffectKey::Overlay(template.clone()),
            Self::Sfx { .. } => EffectKey::Sfx,
            Self::Transition { .. } => EffectKey::Transition,
            Self::Music { .. } => EffectKey::Music,
            Self::Captions(_) => EffectKey::Captions,
        }
    }

    /// Whether `name` is an effect node.
    pub fn is_effect(name: &str) -> bool {
        matches!(
            name,
            "cut" | "layout" | "zoom" | "overlay" | "sfx" | "transition" | "music" | "captions"
        )
    }

    /// Reads an effect node and checks it suits `target`.
    pub(crate) fn read(node: Node<'_>, target: Target) -> Result<Self, ParseError> {
        let effect = match node.name() {
            "cut" => {
                node.expect(0, &["breath", "confidence"])?;
                let confidence: u8 = node.opt_prop("confidence")?.unwrap_or(0);
                if confidence > 100 {
                    return Err(node.error("`confidence` is a percent from 0 to 100"));
                }
                Self::Cut {
                    breath: node.opt_prop("breath")?.unwrap_or_default(),
                    confidence,
                }
            }
            "layout" => {
                node.expect(1, &[])?;
                Self::Layout(node.arg(0)?)
            }
            "zoom" => {
                node.expect(1, &["hold", "lead"])?;
                let scale: u16 = node.arg(0)?;
                if scale < 100 {
                    return Err(node.error("zoom scale is a percentage of at least 100"));
                }
                Self::Zoom {
                    scale,
                    hold: node.opt_prop("hold")?.unwrap_or_default(),
                    lead: node.opt_prop("lead")?.unwrap_or_default(),
                }
            }
            "overlay" => {
                node.expect_block(1, &["for"])?;
                let (mut copies, mut params): (Vec<Attr>, Vec<(Id, ParamValue)>) =
                    (Vec::new(), Vec::new());
                let allowed = target.attrs();
                for param in node.children() {
                    param.expect_any(&[])?;
                    let name = Id::new(param.name())
                        .ok_or_else(|| param.error("parameter names are ids"))?;
                    let taken = params.iter().any(|(n, _)| *n == name)
                        || copies.iter().any(|a| a.as_str() == name.as_str());
                    if taken {
                        return Err(param.error(format!("duplicate parameter `{name}`")));
                    }
                    match param.args::<ParamValue>()?.as_slice() {
                        [] => {
                            let attr = Attr::parse(name.as_str())
                                .filter(|a| allowed.contains(a))
                                .ok_or_else(|| {
                                param.error(format!("`{target}` has no attribute `{name}`"))
                            })?;
                            copies.push(attr);
                        }
                        [value] => params.push((name, value.clone())),
                        _ => return Err(param.error("a parameter takes one value")),
                    }
                }
                copies.sort_unstable();
                let length = node.opt_prop("for")?;
                if length.is_none() && target.is_point() {
                    return Err(node.error(format!("`{target}` is a moment, give `for`")));
                }
                Self::Overlay {
                    template: node.arg(0)?,
                    length,
                    copies,
                    params,
                }
            }
            "sfx" => {
                node.expect(1, &["gain"])?;
                let gain: i16 = node.opt_prop("gain")?.unwrap_or(0);
                if gain > 0 {
                    return Err(node.error("`gain` is dB at most 0"));
                }
                Self::Sfx {
                    sound: node.arg(0)?,
                    gain,
                }
            }
            "transition" => {
                node.expect(1, &["length"])?;
                if !matches!(target, Target::ChapterStart | Target::VideoStart) {
                    return Err(node.error("`transition` applies to chapter-start or video-start"));
                }
                Self::Transition {
                    kind: node.arg(0)?,
                    length: node.opt_prop("length")?.unwrap_or(TRANSITION_LENGTH),
                }
            }
            "music" => {
                node.expect(1, &["gain", "duck"])?;
                let (gain, duck): (i16, i16) = (node.prop("gain")?, node.prop("duck")?);
                if gain > 0 || duck > 0 {
                    return Err(node.error("`gain` and `duck` are dB at most 0"));
                }
                let file: String = node.arg(0)?;
                if file.is_empty() {
                    return Err(node.error("`music` names a file"));
                }
                Self::Music { file, gain, duck }
            }
            "captions" => {
                node.expect(1, &[])?;
                Self::Captions(node.arg(0)?)
            }
            _ => return Err(node.unknown()),
        };
        if matches!(effect, Self::Layout(_) | Self::Captions(_)) && target.is_point() {
            return Err(node.error(format!(
                "`{}` needs a span, `{target}` is a moment",
                node.name()
            )));
        }
        Ok(effect)
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        self.key().write(w);
        match self {
            Self::Cut { breath, confidence } => {
                if breath.0 > 0 {
                    w.prop("breath", breath);
                }
                if *confidence > 0 {
                    w.prop("confidence", confidence);
                }
                w.end();
            }
            Self::Layout(layout) => w.arg(layout).end(),
            Self::Zoom { scale, hold, lead } => {
                w.arg(scale);
                if hold.0 > 0 {
                    w.prop("hold", hold);
                }
                if lead.0 > 0 {
                    w.prop("lead", lead);
                }
                w.end();
            }
            Self::Overlay {
                length,
                copies,
                params,
                ..
            } => {
                if let Some(length) = length {
                    w.prop("for", length);
                }
                if copies.is_empty() && params.is_empty() {
                    w.end();
                    return;
                }
                w.open();
                for attr in copies {
                    w.node(attr.as_str()).end();
                }
                for (name, value) in params {
                    w.node(name.as_str()).arg(value).end();
                }
                w.close();
            }
            Self::Sfx { sound, gain } => {
                w.arg(sound);
                if *gain != 0 {
                    w.prop("gain", gain);
                }
                w.end();
            }
            Self::Transition { kind, length } => {
                w.arg(kind);
                if *length != TRANSITION_LENGTH {
                    w.prop("length", length);
                }
                w.end();
            }
            Self::Music { file, gain, duck } => {
                w.arg(file).prop("gain", gain).prop("duck", duck).end();
            }
            Self::Captions(style) => w.arg(style).end(),
        }
    }
}

/// One `on` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub selector: Selector,
    pub body: RuleBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleBody {
    /// Effects in canonical key order, one per key.
    Effects(Vec<Effect>),
    /// `on <selector> off`: switches off the rule with this selector from an earlier layer.
    Off,
}

impl Rule {
    pub(crate) fn read(node: Node<'_>) -> Result<Self, ParseError> {
        let (selector, off) = Selector::read(node)?;
        let body = if off {
            if node.children().next().is_some() {
                return Err(node.error("a rule switched off has no effects"));
            }
            RuleBody::Off
        } else {
            let mut effects: Vec<Effect> = Vec::new();
            for child in node.children() {
                let effect = Effect::read(child, selector.target)?;
                if effects.iter().any(|e| e.key() == effect.key()) {
                    return Err(child.error(format!("duplicate `{}`", child.name())));
                }
                effects.push(effect);
            }
            if effects.is_empty() {
                return Err(node.error("a rule needs at least one effect, or `off`"));
            }
            effects.sort_by_key(Effect::key);
            RuleBody::Effects(effects)
        };
        Ok(Self { selector, body })
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.node("on");
        match &self.body {
            RuleBody::Off => {
                self.selector.write(w, true);
                w.end();
            }
            RuleBody::Effects(effects) => {
                self.selector.write(w, false);
                w.open();
                for effect in effects {
                    effect.write(w);
                }
                w.close();
            }
        }
    }
}

/// The rules of one layer, in file order, selectors unique.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleSheet {
    pub rules: Vec<Rule>,
}

impl StyleSheet {
    /// Adds the rule of an `on` node.
    pub(crate) fn add(&mut self, node: Node<'_>) -> Result<(), ParseError> {
        let rule = Rule::read(node)?;
        if self.rules.iter().any(|r| r.selector == rule.selector) {
            return Err(node.error(format!("duplicate rule `on {}`", rule.selector)));
        }
        self.rules.push(rule);
        Ok(())
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        for rule in &self.rules {
            rule.write(w);
        }
    }

    /// Folds the layers, lowest first, into the rules that apply: a later rule with the same
    /// selector replaces an earlier one, `off` removes it. The result keeps the order rules
    /// were added in, so a later position means a later layer or a later line.
    pub fn cascade<'a>(layers: impl IntoIterator<Item = (Layer, &'a Self)>) -> Vec<ActiveRule> {
        let mut active: Vec<ActiveRule> = Vec::new();
        for (layer, sheet) in layers {
            for rule in &sheet.rules {
                active.retain(|a| a.selector != rule.selector);
                if let RuleBody::Effects(effects) = &rule.body {
                    active.push(ActiveRule {
                        layer,
                        selector: rule.selector.clone(),
                        effects: effects.clone(),
                    });
                }
            }
        }
        active
    }
}

/// A rule that survived the cascade, with the layer it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRule {
    pub layer: Layer,
    pub selector: Selector,
    pub effects: Vec<Effect>,
}
