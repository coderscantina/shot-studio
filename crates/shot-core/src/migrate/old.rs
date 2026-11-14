//! The grammar from before style rules: rule removals, the review queue and rule zooms in
//! `cut.kdl`, `pacing`, `auto-cut` and `auto-zoom` in a format file. Only migration reads it;
//! every other reader refuses it with [`refused`].

use kdl::KdlValue;

use crate::kdl_io::{FromKdl, Node, Source, keyword_enum, required};
use crate::{FormatKind, Micros, ParseError, TimeRange};

keyword_enum! {
    /// An auto-cut rule, and the rule a `remove` or `review` names.
    pub enum RuleKind {
        Silence = "silence",
        Retake = "retake",
        Misspoken = "misspoken",
        Pause = "pause",
        Filler = "filler",
        IdleScreen = "idle-screen",
    }
}

keyword_enum! {
    /// How much breath auto cut left around a cut.
    pub enum Pacing {
        Loose = "loose",
        Natural = "natural",
        Tight = "tight",
    }
}

impl Pacing {
    /// The silence kept on each side of a gap cut that borders material.
    pub fn breath(self) -> Micros {
        match self {
            Self::Loose => Micros(500_000),
            Self::Natural => Micros(250_000),
            Self::Tight => Micros(50_000),
        }
    }
}

keyword_enum! {
    /// What the auto-zoom rule reacted to.
    pub enum ZoomTrigger {
        Click = "click",
        Typing = "typing",
        Cursor = "cursor",
    }
}

keyword_enum! {
    /// What wrote a rule zoom.
    pub enum ZoomBy {
        /// The pacing's punch-in on every other jump cut.
        JumpCut = "jump-cut",
        Emphasis = "emphasis",
        Click = "click",
        Typing = "typing",
        Cursor = "cursor",
        /// The Z hotkey while recording.
        Marker = "marker",
    }
}

/// Who removed a range: a person, or a rule before the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovedBy {
    Manual,
    Rule(RuleKind),
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

/// A finding the rule was not sure about: `review` while it waited, `ignore` once kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Review {
    pub range: TimeRange,
    pub by: RuleKind,
    pub kept: bool,
}

/// What the rules wrote into one take block, by position in the block. The block itself holds
/// the rule removals as plain `remove` ranges and the rule zooms as plain zooms, so it plays
/// what the old file showed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OldBlock {
    /// Indexes into the block's ranges.
    pub removals: Vec<(usize, RuleKind)>,
    pub reviews: Vec<Review>,
    /// Indexes into the block's zooms.
    pub zooms: Vec<(usize, ZoomBy)>,
}

/// A rule that fired once a stretch lasted at least `min`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedRule {
    pub on: bool,
    pub min: Micros,
}

/// Auto-cut switches and thresholds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCut {
    pub confidence: u8,
    pub silence: TimedRule,
    pub retake: bool,
    pub misspoken: bool,
    pub pause: TimedRule,
    pub filler: bool,
    pub idle_screen: TimedRule,
}

/// The auto-zoom rule: what it reacted to and the zooms it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoZoom {
    pub triggers: Vec<ZoomTrigger>,
    pub scale: u16,
    pub hold: Micros,
}

/// The old settings one format file sets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OldSettings {
    pub pacing: Option<Pacing>,
    pub auto_cut: Option<AutoCut>,
    pub auto_zoom: Option<AutoZoom>,
}

impl OldSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Reads `node` when it is an old setting. False for any other node.
    pub(crate) fn read(&mut self, node: Node<'_>) -> Result<bool, ParseError> {
        match node.name() {
            "pacing" => {
                node.expect(1, &[])?;
                node.set_once(&mut self.pacing, node.arg(0)?)?;
            }
            "auto-cut" => node.set_once(&mut self.auto_cut, AutoCut::read(node)?)?,
            "auto-zoom" => node.set_once(&mut self.auto_zoom, AutoZoom::read(node)?)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// The old settings after the cascade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub pacing: Pacing,
    pub auto_cut: AutoCut,
    pub auto_zoom: Option<AutoZoom>,
}

impl Settings {
    /// What the shipped preset of `kind` set before the lock.
    pub fn preset(kind: FormatKind) -> Self {
        let text = match kind {
            FormatKind::Reaction => PRESET_REACTION,
            FormatKind::Opinion => PRESET_OPINION,
            FormatKind::ProductTour => PRESET_PRODUCT_TOUR,
            FormatKind::Tutorial => PRESET_TUTORIAL,
        };
        let src = Source {
            file: "format.kdl",
            text,
        };
        let doc = src.parse().expect("old presets parse");
        let mut old = OldSettings::default();
        for node in src.nodes(&doc) {
            assert!(old.read(node).expect("old presets parse"));
        }
        Self {
            pacing: old.pacing.expect("presets set the pacing"),
            auto_cut: old.auto_cut.expect("presets set auto-cut"),
            auto_zoom: old.auto_zoom,
        }
    }

    /// These settings with what `layer` sets replacing them.
    pub fn layered(&self, layer: &OldSettings) -> Self {
        Self {
            pacing: layer.pacing.unwrap_or(self.pacing),
            auto_cut: layer.auto_cut.clone().unwrap_or(self.auto_cut.clone()),
            auto_zoom: layer.auto_zoom.clone().or(self.auto_zoom.clone()),
        }
    }

    /// The settings of a full `format.kdl` of a video: the preset's pacing and auto-cut where
    /// it sets none, and auto-zoom only when it has one.
    pub fn full(kind: FormatKind, file: &OldSettings) -> Self {
        Self {
            auto_zoom: file.auto_zoom.clone(),
            ..Self::preset(kind).layered(file)
        }
    }
}

const PRESET_REACTION: &str = "pacing natural
auto-cut confidence=85 {
    silence on=#true min=600_000
    retake on=#true
    misspoken on=#true
    pause on=#true min=1_000_000
    filler on=#true
    idle-screen on=#false min=8_000_000
}
";
const PRESET_OPINION: &str = "pacing tight
auto-cut confidence=85 {
    silence on=#true min=600_000
    retake on=#true
    misspoken on=#true
    pause on=#true min=1_000_000
    filler on=#true
    idle-screen on=#false min=8_000_000
}
";
const PRESET_PRODUCT_TOUR: &str = "pacing tight
auto-cut confidence=85 {
    silence on=#true min=600_000
    retake on=#true
    misspoken on=#true
    pause on=#true min=1_000_000
    filler on=#true
    idle-screen on=#true min=3_000_000
}
auto-zoom click typing scale=160 hold=1_500_000
";
const PRESET_TUTORIAL: &str = "pacing natural
auto-cut confidence=85 {
    silence on=#true min=600_000
    retake on=#true
    misspoken on=#true
    pause on=#true min=1_000_000
    filler on=#true
    idle-screen on=#true min=5_000_000
}
";

/// An error for an old node outside migration, `what` as it was written. Opening a video
/// migrates it only while it has no `cut.lock`, so the node reached a video with a lock, where
/// neither `shot migrate` nor `shot restyle` (which builds from the same files) can read it.
pub(crate) fn refused(node: Node<'_>, what: &str) -> ParseError {
    node.error(format!(
        "{what} is from before style rules, and a video with a cut.lock is not migrated again: \
         remove the node by hand or write it as a person's cut, an override or a rule"
    ))
}

impl AutoZoom {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_any(&["scale", "hold"])?;
        let mut triggers: Vec<ZoomTrigger> = node.args()?;
        let before = triggers.len();
        triggers.sort_unstable();
        triggers.dedup();
        if triggers.len() != before {
            return Err(node.error("`auto-zoom` lists a trigger twice"));
        }
        let scale: u16 = node.prop("scale")?;
        if scale < 100 {
            return Err(node.error("`scale` is a percentage of at least 100"));
        }
        Ok(Self {
            triggers,
            scale,
            hold: node.prop("hold")?,
        })
    }
}

impl AutoCut {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_block(0, &["confidence"])?;
        let confidence: u8 = node.prop("confidence")?;
        if confidence > 100 {
            return Err(node.error("`confidence` is a percentage from 0 to 100"));
        }
        let (mut silence, mut retake, mut misspoken, mut pause, mut filler, mut idle_screen) =
            (None, None, None, None, None, None);
        for rule in node.children() {
            match rule.name() {
                "silence" => rule.set_once(&mut silence, TimedRule::read(rule)?)?,
                "pause" => rule.set_once(&mut pause, TimedRule::read(rule)?)?,
                "idle-screen" => rule.set_once(&mut idle_screen, TimedRule::read(rule)?)?,
                "retake" => rule.set_once(&mut retake, read_switch(rule)?)?,
                "misspoken" => rule.set_once(&mut misspoken, read_switch(rule)?)?,
                "filler" => rule.set_once(&mut filler, read_switch(rule)?)?,
                _ => return Err(rule.unknown()),
            }
        }
        let missing = |message| node.error(message);
        Ok(Self {
            confidence,
            silence: required(silence, "silence", missing)?,
            retake: required(retake, "retake", missing)?,
            misspoken: required(misspoken, "misspoken", missing)?,
            pause: required(pause, "pause", missing)?,
            filler: required(filler, "filler", missing)?,
            idle_screen: required(idle_screen, "idle-screen", missing)?,
        })
    }
}

fn read_switch(node: Node<'_>) -> Result<bool, ParseError> {
    node.expect(0, &["on"])?;
    node.prop("on")
}

impl TimedRule {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect(0, &["on", "min"])?;
        Ok(Self {
            on: node.prop("on")?,
            min: node.prop("min")?,
        })
    }
}
