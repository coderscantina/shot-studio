use kdl::KdlValue;

use crate::kdl_io::{FromKdl, Node, Source, ToKdl, Writer, keyword_enum, quote, required};
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

impl Layout {
    /// Whether the screen track is in the frame, so cursor and keystroke settings draw.
    pub fn shows_screen(self) -> bool {
        !matches!(self, Self::Face)
    }
}

keyword_enum! {
    /// What the auto-zoom rule reacts to.
    pub enum ZoomTrigger {
        Click = "click",
        Typing = "typing",
        Cursor = "cursor",
    }
}

keyword_enum! {
    /// How every zoom of a video moves in, out and between centers.
    pub enum ZoomMotion {
        Instant = "instant",
        Smooth = "smooth",
    }
}

keyword_enum! {
    /// Which keystrokes the keystroke overlay shows.
    pub enum Keystrokes {
        Off = "off",
        /// Only keys with a modifier held.
        Shortcuts = "shortcuts",
        All = "all",
    }
}

keyword_enum! {
    pub enum Shadow {
        None = "none",
        Soft = "soft",
        Hard = "hard",
    }
}

keyword_enum! {
    /// A device bezel drawn around the window in the `window` layout.
    pub enum DeviceFrame {
        None = "none",
        Macbook = "macbook",
        Iphone = "iphone",
    }
}

/// An sRGB color, written as `"#rrggbb"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color(pub [u8; 3]);

impl FromKdl for Color {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let text = String::from_kdl(value)?;
        let parsed = text
            .strip_prefix('#')
            .filter(|hex| hex.len() == 6)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .map(|rgb| rgb.to_be_bytes());
        match parsed {
            Some([_, r, g, b]) => Ok(Self([r, g, b])),
            None => Err(format!("`{text}` is not a color, expected \"#rrggbb\"")),
        }
    }
}

impl ToKdl for Color {
    fn write_kdl(&self, out: &mut String) {
        let [r, g, b] = self.0;
        quote(&format!("#{r:02x}{g:02x}{b:02x}"), out);
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
    /// The product video settings. Each is optional; `None` means the contract's default.
    pub auto_zoom: Option<AutoZoom>,
    pub zoom_motion: Option<ZoomMotion>,
    pub cursor: Option<CursorStyle>,
    pub keystrokes: Option<Keystrokes>,
    pub window: Option<WindowStyle>,
}

/// The auto-zoom rule: what it reacts to and the zooms it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoZoom {
    /// In canonical order, unique. Empty means the rule is configured but off.
    pub triggers: Vec<ZoomTrigger>,
    /// Scale of the written zooms in percent, at least 100.
    pub scale: u16,
    /// Shortest zoom it writes.
    pub hold: Micros,
}

/// How the cursor is drawn from the event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CursorStyle {
    /// Percent of the recorded size. 0 hides the cursor.
    pub size: u16,
    /// Window over which the cursor path is averaged. 0 draws the raw path.
    pub smoothing: Micros,
    /// A ripple on every click.
    pub ripple: bool,
}

/// How the `window` layout places the captured window on its background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowStyle {
    /// Pixels of the output frame.
    pub padding: u16,
    pub corners: u16,
    pub shadow: Shadow,
    pub background: Color,
    pub frame: DeviceFrame,
}

/// The settings that reach the renderer, resolved with the contract's defaults. Each part is
/// hashed into a chunk only when it draws there, see `Output::chunk_hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Effects {
    pub zoom_motion: ZoomMotion,
    pub cursor: CursorStyle,
    pub keystrokes: Keystrokes,
    pub window: WindowStyle,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            zoom_motion: ZoomMotion::Instant,
            cursor: CursorStyle {
                size: 100,
                smoothing: Micros(0),
                ripple: false,
            },
            keystrokes: Keystrokes::Off,
            window: WindowStyle {
                padding: 48,
                corners: 12,
                shadow: Shadow::Soft,
                background: Color([0x10, 0x14, 0x18]),
                frame: DeviceFrame::None,
            },
        }
    }
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
        let (mut auto_zoom, mut zoom_motion, mut cursor, mut keystrokes, mut window) =
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
                "auto-zoom" => node.set_once(&mut auto_zoom, AutoZoom::read(node)?)?,
                "zoom-motion" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut zoom_motion, node.arg(0)?)?;
                }
                "cursor" => node.set_once(&mut cursor, CursorStyle::read(node)?)?,
                "keystrokes" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut keystrokes, node.arg(0)?)?;
                }
                "window" => node.set_once(&mut window, WindowStyle::read(node)?)?,
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
            auto_zoom,
            zoom_motion,
            cursor,
            keystrokes,
            window,
        })
    }

    /// The render settings with the defaults filled in.
    pub fn effects(&self) -> Effects {
        let defaults = Effects::default();
        Effects {
            zoom_motion: self.zoom_motion.unwrap_or(defaults.zoom_motion),
            cursor: self.cursor.unwrap_or(defaults.cursor),
            keystrokes: self.keystrokes.unwrap_or(defaults.keystrokes),
            window: self.window.unwrap_or(defaults.window),
        }
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
        if let Some(z) = &self.auto_zoom {
            w.node("auto-zoom");
            for trigger in &z.triggers {
                w.arg(trigger);
            }
            w.prop("scale", &z.scale).prop("hold", &z.hold).end();
        }
        if let Some(motion) = &self.zoom_motion {
            w.node("zoom-motion").arg(motion).end();
        }
        if let Some(c) = &self.cursor {
            w.node("cursor")
                .prop("size", &c.size)
                .prop("smoothing", &c.smoothing)
                .prop("ripple", &c.ripple)
                .end();
        }
        if let Some(keystrokes) = &self.keystrokes {
            w.node("keystrokes").arg(keystrokes).end();
        }
        if let Some(win) = &self.window {
            w.node("window")
                .prop("padding", &win.padding)
                .prop("corners", &win.corners)
                .prop("shadow", &win.shadow)
                .prop("background", &win.background)
                .prop("frame", &win.frame)
                .end();
        }
        w.finish()
    }
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

impl CursorStyle {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect(0, &["size", "smoothing", "ripple"])?;
        Ok(Self {
            size: node.prop("size")?,
            smoothing: node.prop("smoothing")?,
            ripple: node.prop("ripple")?,
        })
    }
}

impl WindowStyle {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect(0, &["padding", "corners", "shadow", "background", "frame"])?;
        Ok(Self {
            padding: node.prop("padding")?,
            corners: node.prop("corners")?,
            shadow: node.prop("shadow")?,
            background: node.prop("background")?,
            frame: node.prop("frame")?,
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

    fn write(self, w: &mut Writer, kind: RuleKind) {
        w.node(kind.as_str())
            .prop("on", &self.on)
            .prop("min", &self.min)
            .end();
    }
}
