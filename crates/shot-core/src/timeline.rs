use std::fmt;

use crate::cut::{DerivedCut, LayoutChange, Overlay, ParamValue, RangeEdit, TakeEdit, Zoom};
use crate::{CaptionStyle, DerivedKind, Id, Layout, Micros, Project, TimeRange};

/// Bumped whenever the renderer's output changes for the same input, so cached segments expire.
pub const RENDERER_VERSION: u32 = 1;

/// The evaluated project: the main cut followed by each derived cut, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub outputs: Vec<Output>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub kind: OutputKind,
    /// `main` for the main cut, otherwise the derived cut's name.
    pub name: String,
    pub captions: CaptionStyle,
    /// The main cut's chapters. A derived cut has one group named after it.
    pub chapters: Vec<ChapterSegments>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputKind {
    Main,
    Derived(DerivedKind),
}

impl OutputKind {
    /// Width and height of the output aspect ratio.
    pub fn aspect(self) -> (u32, u32) {
        match self {
            Self::Main => (16, 9),
            Self::Derived(DerivedKind::Short) => (9, 16),
            Self::Derived(DerivedKind::XClip) => (1, 1),
        }
    }

    /// The main cut ships captions as a sidecar file, derived cuts burn them in.
    pub fn burns_captions(self) -> bool {
        matches!(self, Self::Derived(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterSegments {
    pub title: String,
    pub segments: Vec<Segment>,
}

/// A stretch of output from one take with one layout, the unit of render caching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub take: Id,
    pub source: TimeRange,
    pub layout: Layout,
    /// Zooms and overlays that overlap the segment, with their full ranges.
    pub zooms: Vec<Zoom>,
    pub overlays: Vec<Overlay>,
    pub hash: ContentHash,
}

/// A BLAKE3 hash over everything that affects a segment's pixels and samples.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

impl Timeline {
    pub fn evaluate(project: &Project) -> Self {
        let captions = project.cut.captions.unwrap_or(project.format.captions);
        let default_layout = project.format.default_layout;
        let main = Output {
            kind: OutputKind::Main,
            name: "main".to_owned(),
            captions,
            chapters: project
                .cut
                .chapters
                .iter()
                .map(|chapter| ChapterSegments {
                    title: chapter.title.clone(),
                    segments: chapter
                        .takes
                        .iter()
                        .flat_map(|take| take_segments(take, default_layout))
                        .map(|parts| parts.finish(OutputKind::Main, captions))
                        .collect(),
                })
                .collect(),
        };
        let derived: Vec<Output> = project
            .cut
            .derived
            .iter()
            .map(|d| derive(&main, d, captions))
            .collect();
        Self {
            outputs: std::iter::once(main).chain(derived).collect(),
        }
    }

    pub fn main(&self) -> &Output {
        &self.outputs[0]
    }
}

/// A segment before hashing.
struct Parts {
    take: Id,
    source: TimeRange,
    layout: Layout,
    zooms: Vec<Zoom>,
    overlays: Vec<Overlay>,
}

impl Parts {
    fn new(
        take: &Id,
        source: TimeRange,
        layout: Layout,
        zooms: &[Zoom],
        overlays: &[Overlay],
    ) -> Self {
        Self {
            take: take.clone(),
            source,
            layout,
            zooms: zooms
                .iter()
                .filter(|z| z.range.overlaps(source))
                .copied()
                .collect(),
            overlays: overlays
                .iter()
                .filter(|o| o.range.overlaps(source))
                .cloned()
                .collect(),
        }
    }

    fn finish(self, kind: OutputKind, captions: CaptionStyle) -> Segment {
        let mut h = HashInput(blake3::Hasher::new());
        h.u64(RENDERER_VERSION.into());
        let (w, hh) = kind.aspect();
        h.u64(w.into());
        h.u64(hh.into());
        h.str(if kind.burns_captions() {
            captions.as_str()
        } else {
            "sidecar"
        });
        h.str(self.take.as_str());
        h.range(self.source);
        h.str(self.layout.as_str());
        h.u64(self.zooms.len() as u64);
        for zoom in &self.zooms {
            h.u64(zoom.scale.into());
            h.range(zoom.range);
        }
        h.u64(self.overlays.len() as u64);
        for overlay in &self.overlays {
            h.str(overlay.template.as_str());
            h.range(overlay.range);
            h.u64(overlay.params.len() as u64);
            for (name, value) in &overlay.params {
                h.str(name.as_str());
                match value {
                    ParamValue::Text(s) => h.tagged(0, s.as_bytes()),
                    ParamValue::Integer(n) => h.tagged(1, &n.to_le_bytes()),
                    ParamValue::Bool(b) => h.tagged(2, &[u8::from(*b)]),
                }
            }
        }
        Segment {
            take: self.take,
            source: self.source,
            layout: self.layout,
            zooms: self.zooms,
            overlays: self.overlays,
            hash: ContentHash(*h.0.finalize().as_bytes()),
        }
    }
}

/// Kept ranges of one take, split wherever the layout changes.
fn take_segments(take: &TakeEdit, default_layout: Layout) -> impl Iterator<Item = Parts> + '_ {
    take.ranges
        .iter()
        .filter_map(|edit| match edit {
            RangeEdit::Keep(range) => Some(*range),
            RangeEdit::Remove { .. } => None,
        })
        .flat_map(move |kept| {
            let mut cuts: Vec<Micros> = take
                .layouts
                .iter()
                .flat_map(|c| [c.range.from(), c.range.to()])
                .filter(|&t| kept.from() < t && t < kept.to())
                .collect();
            cuts.push(kept.from());
            cuts.push(kept.to());
            cuts.sort_unstable();
            cuts.dedup();
            cuts.windows(2)
                .filter_map(|w| TimeRange::new(w[0], w[1]))
                .map(|source| {
                    let layout = layout_at(&take.layouts, source.from()).unwrap_or(default_layout);
                    Parts::new(&take.take, source, layout, &take.zooms, &take.overlays)
                })
                .collect::<Vec<_>>()
        })
}

fn layout_at(changes: &[LayoutChange], at: Micros) -> Option<Layout> {
    changes
        .iter()
        .find(|c| c.range.from() <= at && at < c.range.to())
        .map(|c| c.layout)
}

/// Selects the main cut's segments that fall inside each clip, in clip order.
fn derive(main: &Output, cut: &DerivedCut, main_captions: CaptionStyle) -> Output {
    let kind = OutputKind::Derived(cut.kind);
    let captions = cut.captions.unwrap_or(main_captions);
    let segments = cut
        .clips
        .iter()
        .flat_map(|clip| {
            main.chapters
                .iter()
                .flat_map(|c| &c.segments)
                .filter(move |s| s.take == clip.take)
                .filter_map(move |s| {
                    let source = s.source.intersect(clip.range)?;
                    Some(
                        Parts::new(&s.take, source, s.layout, &s.zooms, &s.overlays)
                            .finish(kind, captions),
                    )
                })
        })
        .collect();
    Output {
        kind,
        name: cut.name.to_string(),
        captions,
        chapters: vec![ChapterSegments {
            title: cut.name.to_string(),
            segments,
        }],
    }
}

/// Feeds values to the hasher with lengths and tags, so distinct inputs never collide by
/// concatenation.
struct HashInput(blake3::Hasher);

impl HashInput {
    fn u64(&mut self, n: u64) {
        self.0.update(&n.to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.0.update(s.as_bytes());
    }

    fn tagged(&mut self, tag: u8, bytes: &[u8]) {
        self.0.update(&[tag]);
        self.u64(bytes.len() as u64);
        self.0.update(bytes);
    }

    fn range(&mut self, range: TimeRange) {
        self.u64(range.from().0);
        self.u64(range.to().0);
    }
}
