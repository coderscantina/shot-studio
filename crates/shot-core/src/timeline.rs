use std::fmt;
use std::ops::Range;

use crate::cut::{
    CaptionFix, DerivedCut, LayoutChange, Overlay, ParamValue, RangeEdit, TakeEdit, Zoom,
};
use crate::{
    CaptionStyle, DerivedKind, Effects, Id, Layout, Micros, Project, TimeRange, ZoomMotion,
};

/// Bumped whenever the renderer's output changes for the same input, so cached chunks expire.
pub const RENDERER_VERSION: u32 = 2;

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
    /// Applied wherever this output shows captions.
    pub caption_fixes: Vec<CaptionFix>,
    /// The format's render settings with defaults filled in.
    pub effects: Effects,
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

/// A stretch of output from one take with one layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub take: Id,
    pub source: TimeRange,
    pub layout: Layout,
    /// Zooms and overlays that overlap the segment, with their full ranges.
    pub zooms: Vec<Zoom>,
    pub overlays: Vec<Overlay>,
}

/// Identifies the recorded files of a take. Take ids can be reused after the newest take is
/// deleted, so cache keys hash this too and a take recorded again never matches the old
/// recording's chunks. The IO side computes it, for example from file sizes and modification
/// times.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MediaId([u8; 32]);

impl MediaId {
    /// The id of a take whose files are described by `facts`.
    pub fn of(facts: &[u8]) -> Self {
        Self(*blake3::hash(facts).as_bytes())
    }
}

/// A BLAKE3 hash over everything that affects a chunk's pixels.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The key of a chunk that a later pass draws from what this key names plus `input`, for
    /// example a derived cut's chunk drawn from a main chunk. The pass encodes `input` so that
    /// distinct inputs never share bytes.
    pub fn extend(&self, input: &[u8]) -> Self {
        let mut h = HashInput(blake3::Hasher::new());
        h.0.update(&self.0);
        h.u64(input.len() as u64);
        h.0.update(input);
        Self(*h.0.finalize().as_bytes())
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
            caption_fixes: project.cut.caption_fixes.clone(),
            effects: project.format.effects(),
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

impl Output {
    /// The cache key of `frames` of `segment`, counted at `fps` from the start of its take: a
    /// hash over everything those frames show. It covers the renderer version, the output
    /// aspect, the burned-in caption style with its caption fixes, the take and its media, the
    /// frames, the layout, the cursor and keystroke settings when the layout shows the screen,
    /// the window style in the `window` layout, the zooms and overlays. Instant zooms are cut
    /// to the frames; smooth zooms ease, so every zoom of the segment counts whole. Overlays
    /// count whole with their parameters. The output position is not part of it, so a chunk
    /// stays valid when an edit before it moves it.
    pub fn chunk_hash(
        &self,
        segment: &Segment,
        media: MediaId,
        fps: u32,
        frames: Range<u64>,
    ) -> ContentHash {
        let time = |frame: u64| Micros(frame * 1_000_000 / u64::from(fps));
        let span = TimeRange::new(time(frames.start), time(frames.end));
        let mut h = HashInput(blake3::Hasher::new());
        h.u64(RENDERER_VERSION.into());
        let (w, hh) = self.kind.aspect();
        h.u64(w.into());
        h.u64(hh.into());
        h.str(if self.kind.burns_captions() {
            self.captions.as_str()
        } else {
            "sidecar"
        });
        if self.kind.burns_captions() && self.captions != CaptionStyle::Off {
            h.u64(self.caption_fixes.len() as u64);
            for fix in &self.caption_fixes {
                h.str(&fix.heard);
                h.str(&fix.said);
            }
        }
        h.str(segment.take.as_str());
        h.0.update(&media.0);
        h.u64(fps.into());
        h.u64(frames.start);
        h.u64(frames.end);
        h.str(segment.layout.as_str());
        let effects = &self.effects;
        if segment.layout.shows_screen() {
            h.u64(effects.cursor.size.into());
            h.u64(effects.cursor.smoothing.0);
            h.u64(u8::from(effects.cursor.ripple).into());
            h.str(effects.keystrokes.as_str());
        }
        if segment.layout == Layout::Window {
            let win = &effects.window;
            h.u64(win.padding.into());
            h.u64(win.corners.into());
            h.str(win.shadow.as_str());
            h.0.update(&win.background.0);
            h.str(win.frame.as_str());
        }
        let smooth = effects.zoom_motion == ZoomMotion::Smooth;
        let zooms: Vec<(&Zoom, TimeRange)> = segment
            .zooms
            .iter()
            .filter_map(|zoom| {
                let range = if smooth {
                    zoom.range
                } else {
                    zoom.range.intersect(span?)?
                };
                Some((zoom, range))
            })
            .collect();
        h.u64(zooms.len() as u64);
        if !zooms.is_empty() {
            h.str(effects.zoom_motion.as_str());
        }
        for (zoom, range) in zooms {
            h.u64(zoom.scale.into());
            h.range(range);
            match zoom.center {
                Some(center) => {
                    h.tagged(1, &center.x.to_le_bytes());
                    h.tagged(1, &center.y.to_le_bytes());
                }
                None => h.tagged(0, &[]),
            }
        }
        let overlays: Vec<&Overlay> = segment
            .overlays
            .iter()
            .filter(|o| span.is_some_and(|span| o.range.overlaps(span)))
            .collect();
        h.u64(overlays.len() as u64);
        for overlay in overlays {
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
        ContentHash(*h.0.finalize().as_bytes())
    }
}

impl Segment {
    /// `zooms` and `overlays` are the take's, cut down to the ones overlapping `source`.
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
}

/// Kept ranges of one take, split wherever the layout changes.
fn take_segments(take: &TakeEdit, default_layout: Layout) -> impl Iterator<Item = Segment> + '_ {
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
                    Segment::new(&take.take, source, layout, &take.zooms, &take.overlays)
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
                    Some(Segment::new(
                        &s.take,
                        source,
                        s.layout,
                        &s.zooms,
                        &s.overlays,
                    ))
                })
        })
        .collect();
    Output {
        kind,
        name: cut.name.to_string(),
        captions,
        caption_fixes: main.caption_fixes.clone(),
        effects: main.effects,
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
