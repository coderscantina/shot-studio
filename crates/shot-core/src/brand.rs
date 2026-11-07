//! `brand/brand.kdl`: the channel's look and sound ([0020], [0023]). Every node is optional, so
//! the same type is the partial `brand { ... }` override in a video's `format.kdl`, and
//! [`Brand::merge`] lays one over another.
//!
//! [0020]: ../../../docs/decisions/0020-channel-brand-and-shared-styles.md
//! [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md

use kdl::KdlValue;

use crate::kdl_io::{FromKdl, Node, Source, ToKdl, Writer, keyword_enum, quote};
use crate::{Color, FormatKind, Id, Micros, ParseError, Shadow, StyleSheet};

/// A style imported from a git repo at a commit, the lowest layer of the brand.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Import {
    pub url: String,
    /// The commit, recorded in the lock.
    pub rev: String,
    /// Which style of the repo: its `styles/<style>.kdl`, a brand file.
    pub style: Id,
}

keyword_enum! {
    /// The OFL fonts the app bundles, named bare.
    pub enum BundledFont {
        Inter = "inter",
        Geist = "geist",
        GeistMono = "geist-mono",
        JetbrainsMono = "jetbrains-mono",
    }
}

/// A bundled font by name, or a font file in the channel repo by path, hashed by content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Font {
    Bundled(BundledFont),
    File(String),
}

impl FromKdl for Font {
    fn from_kdl(value: &KdlValue) -> Result<Self, String> {
        let text = String::from_kdl(value)?;
        if text.contains('/') {
            return Ok(Self::File(text));
        }
        BundledFont::from_kdl(value)
            .map(Self::Bundled)
            .map_err(|e| format!("{e}, or a font file path"))
    }
}

impl ToKdl for Font {
    fn write_kdl(&self, out: &mut String) {
        match self {
            Self::Bundled(font) => font.write_kdl(out),
            Self::File(path) => quote(path, out),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fonts {
    pub heading: Option<Font>,
    pub body: Option<Font>,
    pub mono: Option<Font>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Colors {
    pub accent: Option<Color>,
    pub text: Option<Color>,
    pub card: Option<Color>,
    pub background: Option<Color>,
}

keyword_enum! {
    /// What an automatic sound plays on.
    pub enum SoundEffect {
        Zoom = "zoom",
        Card = "card",
        Click = "click",
        Transition = "transition",
        Pop = "pop",
    }
}

keyword_enum! {
    /// A phase of a move ([0023]). A sound without a phase plays when the effect starts.
    ///
    /// [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md
    pub enum Phase {
        Travel = "travel",
        Impact = "impact",
        Settle = "settle",
        Exit = "exit",
    }
}

/// A key of the `sounds` node: `card` or `card-settle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SoundKey {
    pub effect: SoundEffect,
    pub phase: Option<Phase>,
}

impl SoundKey {
    /// Every key, in canonical order.
    fn all() -> impl Iterator<Item = Self> {
        SoundEffect::ALL.iter().flat_map(|&effect| {
            std::iter::once(Self {
                effect,
                phase: None,
            })
            .chain(Phase::ALL.iter().map(move |&phase| Self {
                effect,
                phase: Some(phase),
            }))
        })
    }

    pub fn to_text(self) -> String {
        match self.phase {
            None => self.effect.as_str().to_owned(),
            Some(phase) => format!("{}-{}", self.effect, phase),
        }
    }
}

/// An intro or outro: a bumper file in `brand/media/`, a generated title, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookend {
    pub bumper: Option<String>,
    pub title: bool,
}

keyword_enum! {
    pub enum CameraShape {
        Rounded = "rounded",
        Circle = "circle",
        Square = "square",
    }
}

/// How the camera is framed wherever a layout shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Camera {
    pub shape: CameraShape,
    /// Corner radius in output pixels.
    pub corners: u16,
    /// Border width in output pixels, 0 for none.
    pub border: u16,
    pub shadow: Shadow,
}

keyword_enum! {
    /// The four motion presets, one per format by default ([0023]).
    ///
    /// [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md
    pub enum PresetName {
        Reactive = "reactive",
        Editorial = "editorial",
        Launch = "launch",
        Calm = "calm",
    }
}

/// Which preset each format uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Motion {
    pub reaction: Option<PresetName>,
    pub opinion: Option<PresetName>,
    pub product_tour: Option<PresetName>,
    pub tutorial: Option<PresetName>,
}

impl Motion {
    pub fn get(&self, format: FormatKind) -> Option<PresetName> {
        match format {
            FormatKind::Reaction => self.reaction,
            FormatKind::Opinion => self.opinion,
            FormatKind::ProductTour => self.product_tour,
            FormatKind::Tutorial => self.tutorial,
        }
    }

    fn get_mut(&mut self, format: FormatKind) -> &mut Option<PresetName> {
        match format {
            FormatKind::Reaction => &mut self.reaction,
            FormatKind::Opinion => &mut self.opinion,
            FormatKind::ProductTour => &mut self.product_tour,
            FormatKind::Tutorial => &mut self.tutorial,
        }
    }
}

/// The feel controls of one preset, each 0 to 100. Missing ones keep the preset's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: PresetName,
    pub tempo: Option<u8>,
    pub bounce: Option<u8>,
    pub density: Option<u8>,
    pub stagger: Option<u8>,
    pub beat_sync: Option<bool>,
}

/// The brand's Lottie stinger and the time its frame fully covers the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stinger {
    pub file: String,
    pub peak: Micros,
}

keyword_enum! {
    pub enum SlotType {
        Text = "text",
        Color = "color",
        Integer = "integer",
        Time = "time",
    }
}

/// A typed slot of a Lottie template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub name: Id,
    pub kind: SlotType,
    pub required: bool,
}

/// A Lottie template declared by the brand: its file and typed slots, the spec the renderer
/// checks overlays against. Declaring a built-in template's name re-skins it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LottieTemplate {
    pub name: Id,
    pub file: String,
    pub slots: Vec<Slot>,
}

/// The brand, or a partial override of one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Brand {
    pub imports: Vec<Import>,
    pub fonts: Fonts,
    pub colors: Colors,
    pub logo: Option<String>,
    /// Sorted by key, each key once.
    pub sounds: Vec<(SoundKey, Id)>,
    pub intro: Option<Bookend>,
    pub outro: Option<Bookend>,
    pub end_card: Option<Micros>,
    pub camera: Option<Camera>,
    pub motion: Motion,
    /// Sorted by name, each name once.
    pub presets: Vec<Preset>,
    pub stinger: Option<Stinger>,
    /// Sorted by name, each name once.
    pub templates: Vec<LottieTemplate>,
    pub rules: StyleSheet,
}

/// Which nodes a brand block may hold.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrandScope {
    /// `brand/brand.kdl`: everything.
    File,
    /// `brand { ... }` in a format file: no imports and no rules, those are the layer's own.
    Patch,
}

impl Brand {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let src = Source {
            file: "brand.kdl",
            text,
        };
        let doc = src.parse()?;
        Self::read(src.nodes(&doc), BrandScope::File)
    }

    pub(crate) fn read<'a>(
        nodes: impl Iterator<Item = Node<'a>>,
        scope: BrandScope,
    ) -> Result<Self, ParseError> {
        let mut brand = Self::default();
        let (mut fonts, mut colors, mut logo, mut sounds, mut intro, mut outro) =
            (None, None, None, None, None, None);
        let (mut end_card, mut camera, mut motion, mut stinger) = (None, None, None, None);
        for node in nodes {
            match node.name() {
                "import" if scope == BrandScope::File => {
                    node.expect(1, &["rev", "style"])?;
                    brand.imports.push(Import {
                        url: node.arg(0)?,
                        rev: node.prop("rev")?,
                        style: node.prop("style")?,
                    });
                }
                "fonts" => {
                    node.expect(0, &["heading", "body", "mono"])?;
                    node.set_once(
                        &mut fonts,
                        Fonts {
                            heading: node.opt_prop("heading")?,
                            body: node.opt_prop("body")?,
                            mono: node.opt_prop("mono")?,
                        },
                    )?;
                }
                "colors" => {
                    node.expect(0, &["accent", "text", "card", "background"])?;
                    node.set_once(
                        &mut colors,
                        Colors {
                            accent: node.opt_prop("accent")?,
                            text: node.opt_prop("text")?,
                            card: node.opt_prop("card")?,
                            background: node.opt_prop("background")?,
                        },
                    )?;
                }
                "logo" => {
                    node.expect(1, &[])?;
                    node.set_once(&mut logo, read_path(node)?)?;
                }
                "sounds" => node.set_once(&mut sounds, read_sounds(node)?)?,
                "intro" => node.set_once(&mut intro, Bookend::read(node)?)?,
                "outro" => node.set_once(&mut outro, Bookend::read(node)?)?,
                "end-card" => {
                    node.expect(0, &["length"])?;
                    node.set_once(&mut end_card, node.prop("length")?)?;
                }
                "camera" => {
                    node.expect(0, &["shape", "corners", "border", "shadow"])?;
                    node.set_once(
                        &mut camera,
                        Camera {
                            shape: node.prop("shape")?,
                            corners: node.prop("corners")?,
                            border: node.prop("border")?,
                            shadow: node.prop("shadow")?,
                        },
                    )?;
                }
                "motion" => {
                    let props: Vec<&str> = FormatKind::ALL.iter().map(|k| k.as_str()).collect();
                    node.expect(0, &props)?;
                    let mut read = Motion::default();
                    for &kind in FormatKind::ALL {
                        *read.get_mut(kind) = node.opt_prop(kind.as_str())?;
                    }
                    node.set_once(&mut motion, read)?;
                }
                "preset" => {
                    let preset = Preset::read(node)?;
                    if brand.presets.iter().any(|p| p.name == preset.name) {
                        return Err(node.error(format!("duplicate preset `{}`", preset.name)));
                    }
                    brand.presets.push(preset);
                }
                "transition" => {
                    node.expect(0, &["stinger", "peak"])?;
                    node.set_once(
                        &mut stinger,
                        Stinger {
                            file: node.prop("stinger")?,
                            peak: node.prop("peak")?,
                        },
                    )?;
                }
                "template" => {
                    let template = LottieTemplate::read(node)?;
                    if brand.templates.iter().any(|t| t.name == template.name) {
                        return Err(node.error(format!("duplicate template `{}`", template.name)));
                    }
                    brand.templates.push(template);
                }
                "on" if scope == BrandScope::File => brand.rules.add(node)?,
                _ => return Err(node.unknown()),
            }
        }
        brand.fonts = fonts.unwrap_or_default();
        brand.colors = colors.unwrap_or_default();
        brand.logo = logo;
        brand.sounds = sounds.unwrap_or_default();
        brand.intro = intro;
        brand.outro = outro;
        brand.end_card = end_card;
        brand.camera = camera;
        brand.motion = motion.unwrap_or_default();
        brand.presets.sort_by_key(|p| p.name);
        brand.stinger = stinger;
        brand.templates.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(brand)
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        self.write(&mut w);
        w.finish()
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        for import in &self.imports {
            w.node("import")
                .arg(&import.url)
                .prop("rev", &import.rev)
                .prop("style", &import.style)
                .end();
        }
        let f = &self.fonts;
        if f != &Fonts::default() {
            w.node("fonts");
            write_opt(w, "heading", &f.heading);
            write_opt(w, "body", &f.body);
            write_opt(w, "mono", &f.mono);
            w.end();
        }
        let c = &self.colors;
        if c != &Colors::default() {
            w.node("colors");
            write_opt(w, "accent", &c.accent);
            write_opt(w, "text", &c.text);
            write_opt(w, "card", &c.card);
            write_opt(w, "background", &c.background);
            w.end();
        }
        if let Some(logo) = &self.logo {
            w.node("logo").arg(logo).end();
        }
        if !self.sounds.is_empty() {
            w.node("sounds");
            for (key, sound) in &self.sounds {
                w.prop(&key.to_text(), sound);
            }
            w.end();
        }
        if let Some(intro) = &self.intro {
            intro.write(w.node("intro"));
        }
        if let Some(outro) = &self.outro {
            outro.write(w.node("outro"));
        }
        if let Some(length) = &self.end_card {
            w.node("end-card").prop("length", length).end();
        }
        if let Some(camera) = &self.camera {
            w.node("camera")
                .prop("shape", &camera.shape)
                .prop("corners", &camera.corners)
                .prop("border", &camera.border)
                .prop("shadow", &camera.shadow)
                .end();
        }
        if self.motion != Motion::default() {
            w.node("motion");
            for &kind in FormatKind::ALL {
                write_opt(w, kind.as_str(), &self.motion.get(kind));
            }
            w.end();
        }
        for preset in &self.presets {
            w.node("preset").arg(&preset.name);
            write_opt(w, "tempo", &preset.tempo);
            write_opt(w, "bounce", &preset.bounce);
            write_opt(w, "density", &preset.density);
            write_opt(w, "stagger", &preset.stagger);
            write_opt(w, "beat-sync", &preset.beat_sync);
            w.end();
        }
        if let Some(stinger) = &self.stinger {
            w.node("transition")
                .prop("stinger", &stinger.file)
                .prop("peak", &stinger.peak)
                .end();
        }
        for template in &self.templates {
            w.node("template")
                .arg(&template.name)
                .prop("file", &template.file);
            if template.slots.is_empty() {
                w.end();
                continue;
            }
            w.open();
            for slot in &template.slots {
                w.node("slot").arg(&slot.name).arg(&slot.kind);
                if slot.required {
                    w.prop("required", &true);
                }
                w.end();
            }
            w.close();
        }
        self.rules.write(w);
    }

    /// Whether no node is set, so a `brand` block would be empty.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// `over` laid on this brand: its fields replace, field by field inside `fonts`, `colors`,
    /// `sounds`, `motion` and each `preset`; a template or bookend replaces whole. Imports are
    /// concatenated and rules are not merged, the cascade orders those as layers.
    pub fn merge(&self, over: &Self) -> Self {
        let mut merged = self.clone();
        merged.imports.extend(over.imports.iter().cloned());
        merge_opt(&mut merged.fonts.heading, &over.fonts.heading);
        merge_opt(&mut merged.fonts.body, &over.fonts.body);
        merge_opt(&mut merged.fonts.mono, &over.fonts.mono);
        merge_opt(&mut merged.colors.accent, &over.colors.accent);
        merge_opt(&mut merged.colors.text, &over.colors.text);
        merge_opt(&mut merged.colors.card, &over.colors.card);
        merge_opt(&mut merged.colors.background, &over.colors.background);
        merge_opt(&mut merged.logo, &over.logo);
        for (key, sound) in &over.sounds {
            match merged.sounds.binary_search_by_key(key, |(k, _)| *k) {
                Ok(i) => merged.sounds[i].1 = sound.clone(),
                Err(i) => merged.sounds.insert(i, (*key, sound.clone())),
            }
        }
        merge_opt(&mut merged.intro, &over.intro);
        merge_opt(&mut merged.outro, &over.outro);
        merge_opt(&mut merged.end_card, &over.end_card);
        merge_opt(&mut merged.camera, &over.camera);
        for &kind in FormatKind::ALL {
            merge_opt(merged.motion.get_mut(kind), &over.motion.get(kind));
        }
        for preset in &over.presets {
            match merged
                .presets
                .binary_search_by_key(&preset.name, |p| p.name)
            {
                Ok(i) => {
                    let base = &mut merged.presets[i];
                    merge_opt(&mut base.tempo, &preset.tempo);
                    merge_opt(&mut base.bounce, &preset.bounce);
                    merge_opt(&mut base.density, &preset.density);
                    merge_opt(&mut base.stagger, &preset.stagger);
                    merge_opt(&mut base.beat_sync, &preset.beat_sync);
                }
                Err(i) => merged.presets.insert(i, preset.clone()),
            }
        }
        merge_opt(&mut merged.stinger, &over.stinger);
        for template in &over.templates {
            match merged
                .templates
                .binary_search_by(|t| t.name.cmp(&template.name))
            {
                Ok(i) => merged.templates[i] = template.clone(),
                Err(i) => merged.templates.insert(i, template.clone()),
            }
        }
        merged.rules = StyleSheet::default();
        merged
    }

    /// The files in the channel repo the brand draws from, sorted and unique: font files, the
    /// logo, the stinger and the Lottie templates. Bumpers are media and not listed.
    pub fn assets(&self) -> Vec<&str> {
        let fonts = [&self.fonts.heading, &self.fonts.body, &self.fonts.mono]
            .into_iter()
            .filter_map(|font| match font {
                Some(Font::File(path)) => Some(path.as_str()),
                _ => None,
            });
        let mut assets: Vec<&str> = fonts
            .chain(self.logo.as_deref())
            .chain(self.stinger.as_ref().map(|s| s.file.as_str()))
            .chain(self.templates.iter().map(|t| t.file.as_str()))
            .collect();
        assets.sort_unstable();
        assets.dedup();
        assets
    }
}

fn merge_opt<T: Clone>(base: &mut Option<T>, over: &Option<T>) {
    if over.is_some() {
        base.clone_from(over);
    }
}

fn write_opt(w: &mut Writer, key: &str, value: &Option<impl ToKdl>) {
    if let Some(value) = value {
        w.prop(key, value);
    }
}

fn read_path(node: Node<'_>) -> Result<String, ParseError> {
    let path: String = node.arg(0)?;
    if path.is_empty() {
        return Err(node.error(format!("`{}` names a file", node.name())));
    }
    Ok(path)
}

fn read_sounds(node: Node<'_>) -> Result<Vec<(SoundKey, Id)>, ParseError> {
    let keys: Vec<(SoundKey, String)> = SoundKey::all().map(|k| (k, k.to_text())).collect();
    let props: Vec<&str> = keys.iter().map(|(_, text)| text.as_str()).collect();
    node.expect(0, &props)?;
    let mut sounds = Vec::new();
    for (key, text) in &keys {
        if let Some(sound) = node.opt_prop::<Id>(text)? {
            sounds.push((*key, sound));
        }
    }
    if sounds.is_empty() {
        return Err(node.error("`sounds` names at least one sound"));
    }
    Ok(sounds)
}

impl Bookend {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect(0, &["bumper", "title"])?;
        let bookend = Self {
            bumper: node.opt_prop("bumper")?,
            title: node.opt_prop("title")?.unwrap_or(false),
        };
        if bookend.bumper.as_deref().is_some_and(str::is_empty) {
            return Err(node.error("`bumper` names a file"));
        }
        if bookend.bumper.is_none() && !bookend.title {
            return Err(node.error(format!("`{}` needs a bumper, a title or both", node.name())));
        }
        Ok(bookend)
    }

    fn write(&self, w: &mut Writer) {
        write_opt(w, "bumper", &self.bumper);
        if self.title {
            w.prop("title", &true);
        }
        w.end();
    }
}

impl Preset {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect(1, &["tempo", "bounce", "density", "stagger", "beat-sync"])?;
        let control = |key: &str| -> Result<Option<u8>, ParseError> {
            let value: Option<u8> = node.opt_prop(key)?;
            if value.is_some_and(|v| v > 100) {
                return Err(node.error(format!("`{key}` is 0 to 100")));
            }
            Ok(value)
        };
        Ok(Self {
            name: node.arg(0)?,
            tempo: control("tempo")?,
            bounce: control("bounce")?,
            density: control("density")?,
            stagger: control("stagger")?,
            beat_sync: node.opt_prop("beat-sync")?,
        })
    }
}

impl LottieTemplate {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_block(1, &["file"])?;
        let mut slots: Vec<Slot> = Vec::new();
        for child in node.children() {
            if child.name() != "slot" {
                return Err(child.unknown());
            }
            child.expect(2, &["required"])?;
            let slot = Slot {
                name: child.arg(0)?,
                kind: child.arg(1)?,
                required: child.opt_prop("required")?.unwrap_or(false),
            };
            if slots.iter().any(|s| s.name == slot.name) {
                return Err(child.error(format!("duplicate slot `{}`", slot.name)));
            }
            slots.push(slot);
        }
        let file: String = node.prop("file")?;
        if file.is_empty() {
            return Err(node.error("`file` names a Lottie file"));
        }
        Ok(Self {
            name: node.arg(0)?,
            file,
            slots,
        })
    }
}
