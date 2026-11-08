//! Lottie files as brand templates: an import check, typed slots and rendering onto vello.
//!
//! velato draws a subset of After Effects, so [`check`] lists everything in a file it cannot
//! render and a [`LottieTemplate`] refuses such a file. Slots bind through Lottie's own slot ids
//! (`sid`), never layer names:
//!
//! - text: a top-level text layer whose text document carries the `sid`. velato has no text, so
//!   the layer becomes a null layer that keeps its transform, and [`Filled::texts`] says what to
//!   draw with parley and where.
//! - color and integer: an animated property with the `sid`, replaced by a static value. velato
//!   reads no alpha from colors, so the alpha of a color value is dropped.
//! - time: a marker named after the slot. The value is how long its span plays in media time.
//!
//! velato 0.12 renders onto vello 0.10, so it is built without its `vello` feature and drawn onto
//! vello 0.11 through [`SceneSink`].

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::panic::{self, AssertUnwindSafe};

use serde_json::{Value, json};
use shot_core::Micros;
use velato::model::fixed::{Brush, Stroke};
use velato::model::{ImageAsset, Layer};
use velato::{Composition, RenderSink, Renderer};
use vello::Scene;
use vello::kurbo::{Affine, Shape};
use vello::peniko::{BlendMode, Fill};

use crate::drawing::Rgba;

/// One thing in a Lottie file velato cannot render. Displays as
/// "layer `Logo`: expression on position: `wiggle(2, 30)`", or without a layer for file-level
/// findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub layer: Option<String>,
    pub feature: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.layer {
            Some(layer) => write!(f, "layer `{layer}`: {}", self.feature),
            None => f.write_str(&self.feature),
        }
    }
}

/// Every unsupported feature and layer in the file, top-level layers first and then precomps,
/// empty when it renders. A file velato cannot parse yields one finding with the parse error.
pub fn check(bytes: &[u8]) -> Vec<Finding> {
    analyze(bytes).err().unwrap_or_default()
}

/// The slots a file offers, by its Lottie slot ids, sorted by id: what a brand declares it with.
/// A file velato cannot render is refused with its findings, as [`LottieTemplate::load`] would.
/// An id on a property that is neither a color nor an integer is no slot and is left out.
pub fn slots(bytes: &[u8]) -> Result<Vec<(String, SlotKind)>, LottieError> {
    let (prepared, _) = analyze(bytes).map_err(LottieError::Unsupported)?;
    let mut slots: Vec<(String, SlotKind)> = prepared
        .texts
        .iter()
        .map(|t| (t.doc.slot.clone(), SlotKind::Text))
        .chain(
            prepared
                .colors
                .iter()
                .map(|b| (b.slot.clone(), SlotKind::Color)),
        )
        .chain(
            prepared
                .integers
                .iter()
                .map(|b| (b.slot.clone(), SlotKind::Integer)),
        )
        .chain(
            prepared
                .markers
                .iter()
                .map(|m| (m.slot.clone(), SlotKind::Time)),
        )
        .collect();
    slots.sort();
    slots.dedup();
    Ok(slots)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlotKind {
    Text,
    Color,
    Integer,
    Time,
}

impl fmt::Display for SlotKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Text => "text",
            Self::Color => "color",
            Self::Integer => "integer",
            Self::Time => "time",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotDecl {
    pub name: String,
    pub kind: SlotKind,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SlotValue {
    Text(String),
    Color(Rgba),
    Integer(i64),
    /// How long the slot's marker span plays.
    Time(Micros),
}

impl SlotValue {
    fn kind(&self) -> SlotKind {
        match self {
            Self::Text(_) => SlotKind::Text,
            Self::Color(_) => SlotKind::Color,
            Self::Integer(_) => SlotKind::Integer,
            Self::Time(_) => SlotKind::Time,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LottieError {
    #[error("the Lottie uses features velato cannot render: {}", join(.0))]
    Unsupported(Vec<Finding>),
    #[error("the file has no {kind} slot `{slot}`")]
    Unbound { slot: String, kind: SlotKind },
    #[error("slot `{slot}` takes a {expected} value")]
    Kind { slot: String, expected: SlotKind },
    #[error("required slot `{slot}` has no value")]
    Missing { slot: String },
    #[error("`{slot}` is not a declared slot")]
    UnknownSlot { slot: String },
    #[error("velato cannot read the Lottie: {0}")]
    Json(String),
}

fn join(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(Finding::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// A checked Lottie file with its slots bound, ready to fill.
#[derive(Debug)]
pub struct LottieTemplate {
    hash: [u8; 32],
    slots: Vec<SlotDecl>,
    base: Composition,
    prepared: Prepared,
}

impl LottieTemplate {
    /// Checks the file and binds every declared slot to the file's Lottie slot ids (`sid`),
    /// refusing a declared slot the file lacks or binds to a property of another kind.
    pub fn load(bytes: &[u8], slots: &[SlotDecl]) -> Result<Self, LottieError> {
        let (prepared, base) = analyze(bytes).map_err(LottieError::Unsupported)?;
        for decl in slots {
            if prepared.binds(&decl.name, decl.kind) {
                continue;
            }
            return Err(if prepared.mentions(&decl.name) {
                LottieError::Kind {
                    slot: decl.name.clone(),
                    expected: decl.kind,
                }
            } else {
                LottieError::Unbound {
                    slot: decl.name.clone(),
                    kind: decl.kind,
                }
            });
        }
        Ok(Self {
            hash: *blake3::hash(bytes).as_bytes(),
            slots: slots.to_vec(),
            base,
            prepared,
        })
    }

    /// blake3 of the file bytes: the file is its own version.
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }

    pub fn size(&self) -> (f64, f64) {
        (self.base.width as f64, self.base.height as f64)
    }

    /// Fills the slots (checks kinds, required and unknown names) and parses the result once.
    pub fn fill(&self, values: &[(&str, SlotValue)]) -> Result<Filled, LottieError> {
        let mut given = BTreeMap::new();
        for (name, value) in values {
            let decl = self
                .slots
                .iter()
                .find(|decl| decl.name == *name)
                .ok_or_else(|| LottieError::UnknownSlot {
                    slot: (*name).to_owned(),
                })?;
            if value.kind() != decl.kind {
                return Err(LottieError::Kind {
                    slot: decl.name.clone(),
                    expected: decl.kind,
                });
            }
            given.insert(decl.name.clone(), value.clone());
        }
        if let Some(decl) = self
            .slots
            .iter()
            .find(|decl| decl.required && !given.contains_key(&decl.name))
        {
            return Err(LottieError::Missing {
                slot: decl.name.clone(),
            });
        }

        let mut json = self.prepared.json.clone();
        for binding in &self.prepared.colors {
            if let Some(SlotValue::Color(Rgba([r, g, b, _]))) = given.get(&binding.slot) {
                binding.set(&mut json, json!([r, g, b]));
            }
        }
        for binding in &self.prepared.integers {
            if let Some(SlotValue::Integer(n)) = given.get(&binding.slot) {
                binding.set(&mut json, json!(n));
            }
        }
        let composition = parse(json).map_err(LottieError::Json)?;

        let texts = self
            .prepared
            .texts
            .iter()
            .map(|layer| {
                let text = match given.get(&layer.doc.slot) {
                    Some(SlotValue::Text(text)) => text.clone(),
                    _ => layer.doc.text.clone(),
                };
                (layer.clone(), text)
            })
            .collect();
        let spans = self
            .prepared
            .markers
            .iter()
            .filter_map(|marker| match given.get(&marker.slot) {
                Some(SlotValue::Time(time)) => Some((marker.frames.clone(), time.0 as f64)),
                _ => None,
            })
            .collect();

        Ok(Filled {
            template: self.hash,
            segments: segments(&self.base.frames, self.base.frame_rate, spans),
            values: given,
            composition,
            texts,
        })
    }
}

/// A template with its slots filled, ready to draw at any frame.
#[derive(Debug)]
pub struct Filled {
    template: [u8; 32],
    values: BTreeMap<String, SlotValue>,
    composition: Composition,
    texts: Vec<(TextLayer, String)>,
    segments: Vec<Segment>,
}

impl PartialEq for Filled {
    fn eq(&self, other: &Self) -> bool {
        self.template == other.template && self.values == other.values
    }
}

impl Filled {
    pub fn size(&self) -> (f64, f64) {
        (
            self.composition.width as f64,
            self.composition.height as f64,
        )
    }

    /// Media time since the template starts to a Lottie frame, with time slots applied.
    pub fn frame_at(&self, elapsed: Micros) -> f64 {
        let mut left = elapsed.0 as f64;
        for segment in &self.segments {
            if left < segment.micros {
                let Range { start, end } = segment.frames;
                return start + (end - start) * left / segment.micros;
            }
            left -= segment.micros;
        }
        self.composition.frames.end
    }

    /// How long it plays, with time slots applied.
    pub fn duration(&self) -> Micros {
        Micros(self.segments.iter().map(|s| s.micros).sum::<f64>().round() as u64)
    }

    /// Appends the shapes at `frame` to `scene` under `transform`.
    pub fn draw(
        &self,
        renderer: &mut Renderer,
        scene: &mut Scene,
        frame: f64,
        transform: Affine,
        alpha: f64,
    ) {
        renderer.append(
            &self.composition,
            frame,
            transform,
            alpha,
            &mut SceneSink(scene),
        );
    }

    /// The text slots visible at `frame`, topmost layer first: what to draw with parley and where.
    pub fn texts(&self, frame: f64) -> Vec<PlacedText> {
        let layers = &self.composition.layers;
        self.texts
            .iter()
            .filter_map(|(text, content)| {
                let layer = layers.get(text.layer)?;
                layer.frames.contains(&frame).then(|| PlacedText {
                    text: content.clone(),
                    size: text.doc.size,
                    color: text.doc.color,
                    justify: text.doc.justify,
                    transform: world_transform(layers, layer, frame),
                    opacity: (layer.opacity.evaluate(frame) / 100.0).clamp(0.0, 1.0) as f32,
                })
            })
            .collect()
    }
}

/// One text slot at a frame. The text starts at the layer's origin and is aligned by `justify`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedText {
    pub text: String,
    pub size: f32,
    pub color: Rgba,
    pub justify: Justify,
    /// The layer's transform through its parents, without the caller's transform.
    pub transform: Affine,
    pub opacity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Justify {
    Left,
    Right,
    Center,
}

/// Mirrors velato's private `compute_transform`.
fn world_transform(layers: &[Layer], layer: &Layer, frame: f64) -> Affine {
    let mut transform = layer.transform.evaluate(frame).into_owned();
    let mut parent = layer.parent;
    for _ in 0..layers.len() {
        let Some(next) = parent.and_then(|index| layers.get(index)) else {
            break;
        };
        transform = next.transform.evaluate(frame).into_owned() * transform;
        parent = next.parent;
    }
    transform
}

/// A stretch of the file's frames and the media time it plays for.
#[derive(Debug)]
struct Segment {
    frames: Range<f64>,
    micros: f64,
}

/// Splits the file's frames at the time slot spans. Spans play for the given micros, the frames
/// between them at the file's rate. Overlapping spans lose their overlap to the earlier one.
fn segments(file: &Range<f64>, rate: f64, mut spans: Vec<(Range<f64>, f64)>) -> Vec<Segment> {
    spans.sort_by(|a, b| a.0.start.total_cmp(&b.0.start));
    let natural = |frames: Range<f64>| Segment {
        micros: (frames.end - frames.start) / rate * 1e6,
        frames,
    };
    let mut out = Vec::new();
    let mut cursor = file.start;
    for (frames, micros) in spans {
        let frames = frames.start.max(cursor)..frames.end.min(file.end);
        if frames.start >= frames.end {
            continue;
        }
        if frames.start > cursor {
            out.push(natural(cursor..frames.start));
        }
        cursor = frames.end;
        out.push(Segment { frames, micros });
    }
    if cursor < file.end {
        out.push(natural(cursor..file.end));
    }
    out
}

/// Draws velato's output onto a vello 0.11 scene. Images are not loaded, [`check`] refuses them.
struct SceneSink<'a>(&'a mut Scene);

impl RenderSink for SceneSink<'_> {
    fn push_layer(
        &mut self,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        shape: &impl Shape,
    ) {
        self.0
            .push_layer(Fill::NonZero, blend, alpha, transform, shape);
    }

    fn push_clip_layer(&mut self, transform: Affine, shape: &impl Shape) {
        self.0.push_clip_layer(Fill::NonZero, transform, shape);
    }

    fn pop_layer(&mut self) {
        self.0.pop_layer();
    }

    fn draw(
        &mut self,
        stroke: Option<&Stroke>,
        transform: Affine,
        brush: &Brush,
        shape: &impl Shape,
    ) {
        match stroke {
            Some(stroke) => self.0.stroke(stroke, transform, brush, None, shape),
            None => self.0.fill(Fill::NonZero, transform, brush, None, shape),
        }
    }

    fn draw_image(&mut self, _: &ImageAsset, _: Affine, _: f64) {}
}

/// The file after normalizing and replacing text layers, with where its slots bind.
#[derive(Debug)]
struct Prepared {
    json: Value,
    texts: Vec<TextLayer>,
    colors: Vec<Binding>,
    integers: Vec<Binding>,
    /// Slot ids on properties that are neither a color nor an integer.
    others: Vec<String>,
    markers: Vec<Marker>,
}

impl Prepared {
    fn binds(&self, slot: &str, kind: SlotKind) -> bool {
        match kind {
            SlotKind::Text => self.texts.iter().any(|t| t.doc.slot == slot),
            SlotKind::Color => self.colors.iter().any(|b| b.slot == slot),
            SlotKind::Integer => self.integers.iter().any(|b| b.slot == slot),
            SlotKind::Time => self.markers.iter().any(|m| m.slot == slot),
        }
    }

    fn mentions(&self, slot: &str) -> bool {
        [SlotKind::Text, SlotKind::Color, SlotKind::Integer]
            .into_iter()
            .any(|kind| self.binds(slot, kind))
            || self.others.iter().any(|other| other == slot)
    }
}

#[derive(Debug)]
struct Binding {
    slot: String,
    pointer: String,
}

impl Binding {
    /// Makes the property static with `value`.
    fn set(&self, json: &mut Value, value: Value) {
        if let Some(property) = json.pointer_mut(&self.pointer) {
            property["a"] = json!(0);
            property["k"] = value;
        }
    }
}

#[derive(Debug, Clone)]
struct TextLayer {
    layer: usize,
    doc: TextDoc,
}

#[derive(Debug, Clone)]
struct TextDoc {
    slot: String,
    size: f32,
    color: Rgba,
    justify: Justify,
    text: String,
}

#[derive(Debug)]
struct Marker {
    slot: String,
    frames: Range<f64>,
}

/// Scans, prepares and parses the file. Either the findings that refuse it, or what a template
/// needs.
fn analyze(bytes: &[u8]) -> Result<(Prepared, Composition), Vec<Finding>> {
    let file = |feature: String| {
        vec![Finding {
            layer: None,
            feature,
        }]
    };
    let doc: Value =
        serde_json::from_slice(bytes).map_err(|err| file(format!("not valid JSON: {err}")))?;
    let mut scan = Scan::default();
    scan.layers(array(&doc, "layers"), false);
    for asset in array(&doc, "assets") {
        scan.layers(array(asset, "layers"), true);
    }
    if !scan.findings.is_empty() {
        return Err(scan.findings);
    }
    let prepared = prepare(doc);
    let composition = parse(prepared.json.clone()).map_err(file)?;
    Ok((prepared, composition))
}

/// velato panics on some inputs it does not model, which `check` must report instead.
fn parse(json: Value) -> Result<Composition, String> {
    panic::catch_unwind(AssertUnwindSafe(|| Composition::from_json(json)))
        .map_err(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            format!("velato cannot import the file: {message}")
        })?
        .map_err(|err| err.to_string())
}

fn prepare(mut json: Value) -> Prepared {
    let mut texts = Vec::new();
    if let Some(layers) = json.get_mut("layers").and_then(Value::as_array_mut) {
        normalize(layers, Some(&mut texts));
    }
    if let Some(assets) = json.get_mut("assets").and_then(Value::as_array_mut) {
        for asset in assets {
            if let Some(layers) = asset.get_mut("layers").and_then(Value::as_array_mut) {
                normalize(layers, None);
            }
        }
    }

    let mut prepared = Prepared {
        markers: array(&json, "markers")
            .iter()
            .filter_map(|marker| {
                let start = marker.get("tm")?.as_f64()?;
                let length = marker.get("dr").and_then(Value::as_f64).unwrap_or(0.0);
                Some(Marker {
                    slot: marker.get("cm")?.as_str()?.to_owned(),
                    frames: start..start + length,
                })
            })
            .collect(),
        json: Value::Null,
        texts,
        colors: Vec::new(),
        integers: Vec::new(),
        others: Vec::new(),
    };
    for key in ["layers", "assets"] {
        if let Some(value) = json.get(key) {
            collect_bindings(value, key, &format!("/{key}"), &mut prepared);
        }
    }
    prepared.json = json;
    prepared
}

/// Gives layer transforms without a rotation the zero rotation velato panics without, and turns
/// text slot layers (top level only, when `texts` is given) into null layers.
fn normalize(layers: &mut [Value], mut texts: Option<&mut Vec<TextLayer>>) {
    for (index, layer) in layers.iter_mut().enumerate() {
        if let Some(ks) = layer.get_mut("ks")
            && ks.get("r").is_none()
            && !has_split_rotation(ks)
        {
            ks["r"] = json!({ "a": 0, "k": 0 });
        }
        let Some(texts) = texts.as_deref_mut() else {
            continue;
        };
        if layer.get("ty").and_then(Value::as_u64) != Some(5) {
            continue;
        }
        let Some(doc) = text_doc(layer) else { continue };
        texts.push(TextLayer { layer: index, doc });
        let mut null = json!({ "ty": 3 });
        for key in ["ks", "ind", "parent", "ip", "op", "st", "nm"] {
            if let Some(value) = layer.get(key) {
                null[key] = value.clone();
            }
        }
        *layer = null;
    }
}

fn collect_bindings(value: &Value, key: &str, path: &str, out: &mut Prepared) {
    match value {
        Value::Object(map) => {
            if let Some(slot) = map.get("sid").and_then(Value::as_str) {
                let binding = Binding {
                    slot: slot.to_owned(),
                    pointer: path.to_owned(),
                };
                match property_kind(key, value) {
                    Some(SlotKind::Color) => out.colors.push(binding),
                    Some(SlotKind::Integer) => out.integers.push(binding),
                    _ => out.others.push(binding.slot),
                }
            }
            for (key, value) in map {
                collect_bindings(value, key, &format!("{path}/{key}"), out);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_bindings(item, key, &format!("{path}/{index}"), out);
            }
        }
        _ => {}
    }
}

/// A color is the `c` property of a fill, stroke or similar with an RGB value, an integer is a
/// scalar.
fn property_kind(key: &str, property: &Value) -> Option<SlotKind> {
    let first = if property.get("a").and_then(Value::as_u64) == Some(1) {
        property.pointer("/k/0/s")?
    } else {
        property.get("k")?
    };
    match first {
        Value::Number(_) => Some(SlotKind::Integer),
        Value::Array(items) if items.len() == 1 && items[0].is_number() => Some(SlotKind::Integer),
        Value::Array(_) if key == "c" && rgb(first).is_some() => Some(SlotKind::Color),
        _ => None,
    }
}

fn rgb(value: &Value) -> Option<Rgba> {
    let [r, g, b, ..] = value.as_array()?.as_slice() else {
        return None;
    };
    let channel = |v: &Value| v.as_f64().map(|v| v as f32);
    Some(Rgba([channel(r)?, channel(g)?, channel(b)?, 1.0]))
}

/// The text slot of a text layer: its `sid` and the first keyframe of its text document.
fn text_doc(layer: &Value) -> Option<TextDoc> {
    let doc = layer.pointer("/t/d")?;
    let style = doc.pointer("/k/0/s")?;
    Some(TextDoc {
        slot: doc.get("sid")?.as_str()?.to_owned(),
        size: style.get("s")?.as_f64()? as f32,
        text: style.get("t")?.as_str()?.to_owned(),
        justify: match style.get("j").and_then(Value::as_u64) {
            Some(1) => Justify::Right,
            Some(2) => Justify::Center,
            _ => Justify::Left,
        },
        color: style
            .get("fc")
            .and_then(rgb)
            .unwrap_or(Rgba([0.0, 0.0, 0.0, 1.0])),
    })
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn label(value: &Value) -> &str {
    ["nm", "mn"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .unwrap_or("unnamed")
}

fn has_split_rotation(transform: &Value) -> bool {
    ["rx", "ry", "rz"]
        .iter()
        .any(|key| transform.get(*key).is_some())
        || transform.get("r").is_some_and(|r| r.get("k").is_none())
}

/// Whether an optional property is the static `expected`.
fn is_static(property: Option<&Value>, expected: f64) -> bool {
    property.is_none_or(|p| {
        p.get("a").and_then(Value::as_u64).unwrap_or(0) == 0
            && p.get("k").and_then(Value::as_f64) == Some(expected)
    })
}

#[derive(Default)]
struct Scan {
    findings: Vec<Finding>,
    layer: Option<String>,
}

impl Scan {
    fn report(&mut self, feature: impl Into<String>) {
        self.findings.push(Finding {
            layer: self.layer.clone(),
            feature: feature.into(),
        });
    }

    fn layers(&mut self, layers: &[Value], nested: bool) {
        for (index, layer) in layers.iter().enumerate() {
            self.layer(index, layer, nested);
        }
        self.layer = None;
    }

    fn layer(&mut self, index: usize, layer: &Value, nested: bool) {
        self.layer = Some(
            layer
                .get("nm")
                .and_then(Value::as_str)
                .map_or_else(|| format!("#{index}"), str::to_owned),
        );
        match layer.get("ty").and_then(Value::as_u64) {
            Some(0 | 3 | 4) => {}
            Some(1) => self.report("solid layer: velato draws nothing for it"),
            Some(2) => self.report("image layer: images are not loaded"),
            Some(5) => self.text(layer, nested),
            Some(n) => self.report(format!("layer type {n} ({})", layer_type(n))),
            None => self.report("layer without a type"),
        }
        if layer.get("ddd").and_then(Value::as_u64) == Some(1) {
            self.report("3D layer");
        }
        if layer.get("ty").and_then(Value::as_u64) == Some(0) && layer.get("tm").is_some() {
            self.report("time remapping on a precomp");
        }
        if layer.get("ao").and_then(Value::as_u64) == Some(1) {
            self.report("auto-orient along the position path");
        }
        match layer.get("bm").and_then(Value::as_u64) {
            Some(16) => self.report("blend mode add"),
            Some(17) => self.report("blend mode hard mix"),
            _ => {}
        }
        if matches!(layer.get("tt").and_then(Value::as_u64), Some(3 | 4)) {
            self.report("luma matte: velato treats it as an alpha matte");
        }
        for effect in array(layer, "ef") {
            if effect.get("en").and_then(Value::as_u64) != Some(0) {
                self.report(format!("effect `{}`", label(effect)));
            }
        }
        for style in array(layer, "sy") {
            let name = style
                .get("ty")
                .and_then(Value::as_u64)
                .map_or("unknown", layer_style);
            self.report(format!("layer style: {name}"));
        }
        for mask in array(layer, "masksProperties") {
            self.mask(mask);
        }
        if let Some(ks) = layer.get("ks")
            && has_split_rotation(ks)
        {
            self.report("split rotation");
        }
        self.shapes(array(layer, "shapes"));
        self.properties("", layer, &mut false);
    }

    fn text(&mut self, layer: &Value, nested: bool) {
        let Some(slot) = layer.pointer("/t/d/sid").and_then(Value::as_str) else {
            return self.report("text layer: text renders only as a declared text slot");
        };
        if nested {
            self.report(format!(
                "text slot `{slot}` inside a precomp: only a top-level text layer can be a slot"
            ));
        } else if text_doc(layer).is_none() {
            self.report(format!(
                "text slot `{slot}`: the text document has no readable first keyframe"
            ));
        }
    }

    fn mask(&mut self, mask: &Value) {
        let mode = mask.get("mode").and_then(Value::as_str).unwrap_or("a");
        if mode != "a" {
            self.report(format!("mask mode `{mode}`: velato only adds masks"));
        }
        if mask.get("inv").and_then(Value::as_bool) == Some(true) {
            self.report("inverted mask");
        }
        if !is_static(mask.get("o"), 100.0) {
            self.report("mask opacity");
        }
        if !is_static(mask.get("x"), 0.0) {
            self.report("mask expansion");
        }
    }

    fn shapes(&mut self, items: &[Value]) {
        for item in items {
            let ty = item.get("ty").and_then(Value::as_str).unwrap_or_default();
            if item.get("hd").and_then(Value::as_bool) == Some(true) {
                self.report(format!(
                    "hidden shape `{}`: velato draws it anyway",
                    label(item)
                ));
            }
            match ty {
                "gr" => self.shapes(array(item, "it")),
                "mm" => self.report("merge paths"),
                "tw" => self.report("twist"),
                "pb" => self.report("pucker and bloat"),
                "op" => self.report("offset path"),
                "rd" => self.report("rounded corners"),
                "zz" => self.report("zig zag"),
                "tr" => {
                    if item.pointer("/p/s").and_then(Value::as_bool) == Some(true) {
                        self.report("split position in a shape transform");
                    }
                    if has_split_rotation(item) {
                        self.report("split rotation in a shape transform");
                    }
                }
                _ => {}
            }
            if matches!(ty, "st" | "gs") && !array(item, "d").is_empty() {
                self.report("stroke dashes");
            }
            if matches!(ty, "fl" | "gf") && item.get("r").and_then(Value::as_u64) == Some(2) {
                self.report("even-odd fill rule");
            }
            if matches!(ty, "gf" | "gs") && !is_static(item.get("o"), 100.0) {
                self.report("gradient opacity");
            }
        }
    }

    /// Expressions and spatial keyframe tangents anywhere below a layer. `key` names the
    /// property the value belongs to.
    fn properties(&mut self, key: &str, value: &Value, spatial: &mut bool) {
        match value {
            Value::Object(map) => {
                if let Some(expression) = map.get("x").and_then(Value::as_str)
                    && map.contains_key("k")
                {
                    self.report(format!(
                        "expression on {}: `{expression}`",
                        property_name(key)
                    ));
                }
                let tangent = |key| map.get(key).is_some_and(has_nonzero);
                if !*spatial && (tangent("ti") || tangent("to")) {
                    *spatial = true;
                    self.report("spatial easing on a position (`ti`, `to`)");
                }
                for (key, value) in map {
                    self.properties(key, value, spatial);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.properties(key, item, spatial);
                }
            }
            _ => {}
        }
    }
}

fn has_nonzero(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().any(|n| n.as_f64().is_some_and(|n| n != 0.0)))
}

fn property_name(key: &str) -> &str {
    match key {
        "a" => "anchor point",
        "p" => "position",
        "s" => "scale or size",
        "r" => "rotation",
        "o" => "opacity",
        "c" => "color",
        "w" => "stroke width",
        "sk" => "skew",
        other => other,
    }
}

fn layer_type(ty: u64) -> &'static str {
    match ty {
        6 => "audio",
        7 => "video placeholder",
        8 => "image sequence",
        9 => "video",
        10 => "image placeholder",
        11 => "guide",
        12 => "adjustment",
        13 => "camera",
        14 => "light",
        _ => "unknown",
    }
}

fn layer_style(ty: u64) -> &'static str {
    match ty {
        0 => "stroke",
        1 => "drop shadow",
        2 => "inner shadow",
        3 => "outer glow",
        4 => "inner glow",
        5 => "bevel and emboss",
        6 => "satin",
        7 => "color overlay",
        8 => "gradient overlay",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(k: Value) -> Value {
        json!({ "a": 0, "k": k })
    }

    fn transform() -> Value {
        json!({ "a": prop(json!([0, 0])), "p": prop(json!([100, 50])), "s": prop(json!([100, 100])), "o": prop(json!(100)) })
    }

    /// A 2 second, 30 fps file with a red square on a layer without a rotation. The fill color
    /// is the `accent` slot and the stroke width the `weight` slot.
    fn square() -> Value {
        json!({
            "v": "5.7.0", "fr": 30, "ip": 0, "op": 60, "w": 200, "h": 100, "layers": [{
                "ty": 4, "nm": "Square", "ind": 1, "ip": 0, "op": 60, "st": 0,
                "ks": transform(),
                "shapes": [{ "ty": "gr", "it": [
                    { "ty": "rc", "p": prop(json!([0, 0])), "s": prop(json!([40, 40])), "r": prop(json!(0)) },
                    { "ty": "fl", "c": { "a": 0, "k": [1, 0, 0], "sid": "accent" }, "o": prop(json!(100)) },
                    { "ty": "st", "c": prop(json!([0, 0, 1])), "o": prop(json!(100)),
                      "w": { "a": 0, "k": 2, "sid": "weight" } },
                    { "ty": "tr", "a": prop(json!([0, 0])), "p": prop(json!([0, 0])),
                      "s": prop(json!([100, 100])), "r": prop(json!(0)), "o": prop(json!(100)) }
                ] }]
            }]
        })
    }

    fn text_layer(slot: &str) -> Value {
        json!({
            "ty": 5, "nm": "Headline", "ind": 2, "ip": 0, "op": 60, "st": 0,
            "ks": {
                "a": prop(json!([0, 0])), "s": prop(json!([100, 100])), "o": prop(json!(100)),
                "p": { "a": 1, "k": [
                    { "t": 0, "s": [10, 20], "i": { "x": 1, "y": 1 }, "o": { "x": 0, "y": 0 } },
                    { "t": 30, "s": [110, 20] }
                ] }
            },
            "t": { "d": { "sid": slot, "k": [{ "t": 0, "s": {
                "s": 48, "t": "Hello", "j": 2, "fc": [1, 1, 1], "f": "Geist"
            } }] } }
        })
    }

    fn bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    fn decl(name: &str, kind: SlotKind) -> SlotDecl {
        SlotDecl {
            name: name.to_owned(),
            kind,
            required: false,
        }
    }

    fn with_layers(mut file: Value, extra: Vec<Value>) -> Value {
        file["layers"].as_array_mut().unwrap().extend(extra);
        file
    }

    /// The colors and stroke styles a filled template draws.
    fn encoded(filled: &Filled) -> (Vec<u32>, String) {
        let mut scene = Scene::new();
        filled.draw(&mut Renderer::new(), &mut scene, 0.0, Affine::IDENTITY, 1.0);
        let encoding = scene.encoding();
        (encoding.draw_data.clone(), format!("{:?}", encoding.styles))
    }

    #[test]
    fn an_expression_is_refused_and_named() {
        let mut file = square();
        file["layers"][0]["nm"] = json!("Logo");
        file["layers"][0]["ks"]["p"] = json!({ "a": 0, "k": [0, 0], "x": "wiggle(2, 30)" });
        let findings = check(&bytes(&file));
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].to_string(),
            "layer `Logo`: expression on position: `wiggle(2, 30)`"
        );
        assert!(matches!(
            LottieTemplate::load(&bytes(&file), &[]),
            Err(LottieError::Unsupported(_))
        ));
    }

    #[test]
    fn every_unsupported_feature_is_listed_in_one_check() {
        let mut file = square();
        file["layers"][0]["ef"] = json!([{ "nm": "Glow", "ty": 25 }]);
        file["layers"][0]["sy"] = json!([{ "ty": 1 }]);
        file["layers"][0]["bm"] = json!(16);
        file["layers"][0]["ddd"] = json!(1);
        file["layers"][0]["masksProperties"] =
            json!([{ "inv": true, "mode": "s", "pt": prop(json!({})) }]);
        file["layers"][0]["shapes"][0]["it"]
            .as_array_mut()
            .unwrap()
            .extend([
                json!({ "ty": "mm", "mm": 1 }),
                json!({ "ty": "rd", "r": prop(json!(4)) }),
                json!({ "ty": "st", "d": [{ "n": "d", "v": prop(json!(4)) }] }),
            ]);
        let file = with_layers(
            file,
            vec![
                json!({ "ty": 13, "nm": "Camera", "ip": 0, "op": 60 }),
                json!({ "ty": 5, "nm": "Caption", "ip": 0, "op": 60,
                    "t": { "d": { "k": [{ "t": 0, "s": { "s": 10, "t": "x" } }] } } }),
            ],
        );
        let features: Vec<String> = check(&bytes(&file))
            .iter()
            .map(Finding::to_string)
            .collect();
        for expected in [
            "layer `Square`: effect `Glow`",
            "layer `Square`: layer style: drop shadow",
            "layer `Square`: blend mode add",
            "layer `Square`: 3D layer",
            "layer `Square`: mask mode `s`: velato only adds masks",
            "layer `Square`: inverted mask",
            "layer `Square`: merge paths",
            "layer `Square`: rounded corners",
            "layer `Square`: stroke dashes",
            "layer `Camera`: layer type 13 (camera)",
            "layer `Caption`: text layer: text renders only as a declared text slot",
        ] {
            assert!(
                features.iter().any(|f| f == expected),
                "{expected} in {features:#?}"
            );
        }
        assert_eq!(features.len(), 11);
    }

    #[test]
    fn a_simple_shape_passes_loads_and_draws() {
        let file = bytes(&square());
        assert!(check(&file).is_empty());
        let template = LottieTemplate::load(&file, &[]).unwrap();
        assert_eq!(template.size(), (200.0, 100.0));
        let filled = template.fill(&[]).unwrap();
        let mut scene = Scene::new();
        filled.draw(
            &mut Renderer::new(),
            &mut scene,
            10.0,
            Affine::IDENTITY,
            1.0,
        );
        assert!(!scene.encoding().is_empty());
    }

    #[test]
    fn a_slot_the_file_lacks_or_binds_to_another_kind_is_refused() {
        let file = bytes(&with_layers(square(), vec![text_layer("headline")]));
        let load = |name, kind| LottieTemplate::load(&file, &[decl(name, kind)]);
        assert!(load("accent", SlotKind::Color).is_ok());
        assert!(load("headline", SlotKind::Text).is_ok());
        assert!(matches!(
            load("logo", SlotKind::Color),
            Err(LottieError::Unbound {
                kind: SlotKind::Color,
                ..
            })
        ));
        assert!(matches!(
            load("accent", SlotKind::Integer),
            Err(LottieError::Kind {
                expected: SlotKind::Integer,
                ..
            })
        ));
        assert!(matches!(
            load("beat", SlotKind::Time),
            Err(LottieError::Unbound {
                kind: SlotKind::Time,
                ..
            })
        ));
    }

    #[test]
    fn color_and_integer_slots_reach_the_scene() {
        let template = LottieTemplate::load(
            &bytes(&square()),
            &[
                decl("accent", SlotKind::Color),
                decl("weight", SlotKind::Integer),
            ],
        )
        .unwrap();
        let fill = |accent: Rgba, weight| {
            template
                .fill(&[
                    ("accent", SlotValue::Color(accent)),
                    ("weight", SlotValue::Integer(weight)),
                ])
                .unwrap()
        };
        let base = fill(Rgba::hex(0xff0000, 1.0), 2);
        assert_eq!(encoded(&base), encoded(&template.fill(&[]).unwrap()));
        assert_ne!(encoded(&base), encoded(&fill(Rgba::hex(0x00ff00, 1.0), 2)));
        assert_ne!(encoded(&base), encoded(&fill(Rgba::hex(0xff0000, 1.0), 20)));
        assert_ne!(base, fill(Rgba::hex(0x00ff00, 1.0), 2));
    }

    #[test]
    fn fill_checks_kinds_required_and_unknown_slots() {
        let required = SlotDecl {
            required: true,
            ..decl("accent", SlotKind::Color)
        };
        let template = LottieTemplate::load(&bytes(&square()), &[required]).unwrap();
        let red = SlotValue::Color(Rgba::hex(0xff0000, 1.0));
        assert!(matches!(
            template.fill(&[]),
            Err(LottieError::Missing { .. })
        ));
        assert!(matches!(
            template.fill(&[("accent", SlotValue::Integer(1))]),
            Err(LottieError::Kind {
                expected: SlotKind::Color,
                ..
            })
        ));
        assert!(matches!(
            template.fill(&[("accent", red.clone()), ("other", red)]),
            Err(LottieError::UnknownSlot { .. })
        ));
    }

    #[test]
    fn a_text_slot_follows_its_layer() {
        let file = bytes(&with_layers(square(), vec![text_layer("headline")]));
        let template = LottieTemplate::load(&file, &[decl("headline", SlotKind::Text)]).unwrap();

        let default = template.fill(&[]).unwrap().texts(0.0);
        assert_eq!(default[0].text, "Hello");

        let filled = template
            .fill(&[("headline", SlotValue::Text("Ship it".to_owned()))])
            .unwrap();
        let [start] = filled.texts(0.0).try_into().unwrap();
        let [end] = filled.texts(30.0).try_into().unwrap();
        assert_eq!(start.text, "Ship it");
        assert_eq!(start.size, 48.0);
        assert_eq!(start.justify, Justify::Center);
        assert_eq!(start.color, Rgba([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(start.transform.translation(), (10.0, 20.0).into());
        assert_eq!(end.transform.translation(), (110.0, 20.0).into());
    }

    #[test]
    fn slots_lists_every_slot_of_a_file_by_id() {
        let mut file = with_layers(square(), vec![text_layer("title")]);
        file["markers"] = json!([{ "cm": "hold", "tm": 30, "dr": 30 }]);
        assert_eq!(
            slots(&bytes(&file)).unwrap(),
            [
                ("accent".to_owned(), SlotKind::Color),
                ("hold".to_owned(), SlotKind::Time),
                ("title".to_owned(), SlotKind::Text),
                ("weight".to_owned(), SlotKind::Integer),
            ]
        );
        let mut refused = square();
        refused["layers"][0]["ks"]["p"]["x"] = json!("wiggle(2, 30)");
        assert!(matches!(
            slots(&bytes(&refused)),
            Err(LottieError::Unsupported(_))
        ));
    }

    #[test]
    fn a_time_slot_stretches_its_marker_span() {
        let mut file = square();
        file["markers"] = json!([{ "cm": "hold", "tm": 30, "dr": 30 }]);
        let template =
            LottieTemplate::load(&bytes(&file), &[decl("hold", SlotKind::Time)]).unwrap();

        let natural = template.fill(&[]).unwrap();
        assert_eq!(natural.duration(), Micros(2_000_000));
        assert_eq!(natural.frame_at(Micros(1_500_000)), 45.0);

        let held = template
            .fill(&[("hold", SlotValue::Time(Micros(2_000_000)))])
            .unwrap();
        assert_eq!(held.duration(), Micros(3_000_000));
        assert_eq!(held.frame_at(Micros(500_000)), 15.0);
        assert_eq!(held.frame_at(Micros(1_000_000)), 30.0);
        assert_eq!(held.frame_at(Micros(2_000_000)), 45.0);
        assert_eq!(held.frame_at(Micros(3_000_000)), 60.0);
        assert_eq!(held.frame_at(Micros(9_000_000)), 60.0);
    }

    #[test]
    fn the_hash_follows_the_file_bytes() {
        let mut file = bytes(&square());
        let before = LottieTemplate::load(&file, &[]).unwrap().hash();
        let at = file.iter().position(|&b| b == b'S').unwrap();
        file[at] = b'T';
        assert_ne!(LottieTemplate::load(&file, &[]).unwrap().hash(), before);
    }
}
