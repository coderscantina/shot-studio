//! What the brand and the video give the overlay templates: the motion preset with the brand's
//! feel, colors, fonts, Lottie templates and re-skins, and the chapter titles a chapter band
//! shows. Read once per video from `cut.lock`, never from the style cascade ([0025]). Also what
//! the brand gives the rest of the frame: the camera's frame, the stinger and the logo.
//!
//! A video without a lock, or a lock whose brand leaves something out, draws the defaults: the
//! preset of its format (reaction reactive, opinion editorial, product tour launch, tutorial
//! calm), the lime accent on dark cards, and Geist for headings and body, Geist Mono for code.
//!
//! [0025]: ../../../docs/decisions/0025-lock-carries-the-resolved-style.md

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use shot_core::{
    Brand, BundledFont, CameraShape, Cut, FormatKind, Id, Layout, Micros, Overlay, ParamValue,
    PresetName, RangeEdit, SlotType,
};
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

use crate::drawing::{Font, Rgba};
use crate::layout::CameraFrame;
use crate::lottie::{Filled, LottieError, LottieTemplate, SlotDecl, SlotKind, SlotValue};
use crate::motion::{Feel, Motion, Moves, Preset};
use crate::overlay::{self, Params, TemplateError};
use crate::typeface::Face;

/// The colors templates draw in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub accent: Rgba,
    pub text: Rgba,
    pub card: Rgba,
    pub background: Rgba,
}

impl Palette {
    /// Secondary text: the text color, dimmed.
    pub fn muted(&self) -> Rgba {
        self.text.faded(0.62)
    }

    /// Text on the accent color: the background color, opaque.
    pub fn on_accent(&self) -> Rgba {
        let [r, g, b, _] = self.background.0;
        Rgba([r, g, b, 1.0])
    }

    /// A palette color by its brand name.
    fn named(&self, name: &str) -> Option<Rgba> {
        Some(match name {
            "accent" => self.accent,
            "text" => self.text,
            "card" => self.card,
            "background" => self.background,
            _ => return None,
        })
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            accent: Rgba::hex(0xC8F25A, 1.0),
            text: Rgba::hex(0xF0F3F6, 1.0),
            card: Rgba::hex(0x16181D, 1.0),
            background: Rgba::hex(0x0F1115, 1.0),
        }
    }
}

/// The fonts templates draw in, each at the weight its role uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Typefaces {
    pub heading: Face,
    pub body: Face,
    pub mono: Face,
}

impl Typefaces {
    pub fn heading(&self) -> Font {
        Font::Face(self.heading)
    }

    pub fn body(&self) -> Font {
        Font::Face(self.body)
    }

    pub fn mono(&self) -> Font {
        Font::Face(self.mono)
    }
}

const HEADING_WEIGHT: u16 = 700;
const BODY_WEIGHT: u16 = 500;
const MONO_WEIGHT: u16 = 400;

impl Default for Typefaces {
    fn default() -> Self {
        Self {
            heading: Face::bundled(BundledFont::Geist, HEADING_WEIGHT),
            body: Face::bundled(BundledFont::Geist, BODY_WEIGHT),
            mono: Face::bundled(BundledFont::GeistMono, MONO_WEIGHT),
        }
    }
}

/// Why a video's theme could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error(transparent)]
    Lock(#[from] crate::lock::LockError),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: shot_core::ParseError,
    },
    #[error("brand template `{template}` ({file}): {source}")]
    Lottie {
        template: String,
        file: String,
        source: LottieError,
    },
    #[error("`{0}` is drawn by code and cannot be re-skinned")]
    NotReskinnable(String),
    #[error("re-skinned `{template}` declares slot `{slot}`, which it does not fill")]
    ReskinSlot { template: String, slot: String },
    #[error("stinger {file}: {source}")]
    Stinger { file: String, source: LottieError },
    #[error("logo {file}: {reason}")]
    Logo { file: String, reason: String },
}

/// The brand's stinger, filled and ready to draw.
#[derive(Debug)]
pub struct StingerAnimation {
    pub lottie: Arc<Filled>,
    /// When its frame fully covers the picture, after it starts.
    pub peak: Micros,
    /// The file's content hash.
    pub hash: [u8; 32],
}

/// The brand's logo as straight-alpha RGBA, rasterized once.
#[derive(Debug, Clone)]
pub struct Logo {
    pub image: ImageData,
    /// The file's content hash.
    pub hash: [u8; 32],
}

/// The longest side a logo is rasterized at.
const LOGO_SIZE: u32 = 1024;
/// The brand Lottie template a generated title plays as its logo reveal, when declared.
pub const LOGO_REVEAL: &str = "logo-reveal";

/// A Lottie template the brand declares, loaded and checked.
#[derive(Debug)]
struct BrandTemplate {
    lottie: LottieTemplate,
    slots: Vec<SlotDecl>,
    /// Filled once per set of values.
    filled: Mutex<HashMap<FillKey, Arc<Filled>>>,
}

/// A set of slot values, as a cache key.
type FillKey = Vec<(String, SlotKey)>;

/// A slot value as a map key: colors by their bits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SlotKey {
    Text(String),
    Color([u32; 4]),
    Integer(i64),
    Time(u64),
}

impl SlotKey {
    fn of(value: &SlotValue) -> Self {
        match value {
            SlotValue::Text(text) => Self::Text(text.clone()),
            SlotValue::Color(c) => Self::Color(c.0.map(f32::to_bits)),
            SlotValue::Integer(n) => Self::Integer(*n),
            SlotValue::Time(t) => Self::Time(t.0),
        }
    }
}

impl BrandTemplate {
    fn fill(&self, values: Vec<(String, SlotValue)>) -> Result<Arc<Filled>, LottieError> {
        let key: FillKey = values
            .iter()
            .map(|(name, value)| (name.clone(), SlotKey::of(value)))
            .collect();
        let mut cache = self.filled.lock().expect("template cache poisoned");
        if let Some(filled) = cache.get(&key) {
            return Ok(filled.clone());
        }
        let refs: Vec<(&str, SlotValue)> = values
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let filled = Arc::new(self.lottie.fill(&refs)?);
        cache.insert(key, filled.clone());
        Ok(filled)
    }
}

/// A chapter's first kept moment in its first take, and its title.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChapterStart {
    take: Id,
    at: Micros,
    title: String,
}

/// See the module docs.
#[derive(Debug)]
pub struct Theme {
    pub preset: Preset,
    /// Whether entrances snap to the beat of the music playing, see [`crate::export`].
    pub beat_sync: bool,
    /// The preset's springs with the brand's feel.
    pub motion: Motion,
    pub moves: Moves,
    pub palette: Palette,
    pub fonts: Typefaces,
    templates: BTreeMap<String, BrandTemplate>,
    chapters: Vec<ChapterStart>,
    /// Everything above that every branded chunk depends on, for chunk keys.
    key: Vec<u8>,
    /// How the camera is framed beside the screen, `None` for the layouts' own corners.
    pub camera: Option<CameraFrame>,
    pub stinger: Option<StingerAnimation>,
    pub logo: Option<Logo>,
}

impl Default for Theme {
    /// An unbranded tutorial: the calm preset.
    fn default() -> Self {
        Self::new(&Brand::default(), FormatKind::Tutorial, None, &|_| {
            unreachable!("an empty brand names no files")
        })
        .expect("an empty brand reads no files")
    }
}

/// The preset a format uses when the brand names none.
pub fn default_preset(format: FormatKind) -> Preset {
    match format {
        FormatKind::Reaction => Preset::Reactive,
        FormatKind::Opinion => Preset::Editorial,
        FormatKind::ProductTour => Preset::Launch,
        FormatKind::Tutorial => Preset::Calm,
    }
}

fn preset_of(name: PresetName) -> Preset {
    match name {
        PresetName::Reactive => Preset::Reactive,
        PresetName::Editorial => Preset::Editorial,
        PresetName::Launch => Preset::Launch,
        PresetName::Calm => Preset::Calm,
    }
}

fn name_of(preset: Preset) -> PresetName {
    match preset {
        Preset::Reactive => PresetName::Reactive,
        Preset::Editorial => PresetName::Editorial,
        Preset::Launch => PresetName::Launch,
        Preset::Calm => PresetName::Calm,
    }
}

/// The channel repo of a video folder `<repo>/videos/<slug>`, which brand paths are relative to.
pub fn channel_root(video_dir: &Path) -> Option<&Path> {
    video_dir.parent()?.parent()
}

impl Theme {
    /// The theme of the video in `video_dir` with `format` and `cut`, from its `cut.lock`.
    pub fn load(video_dir: &Path, format: FormatKind, cut: &Cut) -> Result<Self, ThemeError> {
        let brand = crate::lock::read(video_dir)?
            .map(|lock| lock.brand)
            .unwrap_or_default();
        let root = channel_root(video_dir).unwrap_or(video_dir).to_owned();
        Self::new(&brand, format, Some(cut), &|file| {
            let path = root.join(file);
            std::fs::read(&path).map_err(|source| ThemeError::Io { path, source })
        })
    }

    /// [`Theme::load`] with the format and cut read from the folder's `video.kdl` and
    /// `cut.kdl`, for the preview.
    pub fn open(video_dir: &Path) -> Result<Self, ThemeError> {
        let read = |name: &str| {
            let path = video_dir.join(name);
            std::fs::read_to_string(&path)
                .map_err(|source| ThemeError::Io {
                    path: path.clone(),
                    source,
                })
                .map(|text| (text, path))
        };
        // A folder that is not a video yet, as in tests of other parts, draws the defaults.
        if !video_dir.join("video.kdl").exists() {
            return Ok(Self::default());
        }
        let (video, path) = read("video.kdl")?;
        let video =
            shot_core::Video::parse(&video).map_err(|source| ThemeError::Parse { path, source })?;
        let (cut, path) = read("cut.kdl")?;
        let cut = Cut::parse(&cut).map_err(|source| ThemeError::Parse { path, source })?;
        Self::load(video_dir, video.format, &cut)
    }

    /// The theme of a resolved `brand` for a video of `format`, reading the files the brand
    /// names through `read`.
    pub fn new(
        brand: &Brand,
        format: FormatKind,
        cut: Option<&Cut>,
        read: &dyn Fn(&str) -> Result<Vec<u8>, ThemeError>,
    ) -> Result<Self, ThemeError> {
        let preset = brand
            .motion
            .get(format)
            .map_or_else(|| default_preset(format), preset_of);
        let mut feel = preset.feel();
        let tuned = brand.presets.iter().find(|p| preset_of(p.name) == preset);
        let beat_sync =
            shot_audio::beats::beat_sync(name_of(preset), tuned.and_then(|tuned| tuned.beat_sync));
        if let Some(tuned) = tuned {
            let set = |value: &mut u8, over: Option<u8>| *value = over.unwrap_or(*value);
            set(&mut feel.tempo, tuned.tempo);
            set(&mut feel.bounce, tuned.bounce);
            set(&mut feel.density, tuned.density);
            set(&mut feel.stagger, tuned.stagger);
        }
        let color = |c: Option<shot_core::Color>, default: Rgba| {
            c.map_or(default, |shot_core::Color([r, g, b])| {
                Rgba::hex(u32::from_be_bytes([0, r, g, b]), 1.0)
            })
        };
        let base = Palette::default();
        let palette = Palette {
            accent: color(brand.colors.accent, base.accent),
            text: color(brand.colors.text, base.text),
            card: color(brand.colors.card, base.card),
            background: color(brand.colors.background, base.background),
        };
        let face = |font: &Option<shot_core::Font>, default: Face| -> Result<Face, ThemeError> {
            Ok(match font {
                None => default,
                Some(shot_core::Font::Bundled(font)) => Face::bundled(*font, default.weight),
                Some(shot_core::Font::File(path)) => {
                    Face::load(Arc::new(read(path)?)).at(default.weight)
                }
            })
        };
        let defaults = Typefaces::default();
        let fonts = Typefaces {
            heading: face(&brand.fonts.heading, defaults.heading)?,
            body: face(&brand.fonts.body, defaults.body)?,
            mono: face(&brand.fonts.mono, defaults.mono)?,
        };
        let mut templates = BTreeMap::new();
        for declared in &brand.templates {
            let name = declared.name.as_str();
            let slots: Vec<SlotDecl> = declared
                .slots
                .iter()
                .map(|slot| SlotDecl {
                    name: slot.name.to_string(),
                    kind: match slot.kind {
                        SlotType::Text => SlotKind::Text,
                        SlotType::Color => SlotKind::Color,
                        SlotType::Integer => SlotKind::Integer,
                        SlotType::Time => SlotKind::Time,
                    },
                    required: slot.required,
                })
                .collect();
            if overlay::spec(name).is_some() {
                let Some(fills) = overlay::reskin_slots(name) else {
                    return Err(ThemeError::NotReskinnable(name.to_owned()));
                };
                if let Some(slot) = slots
                    .iter()
                    .find(|s| !fills.contains(&(s.name.as_str(), s.kind)))
                {
                    return Err(ThemeError::ReskinSlot {
                        template: name.to_owned(),
                        slot: slot.name.clone(),
                    });
                }
            }
            let lottie =
                LottieTemplate::load(&read(&declared.file)?, &slots).map_err(|source| {
                    ThemeError::Lottie {
                        template: name.to_owned(),
                        file: declared.file.clone(),
                        source,
                    }
                })?;
            templates.insert(
                name.to_owned(),
                BrandTemplate {
                    lottie,
                    slots,
                    filled: Mutex::default(),
                },
            );
        }
        let chapters = cut.map(chapter_starts).unwrap_or_default();
        let camera = brand.camera.map(|camera| {
            let [r, g, b, _] = palette.accent.0;
            CameraFrame::of(&camera, [r, g, b])
        });
        let stinger = match &brand.stinger {
            None => None,
            Some(stinger) => {
                let error = |source| ThemeError::Stinger {
                    file: stinger.file.clone(),
                    source,
                };
                let lottie = LottieTemplate::load(&read(&stinger.file)?, &[]).map_err(error)?;
                Some(StingerAnimation {
                    lottie: Arc::new(lottie.fill(&[]).map_err(error)?),
                    peak: stinger.peak,
                    hash: lottie.hash(),
                })
            }
        };
        let logo = match &brand.logo {
            None => None,
            Some(file) => Some(load_logo(file, &read(file)?)?),
        };
        let mut theme = Self {
            preset,
            beat_sync,
            motion: Motion::new(feel),
            moves: preset.moves(),
            palette,
            fonts,
            templates,
            chapters,
            key: Vec::new(),
            camera,
            stinger,
            logo,
        };
        theme.key = theme.make_key(feel);
        Ok(theme)
    }

    fn make_key(&self, feel: Feel) -> Vec<u8> {
        let mut key = Vec::new();
        key.extend_from_slice(self.preset.name().as_bytes());
        key.extend_from_slice(&[feel.tempo, feel.bounce, feel.density, feel.stagger]);
        let p = &self.palette;
        for color in [p.accent, p.text, p.card, p.background] {
            for channel in color.0 {
                key.extend_from_slice(&channel.to_le_bytes());
            }
        }
        for face in [self.fonts.heading, self.fonts.body, self.fonts.mono] {
            key.extend_from_slice(&face.hash());
            key.extend_from_slice(&face.weight.to_le_bytes());
        }
        key
    }

    /// The preset, feel, colors and fonts as bytes: part of the key of every chunk where a
    /// branded template draws.
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// The camera's frame as bytes, for the chunks of a layout that shows the camera beside the
    /// screen; `None` where the brand frames no camera or the layout shows none to frame.
    pub fn camera_key(&self, layout: Layout) -> Option<Vec<u8>> {
        let camera = self.camera?;
        if !matches!(layout, Layout::FaceContent | Layout::Split) {
            return None;
        }
        let mut key = format!("{:?}{:?}", camera.shape, camera.shadow).into_bytes();
        for value in [camera.corners, camera.border]
            .into_iter()
            .chain(camera.color)
        {
            key.extend_from_slice(&value.to_le_bytes());
        }
        Some(key)
    }

    /// The shape a mask wipe opens in: the camera's, or a circle.
    pub fn wipe_shape(&self) -> CameraShape {
        self.camera.map_or(CameraShape::Circle, |c| c.shape)
    }

    /// What the end card and a generated title draw from the brand besides a template's own
    /// fields: the theme's key, the logo and the logo reveal.
    pub fn end_card_key(&self) -> Vec<u8> {
        let mut key = self.key.clone();
        key.extend_from_slice(&self.logo.as_ref().map_or([0; 32], |l| l.hash));
        key.extend_from_slice(&self.template_hash(LOGO_REVEAL).unwrap_or([0; 32]));
        key
    }

    /// The number, from 1, and title of the chapter an overlay of `take` starting at `at`
    /// opens: the chapter with the latest start in that take at or before `at`.
    pub fn chapter(&self, take: &Id, at: Micros) -> Option<(usize, &str)> {
        self.chapters
            .iter()
            .enumerate()
            .filter(|(_, c)| &c.take == take && c.at <= at)
            .max_by_key(|(_, c)| c.at)
            .map(|(i, c)| (i + 1, c.title.as_str()))
    }

    /// Whether the brand declares a Lottie template `name`, a new one or a re-skin.
    pub fn declares(&self, name: &str) -> bool {
        self.templates.contains_key(name)
    }

    /// Checks `overlay` against its template: a built-in spec, a brand Lottie template's slots,
    /// or both for a re-skin.
    pub fn check(&self, overlay: &Overlay, take: &Id) -> Result<(), TemplateError> {
        let name = overlay.template.as_str();
        if overlay::spec(name).is_some() {
            overlay::check(overlay)?;
            if let Some(sticker) = Params(overlay).text("sticker")
                && name == overlay::pop::TEMPLATE
                && !self.declares(sticker)
            {
                return Err(TemplateError::Lottie {
                    template: sticker.to_owned(),
                    error: "the brand declares no such sticker".to_owned(),
                });
            }
            if self.declares(name) {
                self.lottie(overlay, take)?;
            }
            return Ok(());
        }
        let template = self
            .templates
            .get(name)
            .ok_or_else(|| TemplateError::Unknown(name.to_owned()))?;
        self.slot_values(template, overlay)?;
        self.lottie(overlay, take).map(|_| ())
    }

    /// The filled brand Lottie an overlay draws, `None` when its template is code.
    pub(crate) fn lottie(
        &self,
        overlay: &Overlay,
        take: &Id,
    ) -> Result<Option<Arc<Filled>>, TemplateError> {
        let name = overlay.template.as_str();
        let Some(template) = self.templates.get(name) else {
            return Ok(None);
        };
        let values = if overlay::spec(name).is_some() {
            overlay::reskin_values(overlay, self, take)
                .into_iter()
                .filter(|(slot, _)| template.slots.iter().any(|s| s.name == *slot))
                .map(|(slot, value)| (slot.to_owned(), value))
                .collect()
        } else {
            self.slot_values(template, overlay)?
        };
        template
            .fill(values)
            .map(Some)
            .map_err(|error| TemplateError::Lottie {
                template: name.to_owned(),
                error: error.to_string(),
            })
    }

    /// A sticker: the brand template `name` with its `text` slot set when it has one.
    pub(crate) fn sticker(&self, name: &str, text: Option<&str>) -> Option<Arc<Filled>> {
        let template = self.templates.get(name)?;
        let mut values = self.palette_values(template);
        if let Some(text) = text
            && template
                .slots
                .iter()
                .any(|s| s.name == "text" && s.kind == SlotKind::Text)
        {
            values.push(("text".to_owned(), SlotValue::Text(text.to_owned())));
        }
        template.fill(values).ok()
    }

    /// The file hash of the brand Lottie `name`, for chunk keys.
    pub(crate) fn template_hash(&self, name: &str) -> Option<[u8; 32]> {
        self.templates.get(name).map(|t| t.lottie.hash())
    }

    /// Color slots named after a palette color, filled with it unless an overlay says otherwise.
    fn palette_values(&self, template: &BrandTemplate) -> Vec<(String, SlotValue)> {
        template
            .slots
            .iter()
            .filter(|s| s.kind == SlotKind::Color)
            .filter_map(|s| {
                Some((
                    s.name.clone(),
                    SlotValue::Color(self.palette.named(&s.name)?),
                ))
            })
            .collect()
    }

    /// An overlay's parameters as the slot values of a new brand template.
    fn slot_values(
        &self,
        template: &BrandTemplate,
        overlay: &Overlay,
    ) -> Result<Vec<(String, SlotValue)>, TemplateError> {
        let name = overlay.template.as_str();
        let mut values = self.palette_values(template);
        for (param, value) in &overlay.params {
            let slot = template
                .slots
                .iter()
                .find(|s| s.name == param.as_str())
                .ok_or_else(|| TemplateError::UnknownParam {
                    template: name.to_owned(),
                    param: param.to_string(),
                })?;
            let kind_error = || TemplateError::Kind {
                template: name.to_owned(),
                param: param.to_string(),
                kind: match slot.kind {
                    SlotKind::Text => "text",
                    SlotKind::Color => "a color, \"#rrggbb\"",
                    SlotKind::Integer => "an integer",
                    SlotKind::Time => "a time in microseconds",
                },
            };
            let filled = match (slot.kind, value) {
                (SlotKind::Text, ParamValue::Text(text)) => SlotValue::Text(text.clone()),
                (SlotKind::Color, ParamValue::Text(text)) => {
                    SlotValue::Color(hex_color(text).ok_or_else(kind_error)?)
                }
                (SlotKind::Integer, ParamValue::Integer(n)) => SlotValue::Integer(*n),
                (SlotKind::Time, ParamValue::Integer(n)) => {
                    SlotValue::Time(Micros(u64::try_from(*n).map_err(|_| kind_error())?))
                }
                _ => return Err(kind_error()),
            };
            values.retain(|(slot, _)| slot != param.as_str());
            values.push((param.to_string(), filled));
        }
        if let Some(missing) = template
            .slots
            .iter()
            .find(|s| s.required && !values.iter().any(|(name, _)| *name == s.name))
        {
            return Err(TemplateError::Missing {
                template: name.to_owned(),
                param: missing.name.clone(),
            });
        }
        Ok(values)
    }
}

/// A logo file, SVG or PNG, as an image no larger than [`LOGO_SIZE`].
fn load_logo(file: &str, bytes: &[u8]) -> Result<Logo, ThemeError> {
    let error = |reason: String| ThemeError::Logo {
        file: file.to_owned(),
        reason,
    };
    let mut rgba = if file.to_ascii_lowercase().ends_with(".svg") {
        let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default())
            .map_err(|e| error(e.to_string()))?;
        let size = tree.size();
        let scale = LOGO_SIZE as f32 / size.width().max(size.height());
        let (width, height) = (
            (size.width() * scale).round().max(1.0) as u32,
            (size.height() * scale).round().max(1.0) as u32,
        );
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| error("it has no size".into()))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        // tiny-skia holds premultiplied alpha; the drawing takes straight alpha.
        let mut data = pixmap.take();
        for pixel in data.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            if alpha > 0 && alpha < 255 {
                for channel in &mut pixel[..3] {
                    *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        image::RgbaImage::from_raw(width, height, data)
            .ok_or_else(|| error("the raster has the wrong size".into()))?
    } else {
        image::load_from_memory(bytes)
            .map_err(|e| error(e.to_string()))?
            .into_rgba8()
    };
    let (width, height) = rgba.dimensions();
    if width.max(height) > LOGO_SIZE {
        let scale = LOGO_SIZE as f32 / width.max(height) as f32;
        rgba = image::imageops::resize(
            &rgba,
            (width as f32 * scale).round().max(1.0) as u32,
            (height as f32 * scale).round().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        );
    }
    let (width, height) = rgba.dimensions();
    Ok(Logo {
        image: ImageData {
            data: Blob::new(Arc::new(rgba.into_raw())),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        },
        hash: *blake3::hash(bytes).as_bytes(),
    })
}

fn hex_color(text: &str) -> Option<Rgba> {
    let hex = text.strip_prefix('#').filter(|h| h.len() == 6)?;
    Some(Rgba::hex(u32::from_str_radix(hex, 16).ok()?, 1.0))
}

/// Each chapter's first kept moment in its first take.
fn chapter_starts(cut: &Cut) -> Vec<ChapterStart> {
    cut.chapters
        .iter()
        .filter_map(|chapter| {
            let first = chapter.takes.first()?;
            let at = first.ranges.iter().find_map(|r| match r {
                RangeEdit::Keep(range) => Some(range.from()),
                RangeEdit::Remove { .. } => None,
            })?;
            Some(ChapterStart {
                take: first.take.clone(),
                at,
                title: chapter.title.clone(),
            })
        })
        .collect()
}

/// What an overlay adds to the key of a chunk it overlaps, `None` when it draws nothing
/// branded: `blur`, `bleep`, b-roll clips and empty slots draw no brand color, font or motion.
/// A chapter band adds the title it shows and a brand Lottie its file's hash.
pub(crate) fn overlay_key(theme: &Theme, overlay: &Overlay, take: &Id) -> Option<Vec<u8>> {
    let name = overlay.template.as_str();
    let mut key = Vec::new();
    if let Some(hash) = theme.template_hash(name) {
        key.extend_from_slice(&hash);
    } else if !overlay::branded(overlay) {
        return None;
    }
    if name == overlay::chapter_band::TEMPLATE && Params(overlay).text("text").is_none() {
        let (number, title) = theme.chapter(take, overlay.range.from()).unwrap_or((0, ""));
        key.extend_from_slice(&(number as u64).to_le_bytes());
        key.extend_from_slice(&(title.len() as u64).to_le_bytes());
        key.extend_from_slice(title.as_bytes());
    }
    if name == overlay::pop::TEMPLATE
        && let Some(hash) = Params(overlay)
            .text("sticker")
            .and_then(|s| theme.template_hash(s))
    {
        key.extend_from_slice(&hash);
    }
    Some(key)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::plan::ExportPlan;
    use serde_json::json;
    use shot_core::{MediaId, Project, Timeline};

    /// A star with a `text` slot and an `accent` color slot.
    pub(crate) fn star() -> Vec<u8> {
        let prop = |k| json!({ "a": 0, "k": k });
        let file = json!({
            "v": "5.7.0", "fr": 60, "ip": 0, "op": 60, "w": 400, "h": 400, "layers": [
                {
                    "ty": 5, "nm": "Label", "ind": 1, "ip": 0, "op": 60, "st": 0,
                    "ks": { "a": prop(json!([0, 0])), "s": prop(json!([100, 100])),
                            "o": prop(json!(100)), "r": prop(json!(0)), "p": prop(json!([200, 200])) },
                    "t": { "d": { "sid": "text", "k": [{ "t": 0, "s": {
                        "s": 44, "t": "NEW", "j": 2, "fc": [0, 0, 0], "f": "Sans"
                    } }] } }
                },
                {
                    "ty": 4, "nm": "Star", "ind": 2, "ip": 0, "op": 60, "st": 0,
                    "ks": { "a": prop(json!([0, 0])), "s": prop(json!([100, 100])),
                            "o": prop(json!(100)), "r": prop(json!(0)), "p": prop(json!([200, 200])) },
                    "shapes": [{ "ty": "gr", "it": [
                        { "ty": "el", "p": prop(json!([0, 0])), "s": prop(json!([300, 300])) },
                        { "ty": "fl", "c": { "a": 0, "k": [1, 0.5, 0.3], "sid": "accent" },
                          "o": prop(json!(100)) },
                        { "ty": "tr", "a": prop(json!([0, 0])), "p": prop(json!([0, 0])),
                          "s": prop(json!([100, 100])), "r": prop(json!(0)), "o": prop(json!(100)) }
                    ] }]
                }
            ]
        });
        serde_json::to_vec(&file).unwrap()
    }

    fn brand(text: &str) -> Brand {
        Brand::parse(text).unwrap()
    }

    fn theme(brand: &Brand) -> Result<Theme, ThemeError> {
        let inter = include_bytes!("../assets/fonts/Inter.ttf");
        Theme::new(brand, FormatKind::Opinion, None, &|file| {
            Ok(match file {
                "brand/fonts/Display.ttf" => inter.to_vec(),
                "brand/star.json" => star(),
                other => panic!("no file {other}"),
            })
        })
    }

    #[test]
    fn the_brand_sets_preset_feel_colors_and_fonts() {
        let plain = Theme::default();
        let branded = theme(&brand(
            "fonts heading=\"brand/fonts/Display.ttf\" body=inter\n\
             colors accent=\"#ff7a59\"\n\
             motion opinion=launch\n\
             preset launch tempo=90\n",
        ))
        .unwrap();
        assert_eq!(plain.preset, Preset::Calm);
        assert_eq!(branded.preset, Preset::Launch);
        let launch = Preset::Launch.motion();
        assert!(branded.motion.enter.settle() < launch.enter.settle());
        assert_eq!(branded.motion.stagger, launch.stagger);
        assert_eq!(branded.palette.accent, Rgba::hex(0xFF7A59, 1.0));
        assert_eq!(branded.palette.text, plain.palette.text);
        let inter = Face::bundled(BundledFont::Inter, 700);
        assert_eq!(branded.fonts.heading.hash(), inter.hash());
        assert_eq!(branded.fonts.body, inter.at(BODY_WEIGHT));
        assert_eq!(branded.fonts.mono, plain.fonts.mono);
    }

    #[test]
    fn brand_templates_check_overlays_by_their_slots() {
        let theme = theme(&brand(
            "template star file=\"brand/star.json\" {\n    slot text text required=#true\n    slot accent color\n}\n\
             template lower-third file=\"brand/star.json\" {\n    slot text text\n}\n",
        ))
        .unwrap();
        let take = Id::new("t01").unwrap();
        let range = shot_core::TimeRange::new(Micros(0), Micros(1_000_000)).unwrap();
        let star = |params| overlay::overlay("star", range, params);
        let text = |s: &str| ParamValue::Text(s.into());
        assert!(
            theme
                .check(&star(vec![("text", text("hi"))]), &take)
                .is_ok()
        );
        assert!(
            theme
                .check(
                    &star(vec![("text", text("hi")), ("accent", text("#00ff00"))]),
                    &take
                )
                .is_ok()
        );
        assert!(matches!(
            theme.check(&star(vec![]), &take),
            Err(TemplateError::Missing { .. })
        ));
        assert!(matches!(
            theme.check(
                &star(vec![("text", text("hi")), ("size", ParamValue::Integer(2))]),
                &take
            ),
            Err(TemplateError::UnknownParam { .. })
        ));
        assert!(matches!(
            theme.check(
                &star(vec![("text", text("hi")), ("accent", text("lime"))]),
                &take
            ),
            Err(TemplateError::Kind { .. })
        ));
        // A re-skinned lower third is checked as a lower third and draws the Lottie.
        let lower = overlay::overlay("lower-third", range, vec![("text", text("Mike"))]);
        assert!(theme.check(&lower, &take).is_ok());
        assert!(theme.lottie(&lower, &take).unwrap().is_some());
        let pop = overlay::overlay("pop", range, vec![("sticker", text("nope"))]);
        assert!(matches!(
            theme.check(&pop, &take),
            Err(TemplateError::Lottie { .. })
        ));
        let code = brand("template highlight file=\"brand/star.json\"\n");
        assert!(matches!(
            self::theme(&code),
            Err(ThemeError::NotReskinnable(_))
        ));
        let slot = brand("template lower-third file=\"brand/star.json\" {\n    slot url text\n}\n");
        assert!(matches!(
            self::theme(&slot),
            Err(ThemeError::ReskinSlot { .. })
        ));
    }

    #[test]
    fn a_new_accent_rekeys_only_chunks_where_something_branded_draws() {
        let project = Project {
            video: shot_core::Video::parse("title \"t\"\nstatus editing\nformat opinion\n").unwrap(),
            script: shot_core::Script::parse("# t\n").unwrap(),
            format: shot_core::Format::parse(
                "prompter beats\npacing tight\nlayouts content\ncaptions off\n\
                 auto-cut confidence=85 {\n    silence on=#false min=600_000\n    retake on=#false\n    \
                 misspoken on=#false\n    pause on=#false min=1_000_000\n    filler on=#false\n    \
                 idle-screen on=#false min=8_000_000\n}\n",
            )
            .unwrap(),
            cut: shot_core::Cut::parse(
                "chapter \"Intro\" {\n    take t01 {\n        keep from=0 to=6_000_000\n        \
                 overlay chapter-band from=0 to=1_000_000\n        \
                 overlay blur from=2_000_000 to=3_000_000 {\n            x 0\n            y 0\n            width 10\n            height 10\n        }\n        \
                 overlay lower-third from=4_000_000 to=5_000_000 {\n            text \"Mike\"\n        }\n    }\n}\n",
            )
            .unwrap(),
        };
        let timeline = Timeline::evaluate(&project);
        let keys = |accent: &str| {
            let brand = brand(&format!("colors accent=\"{accent}\"\n"));
            let theme = Theme::new(
                &brand,
                FormatKind::Opinion,
                Some(&project.cut),
                &|_| unreachable!(),
            )
            .unwrap();
            assert_eq!(
                theme.chapter(&Id::new("t01").unwrap(), Micros(0)),
                Some((1, "Intro"))
            );
            let mut plan = ExportPlan::new(timeline.main(), 60, |_| MediaId::default());
            overlay::key_chunks(&mut plan, &|_| Vec::new(), &theme);
            plan.segments[0]
                .chunks
                .iter()
                .map(|c| c.hash)
                .collect::<Vec<_>>()
        };
        let (lime, coral) = (keys("#c8f25a"), keys("#ff7a59"));
        let changed: Vec<usize> = (0..lime.len()).filter(|&i| lime[i] != coral[i]).collect();
        // Half-second chunks: the chapter band in 0 and 1, the lower third in 8 and 9. The
        // blur in 4 and 5 and the bare chunks keep their keys.
        assert_eq!(changed, [0, 1, 8, 9]);
    }

    #[test]
    fn the_camera_frame_rekeys_only_chunks_that_show_the_camera_beside_the_screen() {
        let cut = shot_core::Cut::parse(
            "chapter \"One\" {\n    take t01 {\n        keep from=0 to=3_000_000\n        \
             layout face-content from=1_000_000 to=2_000_000\n    }\n}\n",
        )
        .unwrap();
        let project = Project {
            video: shot_core::Video::parse("title \"t\"\nstatus editing\nformat opinion\n")
                .unwrap(),
            script: shot_core::Script::parse("# t\n").unwrap(),
            format: FormatKind::Opinion.preset(),
            cut,
        };
        let timeline = Timeline::evaluate(&project);
        let keys = |brand_text: &str| {
            let theme = theme(&brand(brand_text)).unwrap();
            let mut plan = ExportPlan::new(timeline.main(), 60, |_| MediaId::default());
            overlay::key_chunks(&mut plan, &|_| Vec::new(), &theme);
            plan.output_chunks()
                .map(|(_, c)| c.hash)
                .collect::<Vec<_>>()
        };
        let plain = keys("");
        let framed = keys("camera shape=circle corners=24 border=8 shadow=soft\n");
        let changed: Vec<usize> = (0..plain.len())
            .filter(|&i| plain[i] != framed[i])
            .collect();
        // The face-content segment's two chunks; the opinion format's face layout around it
        // fills the canvas and has no frame.
        assert_eq!(changed, [2, 3]);
    }
}
