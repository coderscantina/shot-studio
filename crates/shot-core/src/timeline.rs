use std::fmt;
use std::ops::Range;

use crate::cut::{
    CaptionFix, DerivedCut, LayoutChange, Overlay, ParamValue, RangeEdit, TakeEdit, Zoom,
};
use crate::lock::{Applied, LockTake};
use crate::{
    CaptionStyle, DerivedKind, Effects, Id, Layout, LayoutSwitch, Lock, Micros, Project,
    SwitchKind, TimeRange, ZoomMotion,
};

/// Bumped whenever the renderer's output changes for the same input, so cached chunks expire.
pub const RENDERER_VERSION: u32 = 5;

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
    /// The layout switch whose window overlaps the segment, at most one: windows start on
    /// their edge and end before the next.
    pub switch: Option<SceneSwitch>,
}

/// A layout switch inside a take block: where the layout changes in the take's media time,
/// from what, and how it moves into the segment's layout.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneSwitch {
    /// The edge, where the new layout starts.
    pub at: Micros,
    pub from: Layout,
    pub to: Layout,
    /// What `cut.kdl` writes on the edge, else the format's `layout-switch`.
    pub switch: LayoutSwitch,
    /// From the edge for the switch's length, cut short by the next edge.
    pub window: TimeRange,
}

impl SceneSwitch {
    /// How far through its window the switch is at media time `t`, from 0 to 1.
    pub fn progress(&self, t: Micros) -> f32 {
        let (from, to) = (self.window.from().0, self.window.to().0);
        (t.0.clamp(from, to) - from) as f32 / (to - from) as f32
    }

    /// Whether the switch draws anything over `span`: a cut changes no pixels.
    pub fn draws_over(&self, span: TimeRange) -> bool {
        self.switch.kind != SwitchKind::Cut && self.window.overlaps(span)
    }
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

    /// The hash of `bytes`, for files the lock records by content.
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Reads the 64 hex digits `Display` writes.
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Self(bytes))
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
    /// A video as `cut.kdl` writes it, before the lock.
    pub fn evaluate(project: &Project) -> Self {
        Self::build(project, None)
    }

    /// A migrated video: the kept ranges less the union of the lock's cuts, with the lock's
    /// zooms and overlays next to a person's. The segments, and so the chunk hashes, carry the
    /// lock's resolved values; provenance stays out of them.
    pub fn styled(project: &Project, lock: &Lock) -> Self {
        Self::build(project, Some(lock))
    }

    fn build(project: &Project, lock: Option<&Lock>) -> Self {
        let captions = project.cut.captions.unwrap_or(project.format.captions);
        let default_layout = project.format.default_layout;
        let default_switch = project.format.layout_switch();
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
                        .flat_map(|take| {
                            let styled = lock.and_then(|l| l.take(&take.take));
                            take_segments(take, default_layout, &default_switch, styled)
                        })
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
    /// frames, the layout, a layout switch drawing over the frames with its kind, the layout it
    /// leaves, its edge and window, the cursor and keystroke settings when the layout shows the
    /// screen, the window style in the `window` layout, the zooms and overlays. Instant zooms
    /// are cut to the frames; smooth zooms ease, so every zoom of the segment counts whole.
    /// Overlays count whole with their parameters. The output position is not part of it, so a chunk
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
        // Only a chunk a switch draws over adds it, so every other chunk keeps its key.
        if let Some(s) = &segment.switch
            && span.is_some_and(|span| s.draws_over(span))
        {
            h.tagged(1, s.switch.kind.as_str().as_bytes());
            h.str(s.from.as_str());
            h.u64(s.at.0);
            h.range(s.window);
        }
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
        switches: &[SceneSwitch],
    ) -> Self {
        Self {
            take: take.clone(),
            source,
            layout,
            switch: switches.iter().find(|s| s.window.overlaps(source)).cloned(),
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

/// Kept ranges of one take less the lock's cuts, split wherever the layout changes.
fn take_segments(
    take: &TakeEdit,
    default_layout: Layout,
    default_switch: &LayoutSwitch,
    lock: Option<&LockTake>,
) -> Vec<Segment> {
    let mut zooms = take.zooms.clone();
    let mut overlays = take.overlays.clone();
    let mut cuts = Vec::new();
    for instance in lock.iter().flat_map(|l| &l.instances) {
        match &instance.applied {
            Applied::Cut(range) => cuts.push(*range),
            Applied::Zoom {
                scale,
                range,
                center,
            } => zooms.push(Zoom {
                scale: *scale,
                range: *range,
                center: *center,
                by: None,
            }),
            Applied::Overlay {
                template,
                range,
                params,
            } => overlays.push(Overlay {
                template: template.clone(),
                range: *range,
                params: params.clone(),
            }),
            _ => {}
        }
    }
    zooms.sort_by_key(|z| z.range.from());
    let cuts = union(cuts);
    let switches = scene_switches(take, default_layout, default_switch);
    take.ranges
        .iter()
        .filter_map(|edit| match edit {
            RangeEdit::Keep(range) => Some(*range),
            RangeEdit::Remove { .. } => None,
        })
        .flat_map(|kept| subtract(kept, &cuts))
        .flat_map(|kept| {
            let mut edges: Vec<Micros> = take
                .layouts
                .iter()
                .flat_map(|c| [c.range.from(), c.range.to()])
                .filter(|&t| kept.from() < t && t < kept.to())
                .collect();
            edges.push(kept.from());
            edges.push(kept.to());
            edges.sort_unstable();
            edges.dedup();
            edges
                .windows(2)
                .filter_map(|w| TimeRange::new(w[0], w[1]))
                .map(|source| {
                    let layout = layout_at(&take.layouts, source.from()).unwrap_or(default_layout);
                    Segment::new(&take.take, source, layout, &zooms, &overlays, &switches)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Ranges merged where they overlap or touch, in order.
fn union(mut ranges: Vec<TimeRange>) -> Vec<TimeRange> {
    ranges.sort_by_key(|r| r.from());
    let mut out: Vec<TimeRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match out.last_mut() {
            Some(last) if range.from() <= last.to() => {
                *last = TimeRange::new(last.from(), last.to().max(range.to()))
                    .expect("merged ranges are not empty");
            }
            _ => out.push(range),
        }
    }
    out
}

/// The parts of `range` outside the sorted, disjoint `cuts`.
fn subtract(range: TimeRange, cuts: &[TimeRange]) -> Vec<TimeRange> {
    let mut out = Vec::new();
    let mut from = range.from();
    for cut in cuts.iter().filter(|c| c.overlaps(range)) {
        out.extend(TimeRange::new(from, cut.from()));
        from = from.max(cut.to());
    }
    out.extend(TimeRange::new(from, range.to()));
    out
}

/// Every place inside the block's kept material where the layout changes, in order. A change
/// at or before the first kept moment is where the block starts, not a switch.
fn scene_switches(
    take: &TakeEdit,
    default_layout: Layout,
    default_switch: &LayoutSwitch,
) -> Vec<SceneSwitch> {
    let first = take.ranges.iter().find_map(|r| match r {
        RangeEdit::Keep(range) => Some(range.from()),
        RangeEdit::Remove { .. } => None,
    });
    let Some(first) = first else {
        return Vec::new();
    };
    let layout_of = |t: Micros| layout_at(&take.layouts, t).unwrap_or(default_layout);
    let mut edges: Vec<Micros> = take
        .layouts
        .iter()
        .flat_map(|c| [c.range.from(), c.range.to()])
        .filter(|&t| t > first)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let edges: Vec<(Micros, Layout, Layout)> = edges
        .into_iter()
        .map(|t| (t, layout_of(Micros(t.0 - 1)), layout_of(t)))
        .filter(|(_, from, to)| from != to)
        .collect();
    edges
        .iter()
        .enumerate()
        .filter_map(|(i, &(at, from, to))| {
            let written = take
                .layouts
                .iter()
                .find(|c| c.range.from() == at)
                .and_then(|c| c.enter.as_ref())
                .or_else(|| {
                    take.layouts
                        .iter()
                        .find(|c| c.range.to() == at)
                        .and_then(|c| c.exit.as_ref())
                });
            let switch = written.unwrap_or(default_switch).clone();
            let mut end = Micros(at.0 + switch.length.0);
            if let Some((next, ..)) = edges.get(i + 1) {
                end = end.min(*next);
            }
            Some(SceneSwitch {
                at,
                from,
                to,
                window: TimeRange::new(at, end)?,
                switch,
            })
        })
        .collect()
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
                        s.switch.as_slice(),
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
