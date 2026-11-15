use kdl::KdlValue;

use crate::brand::BrandScope;
use crate::captions::{CaptionLook, LOOK_PROPS};
use crate::kdl_io::{FromKdl, Node, Source, ToKdl, Writer, keyword_enum, quote, required};
use crate::migrate::old::{OldSettings, refused};
use crate::{Brand, Micros, ParseError, StyleSheet, Transition};

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

    /// The preset's settings. Its default rules are in [`FormatKind::preset_layer`].
    pub fn preset(self) -> Format {
        let src = Source {
            file: "format.kdl",
            text: self.preset_source(),
        };
        let doc = src.parse().expect("shipped presets parse");
        FormatLayer::read(src, &doc, None)
            .and_then(|layer| layer.complete(src))
            .expect("shipped presets parse")
    }

    /// The preset as a layer: every setting and the default rules its settings stand for, the
    /// starting copy of a channel's `formats/<format>.kdl`.
    pub fn preset_layer(self) -> FormatLayer {
        FormatLayer::parse(self.preset_source()).expect("shipped presets parse")
    }
}

keyword_enum! {
    /// What the prompter shows.
    pub enum PrompterMode {
        Beats = "beats",
        FullScript = "full-script",
    }
}

/// How screen and camera are arranged in the frame: a built-in layout, or one the channel
/// defines in its brand by name ([`crate::CustomLayout`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layout {
    Face,
    FaceContent,
    Split,
    Content,
    Screen,
    FaceCorner,
    /// A captured window on a background, with padding, corners and shadow.
    Window,
    /// A channel layout by its name, drawn from the brand's `layout` node of that name.
    Custom(LayoutName),
}

impl Layout {
    /// The built-in layouts, in the order they are offered.
    pub const ALL: &[Self] = &[
        Self::Face,
        Self::FaceContent,
        Self::Split,
        Self::Content,
        Self::Screen,
        Self::FaceCorner,
        Self::Window,
    ];

    /// The keyword as written in the project files.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Face => "face",
            Self::FaceContent => "face-content",
            Self::Split => "split",
            Self::Content => "content",
            Self::Screen => "screen",
            Self::FaceCorner => "face-corner",
            Self::Window => "window",
            Self::Custom(name) => name.as_str(),
        }
    }

    /// A built-in layout by its keyword, otherwise a channel layout by that name.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|l| l.as_str() == text)
            .copied()
            .or_else(|| LayoutName::new(text).map(Self::Custom))
    }

    /// Whether the screen track is in the frame, so cursor and keystroke settings draw. A
    /// channel layout counts as showing it, whatever its slots.
    pub fn shows_screen(self) -> bool {
        !matches!(self, Self::Face)
    }

    /// The layout a take with only these tracks draws: content only without a camera, the
    /// face only without a screen. A take with neither keeps the layout, it has no picture.
    pub fn for_tracks(self, screen: bool, camera: bool) -> Self {
        match (screen, camera) {
            (true, false) if !matches!(self, Self::Content | Self::Screen | Self::Window) => {
                Self::Content
            }
            (false, true) => Self::Face,
            _ => self,
        }
    }
}

impl std::fmt::Display for Layout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromKdl for Layout {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let text = value
            .as_string()
            .ok_or_else(|| "expected a layout".to_owned())?;
        Self::parse(text).ok_or_else(|| {
            let names: Vec<&str> = Self::ALL.iter().map(|l| l.as_str()).collect();
            format!(
                "unknown layout `{text}`, expected one of {} or a channel layout's name",
                names.join(", ")
            )
        })
    }
}

impl ToKdl for Layout {
    fn write_kdl(&self, out: &mut String) {
        out.push_str(self.as_str());
    }
}

/// The name of a channel layout: lowercase letters, digits and dashes, starting with a letter,
/// at most [`LayoutName::MAX`] bytes and never a built-in layout's keyword. Stored inline, so a
/// [`Layout`] stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutName {
    len: u8,
    bytes: [u8; LayoutName::MAX],
}

impl LayoutName {
    pub const MAX: usize = 31;

    pub fn new(text: &str) -> Option<Self> {
        let builtin = Layout::ALL.iter().any(|l| l.as_str() == text);
        if builtin || text.len() > Self::MAX || crate::Id::new(text).is_none() {
            return None;
        }
        let mut bytes = [0; Self::MAX];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Some(Self {
            len: text.len() as u8,
            bytes,
        })
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).expect("names are ASCII")
    }
}

impl Ord for LayoutName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for LayoutName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Debug for LayoutName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LayoutName({:?})", self.as_str())
    }
}

impl std::fmt::Display for LayoutName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromKdl for LayoutName {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let text = String::from_kdl(value)?;
        Self::new(&text).ok_or_else(|| {
            format!(
                "`{text}` is not a layout name (lowercase letters, digits and dashes, starting \
                 with a letter, at most {} characters, not a built-in layout)",
                Self::MAX
            )
        })
    }
}

impl ToKdl for LayoutName {
    fn write_kdl(&self, out: &mut String) {
        out.push_str(self.as_str());
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
    /// The caption preset: how burned-in captions look before the brand and the video change
    /// single fields, see [`crate::CaptionLook`].
    pub enum CaptionStyle {
        /// The word being said on a pill in the highlight color.
        WordPop = "word-pop",
        /// Plain lines on a box.
        Clean = "clean",
        /// Words fill with the highlight color as they are said, on a box.
        Karaoke = "karaoke",
        /// Few large uppercase words with an outline, the word being said in color.
        Bold = "bold",
        Off = "off",
    }
}

/// `format.kdl`: pacing, layouts, captions, auto-cut thresholds and prompter mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    pub prompter: PrompterMode,
    /// The layout used wherever `cut.kdl` sets none. Written first in `layouts`.
    pub default_layout: Layout,
    /// Further layouts offered on the 1 to 4 hotkeys after the default.
    pub other_layouts: Vec<Layout>,
    pub captions: CaptionStyle,
    /// Changes to the caption preset's look for this video, over the brand's.
    pub caption_look: CaptionLook,
    /// The product video settings. Each is optional; `None` means the contract's default.
    pub zoom_motion: Option<ZoomMotion>,
    pub cursor: Option<CursorStyle>,
    pub keystrokes: Option<Keystrokes>,
    pub window: Option<WindowStyle>,
    /// How layout switches inside a take move where `cut.kdl` sets nothing. `None` means
    /// [`Transition::SWITCH`].
    pub layout_switch: Option<Transition>,
    /// The tracks a take records. `None` records every track.
    pub inputs: Option<Inputs>,
}

/// The tracks a new take records, switched on the Record screen. An off track is not captured
/// at all, so its file is missing from the take folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Inputs {
    pub screen: bool,
    pub camera: bool,
    pub mic: bool,
    pub system_audio: bool,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            screen: true,
            camera: true,
            mic: true,
            system_audio: true,
        }
    }
}

impl Inputs {
    /// Whether a take would record anything at all.
    pub fn any(self) -> bool {
        self.screen || self.camera || self.mic || self.system_audio
    }
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

/// A format file as one layer of the style cascade: a channel's `formats/<format>.kdl` or a
/// video's `format.kdl`, which holds only what differs from the channel's. Every setting is
/// optional, and the file may hold a `brand { ... }` override and style rules.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormatLayer {
    pub prompter: Option<PrompterMode>,
    /// The default layout first, then the hotkey layouts. Never empty when set.
    pub layouts: Option<Vec<Layout>>,
    pub captions: Option<CaptionStyle>,
    /// Look properties on the `captions` node, laid over the layers below field by field.
    pub caption_look: CaptionLook,
    pub zoom_motion: Option<ZoomMotion>,
    pub cursor: Option<CursorStyle>,
    pub keystrokes: Option<Keystrokes>,
    pub window: Option<WindowStyle>,
    pub layout_switch: Option<Transition>,
    pub inputs: Option<Inputs>,
    /// Fields of the brand this layer changes.
    pub brand: Option<Brand>,
    pub rules: StyleSheet,
}

impl FormatLayer {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let src = Source {
            file: "format.kdl",
            text,
        };
        let doc = src.parse()?;
        Self::read(src, &doc, None)
    }

    /// Reads a format file written before style rules, with its old settings apart.
    pub(crate) fn parse_old(text: &str) -> Result<(Self, OldSettings), ParseError> {
        let src = Source {
            file: "format.kdl",
            text,
        };
        let doc = src.parse()?;
        let mut old = OldSettings::default();
        let layer = Self::read(src, &doc, Some(&mut old))?;
        Ok((layer, old))
    }

    /// Reads the file. The old settings go to `old`, and without it are refused.
    fn read(
        src: Source<'_>,
        doc: &kdl::KdlDocument,
        mut old: Option<&mut OldSettings>,
    ) -> Result<Self, ParseError> {
        let mut layer = Self::default();
        let (mut layouts, mut brand) = (None, None);
        for node in src.versioned(doc)? {
            match node.name() {
                "prompter" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut layer.prompter, node.arg(0)?)?;
                }
                "layouts" => {
                    node.expect_list()?;
                    node.set_once(&mut layouts, node.args::<Layout>()?)?;
                }
                "captions" => {
                    node.expect(1, &LOOK_PROPS)?;
                    node.set_once(&mut layer.captions, node.arg(0)?)?;
                    layer.caption_look = CaptionLook::read(node)?;
                }
                name @ ("pacing" | "auto-cut" | "auto-zoom") => match old.as_deref_mut() {
                    Some(old) => {
                        old.read(node)?;
                    }
                    None => return Err(refused(node, &format!("`{name}`"))),
                },
                "zoom-motion" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut layer.zoom_motion, node.arg(0)?)?;
                }
                "cursor" => node.set_once(&mut layer.cursor, CursorStyle::read(node)?)?,
                "keystrokes" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut layer.keystrokes, node.arg(0)?)?;
                }
                "window" => node.set_once(&mut layer.window, WindowStyle::read(node)?)?,
                "layout-switch" => node.set_once(
                    &mut layer.layout_switch,
                    Transition::read(node, &["length", "sound"])?,
                )?,
                "record" => node.set_once(&mut layer.inputs, Inputs::read(node)?)?,
                "brand" => {
                    node.expect_block(0, &[])?;
                    let patch = Brand::read(node.children(), BrandScope::Patch)?;
                    if patch.is_empty() {
                        return Err(node.error("`brand` overrides at least one field"));
                    }
                    node.set_once(&mut brand, patch)?;
                }
                "on" => layer.rules.add(node)?,
                _ => return Err(node.unknown()),
            }
        }
        layer.layouts = layouts;
        layer.brand = brand;
        Ok(layer)
    }

    /// The settings of `base` with this layer's replacing them. Brand fields and rules are
    /// the cascade's business, not the resolved format's.
    pub fn apply(&self, base: &Format) -> Format {
        let mut format = base.clone();
        if let Some(prompter) = self.prompter {
            format.prompter = prompter;
        }
        if let Some(layouts) = &self.layouts {
            format.default_layout = layouts[0];
            format.other_layouts = layouts[1..].to_vec();
        }
        if let Some(captions) = self.captions {
            format.captions = captions;
        }
        format.caption_look = format.caption_look.merge(&self.caption_look);
        fn opt<T: Clone>(base: &mut Option<T>, layer: &Option<T>) {
            if layer.is_some() {
                base.clone_from(layer);
            }
        }
        opt(&mut format.zoom_motion, &self.zoom_motion);
        opt(&mut format.cursor, &self.cursor);
        opt(&mut format.keystrokes, &self.keystrokes);
        opt(&mut format.window, &self.window);
        opt(&mut format.layout_switch, &self.layout_switch);
        opt(&mut format.inputs, &self.inputs);
        format
    }

    /// A full format from a layer that sets every required node, or the first missing one.
    fn complete(self, src: Source<'_>) -> Result<Format, ParseError> {
        let missing = |message| src.error(0, message);
        let mut layouts = required(self.layouts, "layouts", missing)?.into_iter();
        Ok(Format {
            prompter: required(self.prompter, "prompter", missing)?,
            default_layout: layouts.next().expect("layouts has at least one argument"),
            other_layouts: layouts.collect(),
            captions: required(self.captions, "captions", missing)?,
            caption_look: self.caption_look,
            zoom_motion: self.zoom_motion,
            cursor: self.cursor,
            keystrokes: self.keystrokes,
            window: self.window,
            layout_switch: self.layout_switch,
            inputs: self.inputs,
        })
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        w.version();
        if let Some(prompter) = &self.prompter {
            w.node("prompter").arg(prompter).end();
        }
        if let Some(layouts) = &self.layouts {
            w.node("layouts");
            for layout in layouts {
                w.arg(layout);
            }
            w.end();
        }
        if let Some(captions) = &self.captions {
            w.node("captions").arg(captions);
            self.caption_look.write(&mut w);
            w.end();
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
        if let Some(switch) = &self.layout_switch {
            switch.write(w.node("layout-switch"));
        }
        if let Some(inputs) = &self.inputs {
            w.node("record")
                .prop("screen", &inputs.screen)
                .prop("camera", &inputs.camera)
                .prop("mic", &inputs.mic)
                .prop("system-audio", &inputs.system_audio)
                .end();
        }
        if let Some(brand) = &self.brand {
            w.node("brand").open();
            brand.write(&mut w);
            w.close();
        }
        self.rules.write(&mut w);
        w.finish()
    }
}

impl From<&Format> for FormatLayer {
    fn from(format: &Format) -> Self {
        Self {
            prompter: Some(format.prompter),
            layouts: Some(
                std::iter::once(format.default_layout)
                    .chain(format.other_layouts.iter().copied())
                    .collect(),
            ),
            captions: Some(format.captions),
            caption_look: format.caption_look.clone(),
            zoom_motion: format.zoom_motion,
            cursor: format.cursor,
            keystrokes: format.keystrokes,
            window: format.window,
            layout_switch: format.layout_switch.clone(),
            inputs: format.inputs,
            brand: None,
            rules: StyleSheet::default(),
        }
    }
}

impl Format {
    /// The settings after laying `layers` over this format, lowest layer first.
    pub fn resolve<'a>(&self, layers: impl IntoIterator<Item = &'a FormatLayer>) -> Self {
        layers
            .into_iter()
            .fold(self.clone(), |format, layer| layer.apply(&format))
    }

    /// How a layout switch moves where `cut.kdl` sets nothing.
    pub fn layout_switch(&self) -> Transition {
        self.layout_switch.clone().unwrap_or(Transition::SWITCH)
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
        FormatLayer::from(self).to_kdl()
    }

    /// The tracks a new take records, every one when the format says nothing.
    pub fn inputs(&self) -> Inputs {
        self.inputs.unwrap_or_default()
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

impl Inputs {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect(0, &["screen", "camera", "mic", "system-audio"])?;
        Ok(Self {
            screen: node.prop("screen")?,
            camera: node.prop("camera")?,
            mic: node.prop("mic")?,
            system_audio: node.prop("system-audio")?,
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
