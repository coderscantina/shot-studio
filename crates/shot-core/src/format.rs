use crate::kdl_io::{Node, Source, Writer, keyword_enum, required};
use crate::{Micros, ParseError};

keyword_enum! {
    /// The four shipped formats. Each has a preset `format.kdl`.
    pub enum FormatKind {
        Reaction = "reaction",
        Opinion = "opinion",
        ProductTour = "product-tour",
        Tutorial = "tutorial",
    }
}

impl FormatKind {
    /// The preset file as shipped, in canonical form.
    pub fn preset_source(self) -> &'static str {
        match self {
            Self::Reaction => include_str!("../formats/reaction.kdl"),
            Self::Opinion => include_str!("../formats/opinion.kdl"),
            Self::ProductTour => include_str!("../formats/product-tour.kdl"),
            Self::Tutorial => include_str!("../formats/tutorial.kdl"),
        }
    }

    pub fn preset(self) -> Format {
        Format::parse(self.preset_source()).expect("shipped presets parse")
    }
}

keyword_enum! {
    /// What the prompter shows.
    pub enum PrompterMode {
        Beats = "beats",
        FullScript = "full-script",
    }
}

keyword_enum! {
    /// How much breath auto cut leaves around a cut.
    pub enum Pacing {
        Loose = "loose",
        Natural = "natural",
        Tight = "tight",
    }
}

keyword_enum! {
    /// How screen and camera are arranged in the frame.
    pub enum Layout {
        Face = "face",
        FaceContent = "face-content",
        Split = "split",
        Content = "content",
        Screen = "screen",
        FaceCorner = "face-corner",
        /// A captured window on a background, with padding, corners and shadow.
        Window = "window",
    }
}

keyword_enum! {
    pub enum CaptionStyle {
        WordPop = "word-pop",
        Clean = "clean",
        Off = "off",
    }
}

keyword_enum! {
    /// An auto-cut rule. Also names the rule that removed a range in `cut.kdl`.
    pub enum RuleKind {
        Silence = "silence",
        Retake = "retake",
        Misspoken = "misspoken",
        Pause = "pause",
        Filler = "filler",
        IdleScreen = "idle-screen",
    }
}

/// `format.kdl`: pacing, layouts, captions, auto-cut thresholds and prompter mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    pub prompter: PrompterMode,
    pub pacing: Pacing,
    /// The layout used wherever `cut.kdl` sets none. Written first in `layouts`.
    pub default_layout: Layout,
    /// Further layouts offered on the 1 to 4 hotkeys after the default.
    pub other_layouts: Vec<Layout>,
    pub captions: CaptionStyle,
    pub auto_cut: AutoCut,
}

/// Auto-cut switches and thresholds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCut {
    /// Results at or above this confidence (percent) apply right away, the rest go to review.
    pub confidence: u8,
    pub silence: TimedRule,
    pub retake: bool,
    pub misspoken: bool,
    pub pause: TimedRule,
    pub filler: bool,
    pub idle_screen: TimedRule,
}

/// A rule that fires once a stretch lasts at least `min`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedRule {
    pub on: bool,
    pub min: Micros,
}

impl Format {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let src = Source {
            file: "format.kdl",
            text,
        };
        let doc = src.parse()?;
        let (mut prompter, mut pacing, mut layouts, mut captions, mut auto_cut) =
            (None, None, None, None, None);
        for node in src.nodes(&doc) {
            match node.name() {
                "prompter" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut prompter, node.arg(0)?)?;
                }
                "pacing" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut pacing, node.arg(0)?)?;
                }
                "layouts" => {
                    node.expect_list()?;
                    node.set_once(&mut layouts, node.args::<Layout>()?)?;
                }
                "captions" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut captions, node.arg(0)?)?;
                }
                "auto-cut" => node.set_once(&mut auto_cut, AutoCut::read(node)?)?,
                _ => return Err(node.unknown()),
            }
        }
        let missing = |message| src.error(0, message);
        let mut layouts = required(layouts, "layouts", missing)?.into_iter();
        Ok(Self {
            prompter: required(prompter, "prompter", missing)?,
            pacing: required(pacing, "pacing", missing)?,
            default_layout: layouts.next().expect("layouts has at least one argument"),
            other_layouts: layouts.collect(),
            captions: required(captions, "captions", missing)?,
            auto_cut: required(auto_cut, "auto-cut", missing)?,
        })
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        w.node("prompter").arg(&self.prompter).end();
        w.node("pacing").arg(&self.pacing).end();
        w.node("layouts").arg(&self.default_layout);
        for layout in &self.other_layouts {
            w.arg(layout);
        }
        w.end();
        w.node("captions").arg(&self.captions).end();
        let a = &self.auto_cut;
        w.node("auto-cut").prop("confidence", &a.confidence).open();
        a.silence.write(&mut w, RuleKind::Silence);
        w.node(RuleKind::Retake.as_str())
            .prop("on", &a.retake)
            .end();
        w.node(RuleKind::Misspoken.as_str())
            .prop("on", &a.misspoken)
            .end();
        a.pause.write(&mut w, RuleKind::Pause);
        w.node(RuleKind::Filler.as_str())
            .prop("on", &a.filler)
            .end();
        a.idle_screen.write(&mut w, RuleKind::IdleScreen);
        w.close();
        w.finish()
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

    fn write(self, w: &mut Writer, kind: RuleKind) {
        w.node(kind.as_str())
            .prop("on", &self.on)
            .prop("min", &self.min)
            .end();
    }
}
