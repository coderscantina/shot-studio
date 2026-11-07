//! Derived cuts: Shorts at 9:16 and X clips at 1:1, drawn from the main cut's chunks.
//!
//! A derived cut shows the main cut's own pixels: each of its frames is a window of the main
//! frame at the same moment, scaled to the derived canvas, with the captions burned in. So it
//! renders in two passes, each cached in chunks:
//!
//! 1. **Base**: the main chunk that holds the frames, keyed and rendered exactly as the main
//!    export does. A main export renders the same chunk, so whichever runs first leaves it for the
//!    other ([decision 0017](../../../docs/decisions/0017-derived-cuts-from-main-chunks.md)).
//! 2. **Derived**: the window and the captions over the base chunk's frames, keyed by the base
//!    chunk's key plus the window's position and the caption shown on each frame.
//!
//! The window is as tall as the main frame and as wide as the aspect allows. A Short follows the
//! face: Vision finds the largest face in the camera track every half second, the layout of the
//! moment places it on the main canvas, and the window centers on its smoothed position, moved
//! only as far as it takes to keep the whole face inside. A layout without the camera, or a
//! take where Vision finds no face, keeps the window centered.

use std::collections::HashMap;
use std::ops::Range;

use shot_analyze::vision::NormRect;
use shot_core::{
    CaptionStyle, ContentHash, DerivedKind, Effects, Id, MediaId, Micros, Output, OutputKind,
    Transcript,
};

use crate::captions::{self, Cue};
use crate::drawing::{Drawing, Rgba, Shape};
use crate::frame::Frame;
use crate::kinetic;
use crate::layout::{BACKGROUND, Canvas, Layer, LayoutSpec, Rect, Scene, Source, Sources};
use crate::plan::{ExportPlan, PlannedSegment, frame_time};
use crate::zoom::punch_at;

/// The main canvas the window moves over.
const MAIN: Canvas = Canvas::UHD;
/// The step between face samples: the recordings' keyframe grid, so each sample decodes one
/// keyframe.
pub const FACE_STEP: Micros = Micros(500_000);
/// Face positions are averaged over this much before and after each sample.
const SMOOTHING: Micros = Micros(1_000_000);

/// The size of a derived cut's frames.
pub fn canvas(kind: DerivedKind) -> Canvas {
    match kind {
        DerivedKind::Short => Canvas {
            width: 1080,
            height: 1920,
        },
        DerivedKind::XClip => Canvas {
            width: 1080,
            height: 1080,
        },
    }
}

/// The window's width on the main canvas: as tall as the main frame, at the cut's aspect.
pub fn window_width(kind: DerivedKind) -> f32 {
    let c = canvas(kind);
    (MAIN.height as f32 * c.width as f32 / c.height as f32).round()
}

/// The faces Vision found in a take's camera track.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceTrack {
    /// The camera's frame size in pixels.
    pub size: (u32, u32),
    /// In time order.
    pub samples: Vec<(Micros, Option<NormRect>)>,
}

/// The times a derived output needs faces at, per take: the face step grid over every segment
/// whose layout shows the camera, one step beyond each edge.
pub fn face_times(output: &Output) -> HashMap<Id, Vec<Micros>> {
    let mut times: HashMap<Id, Vec<Micros>> = HashMap::new();
    for segment in output.chapters.iter().flat_map(|c| &c.segments) {
        let shows_camera = LayoutSpec::styled(segment.layout, &output.effects.window)
            .is_ok_and(|spec| spec.uses(Source::Camera));
        if !shows_camera {
            continue;
        }
        let step = FACE_STEP.0;
        let first = (segment.source.from().0 / step).saturating_sub(1);
        let last = segment.source.to().0.div_ceil(step) + 1;
        times
            .entry(segment.take.clone())
            .or_default()
            .extend((first..=last).map(|i| Micros(i * step)));
    }
    for list in times.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    times
}

/// The main chunk a derived chunk is cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    pub hash: ContentHash,
    /// Frames of the take the main chunk holds.
    pub frames: Range<u64>,
}

/// A derived chunk: which base frames it shows, where the window sits on each, and the caption.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedChunk {
    pub base: Base,
    /// The window's left edge on the main canvas, per frame.
    pub windows: Vec<u16>,
    /// Per frame: the cue on screen and the word to highlight.
    pub captions: Vec<Option<Shown>>,
}

/// A caption on screen: the cue's index, the word to highlight, and how long since that word
/// started, up to [`kinetic::word_pop_settle`], for its pop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub cue: usize,
    pub word: Option<usize>,
    pub pop: Micros,
}

/// A derived output planned over the main output: its chunks with their bases and keys.
#[derive(Debug, Clone)]
pub struct DerivedPlan {
    pub kind: DerivedKind,
    pub canvas: Canvas,
    pub main: ExportPlan,
    /// The derived output's segments and chunks. Chunk hashes are the derived keys.
    pub plan: ExportPlan,
    /// Per segment of `plan`, per chunk.
    pub chunks: Vec<Vec<DerivedChunk>>,
    cues: Vec<Cue>,
    style: CaptionStyle,
}

/// What a derived plan is made from besides the timeline.
pub struct DerivedInputs<'a> {
    pub fps: u32,
    pub media: &'a dyn Fn(&Id) -> MediaId,
    /// The identity of a file an overlay names, see [`crate::overlay::key_assets`].
    pub assets: &'a dyn Fn(&str) -> Vec<u8>,
    pub faces: &'a HashMap<Id, FaceTrack>,
    pub transcripts: &'a HashMap<Id, Transcript>,
}

impl DerivedPlan {
    /// Plans `derived`, a derived output of the same timeline as `main`. Every derived chunk lies
    /// inside one main chunk, since a derived segment is a main segment cut down to a clip and
    /// both are chunked on the take's own grid.
    pub fn new(main: &Output, derived: &Output, inputs: &DerivedInputs) -> Self {
        let OutputKind::Derived(kind) = derived.kind else {
            panic!("{} is the main cut", derived.name);
        };
        let fps = inputs.fps;
        let mut main_plan = ExportPlan::new(main, fps, inputs.media);
        crate::overlay::key_assets(&mut main_plan, inputs.assets);
        let mut plan = ExportPlan::new(derived, fps, inputs.media);
        let cues = captions::cues(&plan, inputs.transcripts, &derived.caption_fixes);
        let width = window_width(kind);
        let mut chunks = Vec::with_capacity(plan.segments.len());
        for planned in &mut plan.segments {
            let parent = main_plan
                .segments
                .iter()
                .find(|m| {
                    m.segment.take == planned.segment.take
                        && m.source_frames.start <= planned.source_frames.start
                        && planned.source_frames.end <= m.source_frames.end
                })
                .expect("a derived segment lies inside a main segment");
            let windows = windows(
                parent,
                &main.effects,
                inputs.faces.get(&planned.segment.take),
                width,
                fps,
            );
            let media = (inputs.media)(&planned.segment.take);
            let mut segment_chunks = Vec::with_capacity(planned.chunks.len());
            for chunk in &mut planned.chunks {
                let base = parent
                    .chunks
                    .iter()
                    .find(|c| {
                        c.frames.start <= chunk.frames.start && chunk.frames.end <= c.frames.end
                    })
                    .expect("a derived chunk lies inside a main chunk");
                let output_start =
                    planned.output_frame + chunk.frames.start - planned.source_frames.start;
                let derived_chunk = DerivedChunk {
                    base: Base {
                        hash: base.hash,
                        frames: base.frames.clone(),
                    },
                    windows: chunk
                        .frames
                        .clone()
                        .map(|f| windows[(f - parent.source_frames.start) as usize])
                        .collect(),
                    captions: (0..chunk.frame_count())
                        .map(|i| {
                            caption_at(&cues, frame_time(output_start + i, fps), derived.captions)
                        })
                        .collect(),
                };
                let own = derived.chunk_hash(&planned.segment, media, fps, chunk.frames.clone());
                chunk.hash = own.extend(&key_input(&derived_chunk, &chunk.frames, &cues));
                segment_chunks.push(derived_chunk);
            }
            chunks.push(segment_chunks);
        }
        Self {
            kind,
            canvas: canvas(kind),
            main: main_plan,
            plan,
            chunks,
            cues,
            style: derived.captions,
        }
    }

    /// The main chunks this output draws from, each once.
    pub fn bases(&self) -> Vec<&Base> {
        let mut seen = std::collections::HashSet::new();
        self.chunks
            .iter()
            .flatten()
            .map(|c| &c.base)
            .filter(|b| seen.insert(b.hash))
            .collect()
    }

    /// The frame of chunk `chunk` of segment `segment`, `index` frames in. Its one layer shows
    /// the window of the base frame, passed as the screen source.
    pub fn frame(&self, segment: usize, chunk: usize, index: usize) -> Frame {
        let derived = &self.chunks[segment][chunk];
        let left = f32::from(derived.windows[index]);
        let width = window_width(self.kind);
        let main = MAIN.width as f32;
        let crop = Rect::new(left / main, 0.0, (left + width) / main, 1.0);
        let full = Rect::new(
            0.0,
            0.0,
            self.canvas.width as f32,
            self.canvas.height as f32,
        );
        let mut drawing = Drawing::default();
        if let Some(Shown { cue, word, pop }) = derived.captions[index] {
            drawing
                .over
                .push(caption_shape(&self.cues[cue], word, pop, self.canvas));
        }
        Frame {
            scene: Scene {
                background: BACKGROUND,
                layers: vec![Layer {
                    source: Source::Screen,
                    frame: full,
                    crop,
                    radius: 0.0,
                    blur: 0.0,
                    shutter: Rect::ZERO,
                }],
            },
            drawing,
            cutaway: None,
        }
    }

    pub fn style(&self) -> CaptionStyle {
        self.style
    }
}

/// Renders the derived chunks of `job` into the cache from their base chunks, waiting for bases
/// that are still rendering.
pub(crate) fn render(
    renderer: &crate::graph::Renderer,
    plan: &DerivedPlan,
    job: &crate::export::Job,
    cache: &crate::export::Cache,
    pending: &crate::export::Pending,
    chunk_done: &dyn Fn(ContentHash),
) -> Result<(), crate::export::ExportError> {
    use crate::export::{
        ChunkFiles, ExportError, FPS, KEYFRAME_INTERVAL, QUALITY, decode_error, to_duration,
    };
    use shot_media::encode::{EncoderSettings, HevcEncoder};
    use shot_media::videotoolbox::Decoder;

    let mut encoder = HevcEncoder::new(EncoderSettings {
        width: plan.canvas.width,
        height: plan.canvas.height,
        fps: FPS,
        keyframe_interval: KEYFRAME_INTERVAL,
        quality: QUALITY,
    })?;
    let mut files = ChunkFiles::new(job.chunks, cache, plan.canvas, chunk_done);
    for (index, chunk) in job.range.clone().zip(job.chunks) {
        let derived = &plan.chunks[job.segment][index];
        pending.wait(derived.base.hash)?;
        let path = cache.path(derived.base.hash);
        let mut decoder = Decoder::open_native(&path).map_err(decode_error(&path))?;
        let ends = || ExportError::MediaEnds {
            path: path.clone(),
            time: to_duration(chunk.frames.end - derived.base.frames.start),
        };
        for _ in derived.base.frames.start..chunk.frames.start {
            decoder
                .next_frame()
                .map_err(decode_error(&path))?
                .ok_or_else(ends)?;
        }
        for i in 0..chunk.frame_count() as usize {
            let base = decoder
                .next_frame()
                .map_err(decode_error(&path))?
                .ok_or_else(ends)?;
            let target = encoder.input_buffer()?;
            renderer.draw(
                &plan.frame(job.segment, index, i),
                Sources {
                    screen: Some(&*base.buffer),
                    camera: None,
                },
                &target,
            )?;
            if i == 0 {
                encoder.encode_keyframe(&target)?;
            } else {
                encoder.encode(&target)?;
            }
            for encoded in encoder.take_encoded()? {
                files.push(encoded, &encoder)?;
            }
        }
    }
    for encoded in encoder.finish()? {
        files.push(encoded, &encoder)?;
    }
    files.finish(&encoder)
}

/// The cue and highlighted word on screen at output time `t`.
fn caption_at(cues: &[Cue], t: Micros, style: CaptionStyle) -> Option<Shown> {
    if style == CaptionStyle::Off {
        return None;
    }
    let (_, word) = captions::shown_at(cues, t)?;
    let cue = cues.partition_point(|c| c.from() <= t) - 1;
    let pops = style == CaptionStyle::WordPop;
    let since = Micros(t.0.saturating_sub(cues[cue].words[word].from.0));
    Some(Shown {
        cue,
        word: pops.then_some(word),
        pop: if pops {
            since.min(kinetic::word_pop_settle())
        } else {
            kinetic::word_pop_settle()
        },
    })
}

/// What a derived chunk adds to its own key: the base chunk, which of its frames, the window per
/// frame and the caption per frame, in runs.
fn key_input(chunk: &DerivedChunk, frames: &Range<u64>, cues: &[Cue]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(chunk.base.hash.as_bytes());
    out.extend_from_slice(&(frames.start - chunk.base.frames.start).to_le_bytes());
    out.extend_from_slice(&(frames.end - frames.start).to_le_bytes());
    for window in &chunk.windows {
        out.extend_from_slice(&window.to_le_bytes());
    }
    let mut runs: Vec<(u32, Option<Shown>)> = Vec::new();
    for caption in &chunk.captions {
        match runs.last_mut() {
            Some((count, last)) if last == caption => *count += 1,
            _ => runs.push((1, *caption)),
        }
    }
    for (count, caption) in runs {
        out.extend_from_slice(&count.to_le_bytes());
        match caption {
            None => out.push(0),
            Some(Shown { cue, word, pop }) => {
                out.push(1);
                let text = cues[cue].text();
                out.extend_from_slice(&(text.len() as u64).to_le_bytes());
                out.extend_from_slice(text.as_bytes());
                out.extend_from_slice(&word.map_or(u32::MAX, |w| w as u32).to_le_bytes());
                out.extend_from_slice(&pop.0.to_le_bytes());
            }
        }
    }
    out
}

/// The burned-in caption of `cue` on a derived canvas.
fn caption_shape(cue: &Cue, word: Option<usize>, pop: Micros, canvas: Canvas) -> Shape {
    let tall = canvas.height > canvas.width;
    let size = canvas.width as f32 * if tall { 0.062 } else { 0.05 };
    Shape::Caption {
        center: [
            canvas.width as f32 / 2.0,
            canvas.height as f32 * if tall { 0.72 } else { 0.83 },
        ],
        width: canvas.width as f32 * 0.84,
        words: cue.words.iter().map(|w| w.text.clone()).collect(),
        highlight: word,
        size,
        color: Rgba::hex(0xFFFFFF, 1.0),
        accent: Rgba::hex(0xC8F25A, 1.0),
        background: Rgba::hex(0x0A0B0D, 0.78),
        pop: kinetic::word_pop(pop),
    }
}

/// The window's left edge on the main canvas for every frame of `parent`, a main segment.
fn windows(
    parent: &PlannedSegment,
    effects: &Effects,
    faces: Option<&FaceTrack>,
    width: f32,
    fps: u32,
) -> Vec<u16> {
    let segment = &parent.segment;
    let centered = ((MAIN.width as f32 - width) / 2.0).round() as u16;
    let count = (parent.source_frames.end - parent.source_frames.start) as usize;
    let spec = LayoutSpec::styled(segment.layout, &effects.window).ok();
    let (Some(spec), Some(faces)) = (spec, faces) else {
        return vec![centered; count];
    };
    if !spec.uses(Source::Camera) {
        return vec![centered; count];
    }
    let screen = (MAIN.width, MAIN.height);
    // The face's rectangle on the main canvas at each sample where Vision found one.
    let placed: Vec<(Micros, Rect)> = faces
        .samples
        .iter()
        .filter_map(|(t, face)| {
            let face = (*face)?;
            let punch = punch_at(&segment.zooms, *t, effects.zoom_motion, screen);
            let scene = spec.scene(
                MAIN,
                Sources {
                    screen,
                    camera: faces.size,
                },
                punch,
            );
            let layer = scene.layers.iter().find(|l| l.source == Source::Camera)?;
            let map = |u: f32, v: f32| {
                [
                    layer.frame.left
                        + (u - layer.crop.left) / layer.crop.width() * layer.frame.width(),
                    layer.frame.top
                        + (v - layer.crop.top) / layer.crop.height() * layer.frame.height(),
                ]
            };
            let [left, top] = map(face.x, face.y);
            let [right, bottom] = map(face.x + face.w, face.y + face.h);
            let rect = Rect::new(left, top, right, bottom);
            let [cx, cy] = [(left + right) / 2.0, (top + bottom) / 2.0];
            layer.frame.contains([cx, cy]).then_some((*t, rect))
        })
        .collect();
    if placed.is_empty() {
        return vec![centered; count];
    }
    // Each sample's center averaged with its neighbors within the smoothing window.
    let smoothed: Vec<(Micros, f32, Rect)> = placed
        .iter()
        .map(|(t, rect)| {
            let near: Vec<f32> = placed
                .iter()
                .filter(|(u, _)| u.0.abs_diff(t.0) <= SMOOTHING.0)
                .map(|(_, r)| (r.left + r.right) / 2.0)
                .collect();
            (*t, near.iter().sum::<f32>() / near.len() as f32, *rect)
        })
        .collect();
    parent
        .source_frames
        .clone()
        .map(|frame| {
            let t = frame_time(frame, fps);
            let (center, face) = interpolate(&smoothed, t);
            let mut left = center - width / 2.0;
            if face.width() <= width {
                left = left.clamp(face.right - width, face.left);
            }
            left.clamp(0.0, MAIN.width as f32 - width).round() as u16
        })
        .collect()
}

/// The smoothed center and the face rectangle at `t`, linear between samples and held beyond
/// the first and the last.
fn interpolate(samples: &[(Micros, f32, Rect)], t: Micros) -> (f32, Rect) {
    let after = samples.partition_point(|(u, _, _)| *u <= t);
    if after == 0 {
        return (samples[0].1, samples[0].2);
    }
    if after == samples.len() {
        let last = samples[after - 1];
        return (last.1, last.2);
    }
    let (a, b) = (samples[after - 1], samples[after]);
    let p = (t.0 - a.0.0) as f32 / (b.0.0 - a.0.0) as f32;
    let lerp = |x: f32, y: f32| x + (y - x) * p;
    (
        lerp(a.1, b.1),
        Rect::new(
            lerp(a.2.left, b.2.left),
            lerp(a.2.top, b.2.top),
            lerp(a.2.right, b.2.right),
            lerp(a.2.bottom, b.2.bottom),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Cut, FormatKind, Project, Script, Timeline, Video};

    fn project(cut: &str) -> Project {
        Project {
            video: Video::parse("title \"t\"\nstatus editing\nformat opinion\n").unwrap(),
            script: Script::parse("# t\n").unwrap(),
            format: FormatKind::Opinion.preset(),
            cut: Cut::parse(cut).unwrap(),
        }
    }

    fn plan(project: &Project, faces: &HashMap<Id, FaceTrack>) -> DerivedPlan {
        let timeline = Timeline::evaluate(project);
        DerivedPlan::new(
            timeline.main(),
            &timeline.outputs[1],
            &DerivedInputs {
                fps: 60,
                media: &|_| MediaId::default(),
                assets: &|_| Vec::new(),
                faces,
                transcripts: &HashMap::new(),
            },
        )
    }

    const CUT: &str = r#"
chapter "One" {
    take t01 {
        keep from=0 to=10_000_000
        layout face from=0 to=10_000_000
    }
}

short s {
    clip t01 from=2_100_000 to=6_000_000
}
"#;

    #[test]
    fn every_derived_chunk_reuses_the_main_chunk_around_it() {
        let plan = plan(&project(CUT), &HashMap::new());
        let main_hashes: Vec<ContentHash> = plan.main.segments[0]
            .chunks
            .iter()
            .map(|c| c.hash)
            .collect();
        // 2.1 s to 6 s: frames 126 to 360, the first chunk cut short at the clip's edge.
        let chunks = &plan.chunks[0];
        assert_eq!(chunks.len(), 8);
        assert_eq!(chunks[0].base.frames, 120..150);
        assert!(chunks.iter().all(|c| main_hashes.contains(&c.base.hash)));
        assert_eq!(plan.bases().len(), 8);
        // Without faces the window stays centered: (3840 - 1215) / 2.
        assert!(chunks.iter().all(|c| c.windows.iter().all(|&w| w == 1313)));
    }

    #[test]
    fn the_window_follows_the_face_and_keeps_it_inside() {
        let id = Id::new("t01").unwrap();
        // A face moving from the left edge to the right edge of the camera over 10 s.
        let samples = (0..=20)
            .map(|i| {
                let x = i as f32 / 20.0 * 0.9;
                (
                    Micros(i * 500_000),
                    Some(NormRect {
                        x,
                        y: 0.3,
                        w: 0.1,
                        h: 0.2,
                    }),
                )
            })
            .collect();
        let faces = HashMap::from([(
            id,
            FaceTrack {
                size: (1920, 1080),
                samples,
            },
        )]);
        let plan = plan(&project(CUT), &faces);
        let width = window_width(DerivedKind::Short);
        let segment = &plan.plan.segments[0];
        for (chunk, derived) in segment.chunks.iter().zip(&plan.chunks[0]) {
            for (frame, &left) in chunk.frames.clone().zip(&derived.windows) {
                let t = frame as f32 / 60.0;
                let face_left = t / 10.0 * 0.9 * 3840.0;
                let face_right = face_left + 384.0;
                let (left, right) = (f32::from(left), f32::from(left) + width);
                assert!(
                    left <= face_left + 40.0 && face_right - 40.0 <= right,
                    "frame {frame}: window {left}..{right}, face {face_left}..{face_right}"
                );
            }
        }
        let first = plan.chunks[0][0].windows[0];
        let last = *plan.chunks[0].last().unwrap().windows.last().unwrap();
        assert!(last > first + 500, "{first} to {last}");
    }

    #[test]
    fn a_new_caption_or_window_re_keys_only_its_chunks() {
        let mut transcripts = HashMap::new();
        let id = Id::new("t01").unwrap();
        let mut transcript = Transcript::new(id.clone());
        transcript
            .push(shot_core::Word {
                text: "Hello.".into(),
                range: shot_core::TimeRange::new(Micros(5_000_000), Micros(5_400_000)).unwrap(),
                confidence: 90,
            })
            .unwrap();
        transcripts.insert(id, transcript);
        let project = project(CUT);
        let timeline = Timeline::evaluate(&project);
        let keys = |transcripts: &HashMap<Id, Transcript>| -> Vec<ContentHash> {
            DerivedPlan::new(
                timeline.main(),
                &timeline.outputs[1],
                &DerivedInputs {
                    fps: 60,
                    media: &|_| MediaId::default(),
                    assets: &|_| Vec::new(),
                    faces: &HashMap::new(),
                    transcripts,
                },
            )
            .plan
            .segments[0]
                .chunks
                .iter()
                .map(|c| c.hash)
                .collect()
        };
        let without = keys(&HashMap::new());
        let with = keys(&transcripts);
        let changed: Vec<usize> = (0..with.len()).filter(|&i| with[i] != without[i]).collect();
        // The word shows from 5 s to 5.7 s: the chunks from 5 s and 5.5 s.
        assert_eq!(changed, [6, 7]);
    }
}
