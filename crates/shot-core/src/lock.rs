//! `cut.lock`: every instance the style rules and overrides produced, with the element and
//! the rule that made it, in a stable order. A build writes it, git diffs it, the renderer and
//! the chunk hash read it, nobody edits it ([0019], [0025]).
//!
//! [0019]: ../../../docs/decisions/0019-semantic-video-elements-and-style.md
//! [0025]: ../../../docs/decisions/0025-lock-carries-the-resolved-style.md

use crate::brand::BrandScope;
use crate::kdl_io::{Node, Source, Writer, required};
use crate::{
    Brand, CaptionStyle, ContentHash, ElementKind, Id, Import, Layer, Layout, Micros, ParamValue,
    ParseError, Point, Selector, Target, TimeRange, TransitionKind,
};

/// Bumped when the lock's grammar or the engine's results change for the same inputs.
pub const LOCK_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lock {
    /// The brand's imports with the commit each was read at.
    pub imports: Vec<Import>,
    /// Every file the resolved brand draws from, by content hash, sorted by path.
    pub assets: Vec<Asset>,
    /// The brand after the cascade, with no imports or rules of its own.
    pub brand: Brand,
    /// One block per take of the main cut, in cut order.
    pub takes: Vec<LockTake>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub path: String,
    pub hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockTake {
    pub take: Id,
    /// Sorted by start, then effect, then element.
    pub instances: Vec<Instance>,
    /// Overrides of this take that found no element, sorted by `at` then kind.
    pub orphans: Vec<Orphan>,
}

/// An override whose element is gone: `shot check` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Orphan {
    pub at: Micros,
    pub kind: ElementKind,
}

/// One effect applied to one element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub applied: Applied,
    pub element: ElementRef,
    pub provenance: Provenance,
}

/// The element an instance came from, by identity within its take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementRef {
    pub target: Target,
    pub t: Micros,
}

/// Where an instance came from: a rule of a layer, or an override in `cut.kdl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub layer: Layer,
    /// The rule's selector, `None` for an override.
    pub rule: Option<Selector>,
    /// The override's `at`, which with the take and the element kind is its identity.
    pub override_at: Option<Micros>,
}

/// The resolved values of an instance, in canonical node order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Material removed by the pacing.
    Cut(TimeRange),
    /// A finding the pacing was not sure about, waiting in the review queue. It plays.
    Review(TimeRange),
    Layout {
        layout: Layout,
        range: TimeRange,
    },
    Zoom {
        scale: u16,
        range: TimeRange,
        center: Option<Point>,
    },
    Overlay {
        template: Id,
        range: TimeRange,
        params: Vec<(Id, ParamValue)>,
    },
    Sfx {
        sound: Id,
        at: Micros,
        gain: i16,
    },
    Transition {
        kind: TransitionKind,
        at: Micros,
        length: Micros,
    },
    Music {
        file: String,
        at: Micros,
        gain: i16,
        duck: i16,
    },
    Captions {
        style: CaptionStyle,
        range: TimeRange,
    },
}

impl Applied {
    pub fn start(&self) -> Micros {
        match self {
            Self::Cut(range) | Self::Review(range) => range.from(),
            Self::Layout { range, .. }
            | Self::Zoom { range, .. }
            | Self::Overlay { range, .. }
            | Self::Captions { range, .. } => range.from(),
            Self::Sfx { at, .. } | Self::Transition { at, .. } | Self::Music { at, .. } => *at,
        }
    }

    fn node_name(&self) -> &'static str {
        match self {
            Self::Cut(_) => "cut",
            Self::Review(_) => "review",
            Self::Layout { .. } => "layout",
            Self::Zoom { .. } => "zoom",
            Self::Overlay { .. } => "overlay",
            Self::Sfx { .. } => "sfx",
            Self::Transition { .. } => "transition",
            Self::Music { .. } => "music",
            Self::Captions { .. } => "captions",
        }
    }

    /// The canonical order within a take: by start, then node, then a template or sound.
    fn order_key(&self) -> (Micros, u8, String) {
        let node = match self {
            Self::Cut(_) => 0,
            Self::Review(_) => 1,
            Self::Layout { .. } => 2,
            Self::Zoom { .. } => 3,
            Self::Overlay { .. } => 4,
            Self::Sfx { .. } => 5,
            Self::Transition { .. } => 6,
            Self::Music { .. } => 7,
            Self::Captions { .. } => 8,
        };
        let name = match self {
            Self::Overlay { template, .. } => template.to_string(),
            Self::Sfx { sound, .. } => sound.to_string(),
            _ => String::new(),
        };
        (self.start(), node, name)
    }
}

impl Instance {
    pub(crate) fn order_key(&self) -> (Micros, u8, String, Micros) {
        let (start, node, name) = self.applied.order_key();
        (start, node, name, self.element.t)
    }
}

impl LockTake {
    /// The ranges the pacing removes, in order. They may overlap; the timeline takes the union.
    pub fn cuts(&self) -> impl Iterator<Item = TimeRange> + '_ {
        self.instances.iter().filter_map(|i| match i.applied {
            Applied::Cut(range) => Some(range),
            _ => None,
        })
    }
}

impl Lock {
    pub fn take(&self, take: &Id) -> Option<&LockTake> {
        self.takes.iter().find(|t| &t.take == take)
    }

    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let src = Source {
            file: "cut.lock",
            text,
        };
        let doc = src.parse()?;
        let mut lock = Self::default();
        let (mut version, mut brand) = (None, None);
        let mut brand_nodes = Vec::new();
        for node in src.nodes(&doc) {
            match node.name() {
                "lock" => {
                    node.expect(1, &[])?;
                    let found: u32 = node.arg(0)?;
                    if found != LOCK_VERSION {
                        return Err(node.error(format!(
                            "lock version {found}, this build reads version {LOCK_VERSION}"
                        )));
                    }
                    node.set_once(&mut version, found)?;
                }
                "import" => {
                    node.expect(1, &["rev", "style"])?;
                    lock.imports.push(Import {
                        url: node.arg(0)?,
                        rev: node.prop("rev")?,
                        style: node.prop("style")?,
                    });
                }
                "asset" => {
                    node.expect(1, &["hash"])?;
                    let hex: String = node.prop("hash")?;
                    let hash = ContentHash::from_hex(&hex)
                        .ok_or_else(|| node.error("`hash` is 64 hex digits"))?;
                    lock.assets.push(Asset {
                        path: node.arg(0)?,
                        hash,
                    });
                }
                "brand" => {
                    node.expect_block(0, &[])?;
                    node.set_once(&mut brand, ())?;
                    brand_nodes.extend(node.children());
                }
                "take" => lock.takes.push(LockTake::read(node)?),
                _ => return Err(node.unknown()),
            }
        }
        required(version, "lock", |m| src.error(0, m))?;
        lock.brand = Brand::read(brand_nodes.into_iter(), BrandScope::Patch)?;
        if let Some(window) = lock.assets.windows(2).find(|w| w[0].path >= w[1].path) {
            return Err(src.error(
                0,
                format!(
                    "assets must be sorted by path, `{}` is out of order",
                    window[1].path
                ),
            ));
        }
        Ok(lock)
    }

    pub fn to_kdl(&self) -> String {
        let mut w = Writer::default();
        w.node("lock").arg(&LOCK_VERSION).end();
        for import in &self.imports {
            w.node("import")
                .arg(&import.url)
                .prop("rev", &import.rev)
                .prop("style", &import.style)
                .end();
        }
        for asset in &self.assets {
            w.node("asset")
                .arg(&asset.path)
                .prop("hash", &asset.hash.to_string())
                .end();
        }
        if !self.brand.is_empty() {
            w.blank();
            w.node("brand").open();
            self.brand.write(&mut w);
            w.close();
        }
        for take in &self.takes {
            w.blank();
            take.write(&mut w);
        }
        w.finish()
    }
}

impl LockTake {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        node.expect_block(1, &[])?;
        let mut take = Self {
            take: node.arg(0)?,
            instances: Vec::new(),
            orphans: Vec::new(),
        };
        for child in node.children() {
            if child.name() == "orphan" {
                child.expect(1, &["at"])?;
                take.orphans.push(Orphan {
                    at: child.prop("at")?,
                    kind: child.arg(0)?,
                });
            } else {
                take.instances.push(Instance::read(child)?);
            }
        }
        take.instances.sort_by_key(Instance::order_key);
        take.orphans.sort_unstable();
        Ok(take)
    }

    fn write(&self, w: &mut Writer) {
        w.node("take").arg(&self.take).open();
        for instance in &self.instances {
            instance.write(w);
        }
        for orphan in &self.orphans {
            w.node("orphan")
                .arg(&orphan.kind)
                .prop("at", &orphan.at)
                .end();
        }
        w.close();
    }
}

const PROVENANCE_PROPS: [&str; 5] = ["on", "t", "layer", "rule", "override"];

/// The instance's own properties followed by the provenance ones.
fn props<'a>(own: &[&'a str]) -> Vec<&'a str> {
    own.iter().chain(&PROVENANCE_PROPS).copied().collect()
}

impl Instance {
    fn read(node: Node<'_>) -> Result<Self, ParseError> {
        let applied = match node.name() {
            "cut" => {
                node.expect(0, &props(&["from", "to"]))?;
                Applied::Cut(TimeRange::read(node)?)
            }
            "review" => {
                node.expect(0, &props(&["from", "to"]))?;
                Applied::Review(TimeRange::read(node)?)
            }
            "layout" => {
                node.expect(1, &props(&["from", "to"]))?;
                Applied::Layout {
                    layout: node.arg(0)?,
                    range: TimeRange::read(node)?,
                }
            }
            "zoom" => {
                node.expect(1, &props(&["from", "to", "x", "y"]))?;
                let scale: u16 = node.arg(0)?;
                if scale < 100 {
                    return Err(node.error("zoom scale is a percentage of at least 100"));
                }
                let center = match (node.opt_prop("x")?, node.opt_prop("y")?) {
                    (Some(x), Some(y)) => Some(Point { x, y }),
                    (None, None) => None,
                    _ => return Err(node.error("`x` and `y` come together")),
                };
                Applied::Zoom {
                    scale,
                    range: TimeRange::read(node)?,
                    center,
                }
            }
            "overlay" => {
                node.expect_block(1, &props(&["from", "to"]))?;
                let mut params: Vec<(Id, ParamValue)> = Vec::new();
                for param in node.children() {
                    param.expect(1, &[])?;
                    let name = Id::new(param.name())
                        .ok_or_else(|| param.error("parameter names are ids"))?;
                    if params.iter().any(|(n, _)| *n == name) {
                        return Err(param.error(format!("duplicate parameter `{name}`")));
                    }
                    params.push((name, param.arg(0)?));
                }
                Applied::Overlay {
                    template: node.arg(0)?,
                    range: TimeRange::read(node)?,
                    params,
                }
            }
            "sfx" => {
                node.expect(1, &props(&["at", "gain"]))?;
                Applied::Sfx {
                    sound: node.arg(0)?,
                    at: node.prop("at")?,
                    gain: node.opt_prop("gain")?.unwrap_or(0),
                }
            }
            "transition" => {
                node.expect(1, &props(&["at", "length"]))?;
                Applied::Transition {
                    kind: node.arg(0)?,
                    at: node.prop("at")?,
                    length: node.prop("length")?,
                }
            }
            "music" => {
                node.expect(1, &props(&["at", "gain", "duck"]))?;
                Applied::Music {
                    file: node.arg(0)?,
                    at: node.prop("at")?,
                    gain: node.prop("gain")?,
                    duck: node.prop("duck")?,
                }
            }
            "captions" => {
                node.expect(1, &props(&["from", "to"]))?;
                Applied::Captions {
                    style: node.arg(0)?,
                    range: TimeRange::read(node)?,
                }
            }
            _ => return Err(node.unknown()),
        };
        let element = ElementRef {
            target: node.prop("on")?,
            t: node.prop("t")?,
        };
        let layer: Layer = node.prop("layer")?;
        let rule: Option<String> = node.opt_prop("rule")?;
        let override_at: Option<Micros> = node.opt_prop("override")?;
        let provenance = match (layer, rule, override_at) {
            (Layer::Override, None, Some(at)) => Provenance {
                layer,
                rule: None,
                override_at: Some(at),
            },
            (Layer::Override, _, _) => {
                return Err(node.error("an override instance names `override`, not `rule`"));
            }
            (_, Some(rule), None) => Provenance {
                layer,
                rule: Some(Selector::parse(&rule).map_err(|e| node.error(e.message))?),
                override_at: None,
            },
            _ => return Err(node.error("a rule instance names `rule`, not `override`")),
        };
        Ok(Self {
            applied,
            element,
            provenance,
        })
    }

    fn write(&self, w: &mut Writer) {
        w.node(self.applied.node_name());
        match &self.applied {
            Applied::Cut(range) | Applied::Review(range) => {
                write_range(w, *range);
            }
            Applied::Layout { layout, range } => {
                w.arg(layout);
                write_range(w, *range);
            }
            Applied::Zoom {
                scale,
                range,
                center,
            } => {
                w.arg(scale);
                write_range(w, *range);
                if let Some(Point { x, y }) = center {
                    w.prop("x", x).prop("y", y);
                }
            }
            Applied::Overlay {
                template, range, ..
            } => {
                w.arg(template);
                write_range(w, *range);
            }
            Applied::Sfx { sound, at, gain } => {
                w.arg(sound).prop("at", at);
                if *gain != 0 {
                    w.prop("gain", gain);
                }
            }
            Applied::Transition { kind, at, length } => {
                w.arg(kind).prop("at", at).prop("length", length);
            }
            Applied::Music {
                file,
                at,
                gain,
                duck,
            } => {
                w.arg(file)
                    .prop("at", at)
                    .prop("gain", gain)
                    .prop("duck", duck);
            }
            Applied::Captions { style, range } => {
                w.arg(style);
                write_range(w, *range);
            }
        }
        w.prop("on", &self.element.target)
            .prop("t", &self.element.t);
        w.prop("layer", &self.provenance.layer);
        if let Some(rule) = &self.provenance.rule {
            w.prop("rule", &rule.to_string());
        }
        if let Some(at) = &self.provenance.override_at {
            w.prop("override", at);
        }
        match &self.applied {
            Applied::Overlay { params, .. } if !params.is_empty() => {
                w.open();
                for (name, value) in params {
                    w.node(name.as_str()).arg(value).end();
                }
                w.close();
            }
            _ => w.end(),
        }
    }
}

fn write_range(w: &mut Writer, range: TimeRange) {
    w.prop("from", &range.from()).prop("to", &range.to());
}
