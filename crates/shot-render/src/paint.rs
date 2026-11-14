//! Draws output frames of an [`ExportPlan`] for export and preview alike: the piece at each
//! frame with the tracks it needs decoded, the bookends, the end card, and the transitions
//! between pieces ([`crate::join`]) and between layouts inside a take. Any frame draws the same
//! whether it is reached by playing or by seeking: everything it shows is a function of the plan
//! and the frame.
//!
//! Every transition draws through one path, whichever edge it is on: two sides, the outgoing and
//! the incoming one, and a window with the place where one gives way to the other. Between pieces
//! the sides are the two pieces and the window is centered on the edge; at a layout switch they
//! are the same moment of the take in the old and the new layout, and the window starts on the
//! edge, its first frame the old layout's as it is.
//!
//! | Transition | How a frame of it draws |
//! | --- | --- |
//! | `dip` | its own side under the brand's background, opaque at the cut |
//! | `dissolve` | the outgoing side, the incoming one over it fading in |
//! | `mask-wipe` | the outgoing side, the incoming one inside a shape that opens from the middle, the camera's shape or a circle, edged in the accent |
//! | `zoom-through` | its own side zooming in towards the cut and out after it, motion blurred |
//! | `stinger` | its own side under the brand's Lottie stinger, which covers the cut at its peak |
//! | `morph` | one shot, every slot sprung from the old layout into the new, drawn by [`compose`]; only at a layout switch |
//!
//! Both sides of a dissolve or mask wipe decode at once: the painter keeps two cursors per file,
//! so two sides of one take each decode on instead of seeking back and forth. A prefetching
//! painter, the preview's, decodes the incoming side's first frame ahead of the window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use shot_core::{CameraShape, Effects, Id, Layout, Micros, TimeRange, TransitionKind};
use shot_media::videotoolbox::{DecodeError, Decoder};

use crate::RenderError;
use crate::assets::{AssetError, Assets};
use crate::cursor::FrameCursor;
use crate::drawing::{Drawing, Shape};
use crate::frame::{Frame, Look, Shot, compose};
use crate::graph::Renderer;
use crate::join::{JoinFrame, Piece};
use crate::layout::{Border, Canvas, Layer, Rect, Scene, Source, Sources, UnsupportedLayout};
use crate::lottie::{Filled, LottieError, LottieTemplate};
use crate::motion::Part;
use crate::plan::{ExportPlan, frame_time};
use crate::surface::{BufferPool, Matrix, Range, size};
use crate::theme::Theme;

/// How far a zoom-through punches in at the cut.
const ZOOM_THROUGH: f32 = 2.6;
/// The motion blur shutter, in frames: 1/120 s at 60 fps, as for every other move.
const SHUTTER_FRAMES: f32 = 0.5;
/// How many frames ahead of a two-sided transition a prefetching painter decodes its incoming
/// side, half a second.
pub const PREFETCH: u64 = 30;

/// The tracks of a take, by file name inside `media/<take>/`.
const SCREEN_FILE: &str = "screen.mp4";
const CAMERA_FILE: &str = "camera.mp4";

#[derive(Debug, thiserror::Error)]
pub enum PaintError {
    #[error("{path}: {source}")]
    Decode { path: PathBuf, source: DecodeError },
    #[error("{path}: {source}")]
    Lottie { path: PathBuf, source: LottieError },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Assets(#[from] AssetError),
    #[error(transparent)]
    Layout(#[from] UnsupportedLayout),
    #[error("output frame {0} is past the end")]
    Outside(u64),
}

/// Drawn on every segment frame after its overlays, at the take time, for benches. Export never
/// sets one and chunk keys do not know it.
pub type Decoration = fn(Micros, Canvas, &mut Frame);

/// See the module docs. One per render thread.
pub struct Painter {
    dir: PathBuf,
    /// The channel repo, which bumper paths are relative to.
    channel: PathBuf,
    effects: Effects,
    assets: Assets,
    tracks: Tracks,
    lotties: HashMap<PathBuf, Arc<Filled>>,
    /// Holds one side of a transition while the other draws.
    scratch: Option<(Target, BufferPool)>,
    decoration: Option<Decoration>,
    prefetch: bool,
    /// The window start of the transition last prefetched for.
    prefetched: Option<u64>,
}

/// The size and encoding of a render target.
type Target = (u32, u32, Range, Matrix);

impl Painter {
    /// A painter for the video in `video_dir` with the format's `effects`, its theme read from
    /// the folder on first use.
    pub fn new(video_dir: &Path, effects: Effects) -> Self {
        Self::with_assets(video_dir, effects, Assets::new(video_dir))
    }

    /// A painter whose theme is already read.
    pub fn with_theme(video_dir: &Path, effects: Effects, theme: Arc<Theme>) -> Self {
        Self::with_assets(video_dir, effects, Assets::with_theme(video_dir, theme))
    }

    fn with_assets(video_dir: &Path, effects: Effects, assets: Assets) -> Self {
        Self {
            dir: video_dir.to_owned(),
            channel: crate::theme::channel_root(video_dir)
                .unwrap_or(video_dir)
                .to_owned(),
            effects,
            assets,
            tracks: Tracks::default(),
            lotties: HashMap::new(),
            scratch: None,
            decoration: None,
            prefetch: false,
            prefetched: None,
        }
    }

    /// Draws `decoration` on every segment frame from now on.
    pub fn set_decoration(&mut self, decoration: Decoration) {
        self.decoration = Some(decoration);
    }

    /// Prefetches from now on: [`PREFETCH`] frames ahead of a dissolve or mask wipe between
    /// pieces, decodes the incoming side's first frame, so the window starts without a seek. For
    /// playback; export paints in order and gains nothing.
    pub fn set_prefetch(&mut self, on: bool) {
        self.prefetch = on;
    }

    /// Draws output frame `frame` of `plan` into `target`, at the target's size, and returns
    /// where the take's screen recording sits in it: `None` in a bookend, a transition, a b-roll
    /// cutaway or a layout without the screen.
    pub fn paint(
        &mut self,
        renderer: &Renderer,
        plan: &ExportPlan,
        frame: u64,
        target: &CVPixelBuffer,
    ) -> Result<Option<ScreenLayer>, PaintError> {
        let (width, height) = size(target);
        let canvas = Canvas { width, height };
        let piece = plan.piece_at(frame).ok_or(PaintError::Outside(frame))?;
        let theme = self.assets.theme()?;
        let crossing = match plan.join_at(frame) {
            Some(join) => Some(Crossing::of_join(&join, frame, plan.fps)),
            None => Crossing::of_switch(plan, piece, frame, &theme),
        };
        let screen = match crossing {
            Some(crossing) => {
                self.cross(plan, frame, canvas, &crossing, renderer, target)?;
                None
            }
            None => {
                let drawn = self.side(plan, Side::of(piece), frame, canvas)?;
                drawn.draw(renderer, target)?;
                drawn.screen
            }
        };
        if self.prefetch {
            self.prefetch(plan, frame, canvas)?;
        }
        Ok(screen)
    }

    /// Draws a frame inside a transition on either edge.
    fn cross(
        &mut self,
        plan: &ExportPlan,
        frame: u64,
        canvas: Canvas,
        crossing: &Crossing,
        renderer: &Renderer,
        target: &CVPixelBuffer,
    ) -> Result<(), PaintError> {
        let theme = self.assets.theme()?;
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        // A layout switch's first frame, before anything moves, is the outgoing side as it is.
        if crossing.progress == 0.0 {
            let outgoing = self.side(plan, crossing.outgoing, frame, canvas)?;
            return Ok(outgoing.draw(renderer, target)?);
        }
        match crossing.kind {
            TransitionKind::Dip => {
                let mut own = self.side(plan, crossing.own(), frame, canvas)?;
                own.frame.drawing.over.push(Shape::Rect {
                    rect: full,
                    radius: 0.0,
                    color: theme
                        .palette
                        .background
                        .faded(ease(crossing.toward(crossing.at))),
                });
                own.draw(renderer, target)?;
            }
            TransitionKind::Stinger => {
                let mut own = self.side(plan, crossing.own(), frame, canvas)?;
                if let Some(stinger) = &theme.stinger {
                    let (w, h) = stinger.lottie.size();
                    let scale = (full.width() / w as f32).max(full.height() / h as f32);
                    own.frame.drawing.over.push(Shape::Lottie {
                        lottie: stinger.lottie.clone(),
                        origin: [
                            (full.width() - w as f32 * scale) / 2.0,
                            (full.height() - h as f32 * scale) / 2.0,
                        ],
                        scale,
                        elapsed: crossing.elapsed,
                        opacity: 1.0,
                        font: theme.fonts.body(),
                    });
                }
                own.draw(renderer, target)?;
            }
            TransitionKind::ZoomThrough => {
                let scratch = self.scratch(target)?;
                self.side(plan, crossing.own(), frame, canvas)?
                    .draw(renderer, &scratch)?;
                zoom_through(&theme, canvas, crossing, &scratch, renderer, target)?;
            }
            TransitionKind::Dissolve | TransitionKind::MaskWipe => {
                let outgoing = self.side(plan, crossing.outgoing, frame, canvas)?;
                let incoming = self.side(plan, crossing.incoming, frame, canvas)?;
                outgoing.draw(renderer, target)?;
                let scratch = self.scratch(target)?;
                incoming.draw(renderer, &scratch)?;
                let x = ease(crossing.progress);
                let layer = match crossing.kind {
                    TransitionKind::Dissolve => Layer {
                        opacity: x,
                        ..Layer::new(Source::Screen, full, Rect::UNIT, 0.0)
                    },
                    _ => wipe(&theme, canvas, x),
                };
                let over = Frame {
                    scene: Scene {
                        background: background(&theme),
                        layers: vec![layer],
                    },
                    drawing: Drawing::default(),
                    cutaway: None,
                    cutout: None,
                };
                renderer.draw_over(&over, only_screen(&scratch), target)?;
            }
            // A morph is one shot that `compose` draws; a cut draws nothing.
            TransitionKind::Cut | TransitionKind::Morph => self
                .side(plan, crossing.own(), frame, canvas)?
                .draw(renderer, target)?,
        }
        Ok(())
    }

    /// Decodes the incoming side of the next dissolve or mask wipe between pieces at its first
    /// frame, once, when its window starts within [`PREFETCH`] frames of `frame`.
    fn prefetch(
        &mut self,
        plan: &ExportPlan,
        frame: u64,
        canvas: Canvas,
    ) -> Result<(), PaintError> {
        let next = plan.transitions.iter().find(|t| {
            t.two_sided() && frame < t.window.start && t.window.start <= frame + PREFETCH
        });
        let Some(transition) = next else {
            return Ok(());
        };
        let start = transition.window.start;
        if self.prefetched == Some(start) {
            return Ok(());
        }
        self.prefetched = Some(start);
        let incoming = Side {
            own: false,
            ..Side::of(transition.after)
        };
        self.side(plan, incoming, start, canvas)?;
        Ok(())
    }

    /// A buffer like `target` to draw one side into.
    fn scratch(
        &mut self,
        target: &CVPixelBuffer,
    ) -> Result<CFRetained<CVPixelBuffer>, RenderError> {
        let (width, height) = size(target);
        let key = (width, height, Range::of(target)?, Matrix::of(target));
        if self.scratch.as_ref().is_none_or(|(k, _)| *k != key) {
            self.scratch = Some((key, BufferPool::new(width, height, key.2, key.3)?));
        }
        self.scratch.as_ref().expect("made above").1.buffer()
    }

    /// What `side` shows at output frame `frame`.
    fn side(
        &mut self,
        plan: &ExportPlan,
        side: Side,
        frame: u64,
        canvas: Canvas,
    ) -> Result<Drawn, PaintError> {
        let position = plan.position(side.piece, frame);
        let mut drawn = match side.piece {
            Piece::Segment(i) => self.segment(plan, i, position, canvas, side.layout)?,
            Piece::Bookend(i) => self.bookend(plan, i, position, canvas)?,
        };
        if side.own
            && let Some(card) = &plan.end_card
            && card.contains(&frame)
        {
            self.end_card(
                plan,
                frame - card.start,
                card.end - card.start,
                canvas,
                &mut drawn,
            )?;
        }
        Ok(drawn)
    }

    /// Segment `index` at `source_frame`, in its own layout and switch, or in `layout` alone for
    /// one side of a two-sided scene switch.
    fn segment(
        &mut self,
        plan: &ExportPlan,
        index: usize,
        source_frame: u64,
        canvas: Canvas,
        layout: Option<Layout>,
    ) -> Result<Drawn, PaintError> {
        let segment = &plan.segments[index].segment;
        let theme = self.assets.theme()?;
        let take = self.dir.join("media").join(segment.take.as_str());
        // A take recorded without the camera or the screen draws the layout it can.
        let tracks = (
            take.join(SCREEN_FILE).exists(),
            take.join(CAMERA_FILE).exists(),
        );
        let time = frame_time(source_frame, plan.fps);
        let switch = match layout {
            Some(_) => None,
            None => segment.switch.as_ref().filter(|s| {
                s.transition.kind == TransitionKind::Morph
                    && s.window.from() <= time
                    && time < s.window.to()
            }),
        };
        let layout = layout
            .unwrap_or(segment.layout)
            .for_tracks(tracks.0, tracks.1);
        let mut specs = vec![theme.layout(layout, &self.effects.window)?];
        if let Some(switch) = switch {
            let from = switch.from.for_tracks(tracks.0, tracks.1);
            specs.push(theme.layout(from, &self.effects.window)?);
        }
        let mut decoded = Sources::<Option<Decoded>>::default();
        for (source, file) in [(Source::Screen, SCREEN_FILE), (Source::Camera, CAMERA_FILE)] {
            let path = take.join(file);
            // A take recorded without a camera draws its layouts without the camera slot.
            if !specs.iter().any(|s| s.uses(source)) || !path.exists() {
                continue;
            }
            let frame = self.tracks.frame(&path, duration(time))?;
            match source {
                Source::Screen => decoded.screen = frame,
                Source::Camera => decoded.camera = frame,
            }
        }
        let sizes = Sources {
            screen: decoded.screen.as_ref().map_or((16, 9), |d| d.size),
            camera: decoded.camera.as_ref().map_or((16, 9), |d| d.size),
        };
        let look = self
            .assets
            .look(&segment.take, &segment.overlays, self.effects)?;
        let shot = Shot {
            layout,
            zooms: &segment.zooms,
            overlays: &segment.overlays,
            time,
            switch,
        };
        let mut frame = compose(&shot, &look, canvas, sizes)?;
        if let Some(decorate) = self.decoration {
            decorate(time, canvas, &mut frame);
        }
        let layer = frame
            .scene
            .layers
            .iter()
            .find(|l| l.source == Source::Screen);
        let screen_layer = match (&frame.cutaway, layer, &decoded.screen) {
            (None, Some(layer), Some(decoded)) => Some(ScreenLayer {
                frame: layer.frame,
                crop: layer.crop,
                size: decoded.size,
            }),
            _ => None,
        };
        let mut screen = decoded.screen.map(|d| d.buffer);
        // A b-roll clip shows in the screen layer instead of the take's screen.
        if let Some(cutaway) = &frame.cutaway {
            let path = self.dir.join(&cutaway.file);
            let clip = self.tracks.frame(&path, duration(cutaway.time))?;
            if let Some(clip) = &clip {
                crate::overlay::broll::fit_cutaway(&mut frame, clip.size);
            }
            screen = clip.map(|c| c.buffer);
        }
        let sources = Sources {
            screen,
            camera: decoded.camera.map(|d| d.buffer),
        };
        frame
            .scene
            .layers
            .retain(|layer| sources.get(layer.source).is_some());
        Ok(Drawn {
            frame,
            sources,
            card: None,
            screen: screen_layer,
        })
    }

    fn bookend(
        &mut self,
        plan: &ExportPlan,
        index: usize,
        local: u64,
        canvas: Canvas,
    ) -> Result<Drawn, PaintError> {
        let bookend = &plan.bookends[index];
        let theme = self.assets.theme()?;
        let t = frame_time(local, plan.fps);
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let mut frame = Frame {
            scene: Scene {
                background: background(&theme),
                layers: Vec::new(),
            },
            drawing: Drawing::default(),
            cutaway: None,
            cutout: None,
        };
        let mut screen = None;
        if let Some(bumper) = &bookend.bumper {
            let path = self.channel.join(bumper);
            if is_lottie(bumper) {
                let lottie = self.lottie(&path)?;
                let (w, h) = lottie.size();
                let scale = (full.width() / w as f32).max(full.height() / h as f32);
                frame.drawing.over.push(Shape::Lottie {
                    lottie,
                    origin: [
                        (full.width() - w as f32 * scale) / 2.0,
                        (full.height() - h as f32 * scale) / 2.0,
                    ],
                    scale,
                    elapsed: t,
                    opacity: 1.0,
                    font: theme.fonts.body(),
                });
            } else if let Some(decoded) = self.tracks.frame(&path, duration(t))? {
                let (w, h) = decoded.size;
                let crop = crate::overlay::broll::cover(
                    w as f32 / h.max(1) as f32,
                    full.width() / full.height(),
                    1.0,
                );
                frame
                    .scene
                    .layers
                    .push(Layer::new(Source::Screen, full, crop, 0.0));
                screen = Some(decoded.buffer);
            }
        }
        if let Some(text) = &bookend.title {
            let length = frame_time(bookend.frames, plan.fps);
            crate::bookend::title(&theme, text, t, length, canvas, &mut frame.drawing);
        }
        Ok(Drawn {
            frame,
            sources: Sources {
                screen,
                camera: None,
            },
            card: None,
            screen: None,
        })
    }

    /// The end card `at` frames into its `length`, in a pass of its own over everything
    /// `drawn` shows, so the brand's logo can sit on its backdrop: the template's shapes go
    /// under the logo's sheet, which comes in after them.
    fn end_card(
        &mut self,
        plan: &ExportPlan,
        at: u64,
        length: u64,
        canvas: Canvas,
        drawn: &mut Drawn,
    ) -> Result<(), PaintError> {
        let Some(range) = TimeRange::new(Micros(0), frame_time(length, plan.fps)) else {
            return Ok(());
        };
        let theme = self.assets.theme()?;
        let overlay =
            crate::overlay::overlay(crate::overlay::end_card::TEMPLATE, range, Vec::new());
        let look = Look {
            effects: self.effects,
            theme: theme.clone(),
            // Templates look chapters up by take; an end card shows none.
            take: plan
                .segments
                .first()
                .map(|s| s.segment.take.clone())
                .or(Id::new("t")),
            ..Look::default()
        };
        let mut card = Frame {
            scene: Scene {
                background: background(&theme),
                layers: Vec::new(),
            },
            drawing: Drawing::default(),
            cutaway: None,
            cutout: None,
        };
        let t = frame_time(at, plan.fps);
        crate::overlay::draw_alone(&overlay, t, canvas, &look, &mut card);
        card.drawing.under = std::mem::take(&mut card.drawing.over);
        if let Some(logo) = &theme.logo {
            let s = canvas.width as f32 / 3840.0;
            let headline = crate::overlay::end_card::EndScreen::of(canvas).headline;
            let height = 180.0 * s;
            let width = height * logo.image.width as f32 / logo.image.height.max(1) as f32;
            let center = [
                headline[0] + width / 2.0,
                headline[1] - 80.0 * s - height / 2.0,
            ];
            // After the template's own parts: backdrop, headline, button and two videos.
            let place = |t| {
                let pose = theme.motion.pose(range, Part::new(5, 6), t);
                (0.85 + 0.15 * pose.enter, pose.presence())
            };
            card.drawing.sheets.extend(crate::bookend::logo_sheet(
                logo,
                center,
                height,
                crate::materials::shutter(t).map(place),
            ));
        }
        drawn.card = Some(card);
        Ok(())
    }

    /// A Lottie bumper, read and checked once.
    fn lottie(&mut self, path: &Path) -> Result<Arc<Filled>, PaintError> {
        if let Some(lottie) = self.lotties.get(path) {
            return Ok(lottie.clone());
        }
        let bytes = std::fs::read(path).map_err(|source| PaintError::Io {
            path: path.to_owned(),
            source,
        })?;
        let error = |source| PaintError::Lottie {
            path: path.to_owned(),
            source,
        };
        let filled = Arc::new(
            LottieTemplate::load(&bytes, &[])
                .and_then(|t| t.fill(&[]))
                .map_err(error)?,
        );
        self.lotties.insert(path.to_owned(), filled.clone());
        Ok(filled)
    }
}

/// Whether a brand file is a Lottie rather than a video.
pub fn is_lottie(file: &str) -> bool {
    let file = file.to_ascii_lowercase();
    file.ends_with(".json") || file.ends_with(".lottie")
}

/// A composed frame with the decoded frames its layers draw from, and the end card over it.
struct Drawn {
    frame: Frame,
    sources: Sources<Option<CFRetained<CVPixelBuffer>>>,
    card: Option<Frame>,
    screen: Option<ScreenLayer>,
}

/// Where the take's screen recording sits in a painted frame, so a point on the frame maps back
/// to the recording's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenLayer {
    /// Canvas pixels.
    pub frame: Rect,
    /// Texture coordinates of the recording.
    pub crop: Rect,
    /// The recording's size in pixels.
    pub size: (u32, u32),
}

/// One piece as a side of a frame: a segment in its own layout or in `layout`, for one side of
/// a layout switch, with the end card over it when `own`, the piece the frame belongs to.
#[derive(Debug, Clone, Copy)]
struct Side {
    piece: Piece,
    layout: Option<Layout>,
    own: bool,
}

impl Side {
    /// `piece` as the frame's own, in its own layout.
    fn of(piece: Piece) -> Self {
        Self {
            piece,
            layout: None,
            own: true,
        }
    }
}

/// A frame inside a transition, on a chapter edge or a layout switch, in frames of its window.
/// Positions run from the window's start: `at` is where the frame samples, `cut` where the
/// outgoing side gives way to the incoming one, `length` the window's end.
#[derive(Debug, Clone, Copy)]
struct Crossing {
    kind: TransitionKind,
    at: f32,
    cut: f32,
    length: f32,
    /// How far through the window the frame is, 0 to 1.
    progress: f32,
    /// Since the window's start, for the stinger.
    elapsed: Micros,
    outgoing: Side,
    incoming: Side,
}

impl Crossing {
    /// A frame inside a transition between pieces, centered on its edge. Frames sample at their
    /// middle.
    fn of_join(join: &JoinFrame, frame: u64, fps: u32) -> Self {
        let t = join.transition;
        let start = t.window.start;
        Self {
            kind: t.kind,
            at: frame as f32 + 0.5 - start as f32,
            cut: (t.edge - start) as f32,
            length: (t.window.end - start) as f32,
            progress: join.progress,
            elapsed: frame_time(frame - start, fps),
            outgoing: Side {
                own: join.before_edge,
                ..Side::of(t.before)
            },
            incoming: Side {
                own: !join.before_edge,
                ..Side::of(t.after)
            },
        }
    }

    /// The layout switch output frame `frame` of `piece` is inside, starting on its edge. Frames
    /// sample at their start, so the edge's frame is the outgoing layout's. The layouts give way
    /// in the window's middle, or at a stinger's peak.
    fn of_switch(plan: &ExportPlan, piece: Piece, frame: u64, theme: &Theme) -> Option<Self> {
        let Piece::Segment(index) = piece else {
            return None;
        };
        let switch = plan.segments[index].segment.switch.as_ref()?;
        let kind = switch.transition.kind;
        let time = frame_time(plan.position(piece, frame), plan.fps);
        let inside = switch.window.from() <= time && time < switch.window.to();
        if !inside || matches!(kind, TransitionKind::Cut | TransitionKind::Morph) {
            return None;
        }
        let frames = |t: Micros| t.0 as f32 / 1e6 * plan.fps as f32;
        let length = frames(switch.window.duration());
        let progress = switch.progress(time);
        let cut = match (&theme.stinger, kind) {
            (Some(stinger), TransitionKind::Stinger) => frames(stinger.peak).min(length),
            _ => length / 2.0,
        };
        let side = |layout| Side {
            layout: Some(layout),
            ..Side::of(piece)
        };
        Some(Self {
            kind,
            at: progress * length,
            cut,
            length,
            progress,
            elapsed: Micros(time.0 - switch.window.from().0),
            outgoing: side(switch.from),
            incoming: side(switch.to),
        })
    }

    /// The side a one-sided kind draws at the frame.
    fn own(&self) -> Side {
        if self.at < self.cut {
            self.outgoing
        } else {
            self.incoming
        }
    }

    /// How far towards the cut position `at` is, on its side of it: 0 at the window's ends, 1
    /// at the cut.
    fn toward(&self, at: f32) -> f32 {
        let value = if at < self.cut {
            at / self.cut
        } else {
            (self.length - at) / (self.length - self.cut)
        };
        value.clamp(0.0, 1.0)
    }
}

impl Drawn {
    fn draw(&self, renderer: &Renderer, target: &CVPixelBuffer) -> Result<(), RenderError> {
        renderer.draw(
            &self.frame,
            Sources {
                screen: self.sources.screen.as_deref(),
                camera: self.sources.camera.as_deref(),
            },
            target,
        )?;
        match &self.card {
            Some(card) => renderer.draw_over(
                card,
                Sources {
                    screen: None,
                    camera: None,
                },
                target,
            ),
            None => Ok(()),
        }
    }
}

fn only_screen(buffer: &CVPixelBuffer) -> Sources<Option<&CVPixelBuffer>> {
    Sources {
        screen: Some(buffer),
        camera: None,
    }
}

fn background(theme: &Theme) -> [f32; 3] {
    let [r, g, b, _] = theme.palette.background.0;
    [r, g, b]
}

fn duration(t: Micros) -> Duration {
    Duration::from_micros(t.0)
}

/// Smoothstep: slow out of 0, slow into 1.
fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Draws `scratch`, one side's frame, punched in towards its middle by how far the crossing's
/// frame is towards the cut, motion blurred over the shutter.
fn zoom_through(
    theme: &Theme,
    canvas: Canvas,
    crossing: &Crossing,
    scratch: &CVPixelBuffer,
    renderer: &Renderer,
    target: &CVPixelBuffer,
) -> Result<(), RenderError> {
    let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
    let crop = |at: f32| {
        let scale = 1.0 + (ZOOM_THROUGH - 1.0) * crossing.toward(at).powi(3);
        let half = 0.5 / scale;
        Rect::new(0.5 - half, 0.5 - half, 0.5 + half, 0.5 + half)
    };
    let at = crossing.at;
    let [open, close] = [-SHUTTER_FRAMES / 2.0, SHUTTER_FRAMES / 2.0].map(|d| crop(at + d));
    let layer = Layer {
        shutter: Rect::new(
            close.left - open.left,
            close.top - open.top,
            close.right - open.right,
            close.bottom - open.bottom,
        ),
        ..Layer::new(Source::Screen, full, crop(at), 0.0)
    };
    let zoomed = Frame {
        scene: Scene {
            background: background(theme),
            layers: vec![layer],
        },
        drawing: Drawing::default(),
        cutaway: None,
        cutout: None,
    };
    renderer.draw(&zoomed, only_screen(scratch), target)
}

/// The incoming side inside the brand's shape opened `x` of the way from the middle.
fn wipe(theme: &Theme, canvas: Canvas, x: f32) -> Layer {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let s = w / 3840.0;
    let (half_w, half_h, radius) = match theme.wipe_shape() {
        CameraShape::Circle => {
            let r = x * (w.hypot(h) / 2.0 + 2.0);
            (r, r, r)
        }
        shape => {
            let (hw, hh) = (x * (w / 2.0 + 60.0 * s), x * (h / 2.0 + 60.0 * s));
            let corners = match shape {
                CameraShape::Rounded => theme.camera.map_or(0.0, |c| c.corners) * s,
                _ => 0.0,
            };
            (hw, hh, corners.min(hw.min(hh)))
        }
    };
    let frame = Rect::new(
        w / 2.0 - half_w,
        h / 2.0 - half_h,
        w / 2.0 + half_w,
        h / 2.0 + half_h,
    );
    let [r, g, b, _] = theme.palette.accent.0;
    Layer {
        border: (x < 1.0).then_some(Border {
            width: 14.0 * s,
            color: [r, g, b],
        }),
        ..Layer::new(
            Source::Screen,
            frame,
            Rect::new(
                frame.left / w,
                frame.top / h,
                frame.right / w,
                frame.bottom / h,
            ),
            radius,
        )
    }
}

/// A decoded frame and its size.
struct Decoded {
    buffer: CFRetained<CVPixelBuffer>,
    size: (u32, u32),
}

/// Video files decoded in time order, two cursors per file: a request decodes on in the cursor
/// closest before it, within a second, and otherwise seeks the cursor used least recently. So
/// both sides of a transition in one take decode on, and a seek costs one cursor.
#[derive(Default)]
struct Tracks {
    files: HashMap<PathBuf, Vec<(FrameCursor, u64)>>,
    clock: u64,
}

/// Cursors kept per file.
const CURSORS: usize = 2;

impl Tracks {
    /// The frame of the video at `path` on screen at `time`, the last one past its end, `None`
    /// for a file without frames.
    fn frame(&mut self, path: &Path, time: Duration) -> Result<Option<Decoded>, PaintError> {
        let error = |source| PaintError::Decode {
            path: path.to_owned(),
            source,
        };
        self.clock += 1;
        let cursors = self.files.entry(path.to_owned()).or_default();
        let ahead = |cursor: &FrameCursor| {
            let pts = cursor.current()?.pts;
            (pts <= time + Duration::from_millis(1) && time < pts + Duration::from_secs(1))
                .then(|| time.saturating_sub(pts))
        };
        let pick = match cursors
            .iter()
            .enumerate()
            .filter_map(|(i, (cursor, _))| ahead(cursor).map(|d| (i, d)))
            .min_by_key(|(_, d)| *d)
        {
            Some((i, _)) => i,
            None if cursors.len() < CURSORS => {
                let decoder = Decoder::open_native(path).map_err(error)?;
                cursors.push((FrameCursor::new(decoder), 0));
                cursors.len() - 1
            }
            None => cursors
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(i, _)| i)
                .expect("a file has a cursor"),
        };
        let (cursor, used) = &mut cursors[pick];
        *used = self.clock;
        Ok(cursor.goto(time).map_err(error)?.map(|frame| Decoded {
            size: size(&frame.buffer),
            buffer: frame.buffer.clone(),
        }))
    }
}
