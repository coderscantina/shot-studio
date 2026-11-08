//! The Edit screen's preview: a render thread that draws output frames of an [`EditMap`] through
//! `shot-render`'s render graph, the same decoded frames, layouts, crops and zooms export draws,
//! into `420f` BT.601 buffers, the encoding gpui's surface element decodes.
//!
//! The thread keeps a few frames ahead while playing and renders one frame while paused. A seek
//! drops what is queued. Each track has a [`FrameCursor`], export's own.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use shot_core::{Effects, Micros};
use shot_media::videotoolbox::Decoder;
use shot_render::assets::Assets;
use shot_render::cursor::FrameCursor;
use shot_render::frame::{Frame, Shot, compose};
use shot_render::graph::Renderer;
use shot_render::layout::{Canvas, LayoutSpec, Source, Sources};
use shot_render::plan::frame_time;
use shot_render::surface::{BufferPool, Matrix, Range, size};

use crate::model::{EditMap, FPS};

/// Frames rendered ahead while playing, about 100 ms.
const AHEAD: usize = 6;
/// The preview's render size. The Picture workspace shows it at 1040 points, 2080 pixels.
const CANVAS: Canvas = Canvas {
    width: 1920,
    height: 1080,
};

/// A rendered output frame.
pub struct PreviewFrame {
    pub frame: u64,
    pub buffer: CFRetained<CVPixelBuffer>,
}

// SAFETY: CoreVideo buffers are reference counted atomically and the render thread never touches
// a buffer once it is queued, so handing it to the UI thread is sound.
unsafe impl Send for PreviewFrame {}

/// Adds drawing to a composed frame, given its media time and the canvas.
pub type Decoration = fn(Micros, Canvas, &mut Frame);

/// What one preview plays: where each output frame comes from, where the takes live, and the
/// format's effects.
pub struct Plan {
    pub map: EditMap,
    pub media: PathBuf,
    pub effects: Effects,
}

#[derive(Default)]
struct State {
    plan: Option<Arc<Plan>>,
    /// Counts seeks and plan changes, so a frame rendered for an older one is dropped.
    generation: u64,
    /// The next frame to render.
    position: u64,
    playing: bool,
    /// A paused preview still owes the frame at `position`.
    owed: bool,
    queue: VecDeque<PreviewFrame>,
    /// Draws on top of every composed frame. Only the bench sets it.
    decoration: Option<Decoration>,
    error: Option<String>,
    quit: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("preview state poisoned")
    }
}

pub struct Engine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn spawn() -> Self {
        let shared = Arc::new(Shared::default());
        let thread = std::thread::Builder::new()
            .name("shot-preview".into())
            .spawn({
                let shared = shared.clone();
                move || render_loop(&shared)
            })
            .expect("spawning the preview thread");
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Plays `plan` from `frame`.
    pub fn set_plan(&self, plan: Plan, frame: u64) {
        let mut state = self.shared.lock();
        state.plan = Some(Arc::new(plan));
        state.error = None;
        Self::restart(&mut state, frame);
        self.shared.changed.notify_all();
    }

    /// Draws `decoration` on every frame from the next render on.
    pub fn set_decoration(&self, decoration: Decoration) {
        let mut state = self.shared.lock();
        state.decoration = Some(decoration);
        let frame = state.position;
        Self::restart(&mut state, frame);
        self.shared.changed.notify_all();
    }

    pub fn seek(&self, frame: u64) {
        let mut state = self.shared.lock();
        Self::restart(&mut state, frame);
        self.shared.changed.notify_all();
    }

    fn restart(state: &mut State, frame: u64) {
        state.generation += 1;
        state.queue.clear();
        state.position = frame;
        state.owed = true;
    }

    pub fn set_playing(&self, playing: bool) {
        self.shared.lock().playing = playing;
        self.shared.changed.notify_all();
    }

    /// Pops every queued frame up to `due` and returns the newest of them.
    pub fn take_due(&self, due: u64) -> Option<PreviewFrame> {
        let mut state = self.shared.lock();
        let mut newest = None;
        while state.queue.front().is_some_and(|f| f.frame <= due) {
            newest = state.queue.pop_front();
        }
        if newest.is_some() {
            self.shared.changed.notify_all();
        }
        newest
    }

    /// The first queued frame, while paused or before playback starts its clock.
    pub fn take_next(&self) -> Option<PreviewFrame> {
        let frame = self.shared.lock().queue.pop_front();
        self.shared.changed.notify_all();
        frame
    }

    /// Frames queued and the frame number at the front, so playback can start its clock.
    pub fn front(&self) -> Option<u64> {
        self.shared.lock().queue.front().map(|f| f.frame)
    }

    pub fn error(&self) -> Option<String> {
        self.shared.lock().error.clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.lock().quit = true;
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn render_loop(shared: &Shared) {
    let mut renderer = None;
    let mut pool = None;
    let mut tracks: HashMap<PathBuf, FrameCursor> = HashMap::new();
    // Event logs and snapshots, read again for each new plan.
    let mut assets: Option<(Arc<Plan>, Assets)> = None;
    loop {
        let (plan, frame, generation, decoration) = {
            let mut state = shared.lock();
            loop {
                if state.quit {
                    return;
                }
                let frames = state.plan.as_ref().map_or(0, |p| p.map.frames);
                let wants = state.position < frames
                    && state.error.is_none()
                    && (state.owed || (state.playing && state.queue.len() < AHEAD));
                if wants {
                    break;
                }
                state = shared.changed.wait(state).expect("preview state poisoned");
            }
            state.owed = false;
            let plan = state.plan.clone().expect("checked above");
            (plan, state.position, state.generation, state.decoration)
        };

        let rendered = (|| {
            let renderer = match &mut renderer {
                Some(renderer) => renderer,
                None => renderer.insert(Renderer::new().map_err(|e| e.to_string())?),
            };
            let pool = match &mut pool {
                Some(pool) => pool,
                None => pool.insert(
                    BufferPool::new(CANVAS.width, CANVAS.height, Range::Full, Matrix::Bt601)
                        .map_err(|e| e.to_string())?,
                ),
            };
            let assets = match &mut assets {
                Some((current, assets)) if Arc::ptr_eq(current, &plan) => assets,
                _ => {
                    let dir = plan.media.parent().unwrap_or(&plan.media);
                    &mut assets.insert((plan.clone(), Assets::new(dir))).1
                }
            };
            render(
                &plan,
                frame,
                decoration,
                renderer,
                pool,
                &mut tracks,
                assets,
            )
        })();

        let mut state = shared.lock();
        if state.generation != generation {
            continue;
        }
        match rendered {
            Ok(buffer) => {
                state.queue.push_back(PreviewFrame { frame, buffer });
                state.position = frame + 1;
            }
            Err(error) => state.error = Some(error),
        }
        shared.changed.notify_all();
    }
}

fn render(
    plan: &Plan,
    frame: u64,
    decoration: Option<Decoration>,
    renderer: &Renderer,
    pool: &BufferPool,
    tracks: &mut HashMap<PathBuf, FrameCursor>,
    assets: &mut Assets,
) -> Result<CFRetained<CVPixelBuffer>, String> {
    let span = plan
        .map
        .span_at(frame)
        .ok_or_else(|| format!("no segment at frame {frame}"))?;
    let spec = LayoutSpec::styled(span.layout, &plan.effects.window).map_err(|e| e.to_string())?;
    let source_frame = span.source_frame(frame);
    let time = Duration::from_micros(frame_time(source_frame, FPS).0);
    let take = plan.media.join(span.take.as_str());
    let path = |source| {
        take.join(match source {
            Source::Screen => "screen.mp4",
            Source::Camera => "camera.mp4",
        })
    };
    // A take recorded without a camera draws its layouts without the camera slot.
    let available = |source| spec.uses(source) && path(source).exists();
    for source in [Source::Screen, Source::Camera] {
        if available(source) {
            let path = path(source);
            if !tracks.contains_key(&path) {
                let decoder =
                    Decoder::open_native(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                tracks.insert(path.clone(), FrameCursor::new(decoder));
            }
            tracks
                .get_mut(&path)
                .expect("inserted above")
                .goto(time)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    let size_of = |source| {
        available(source)
            .then(|| tracks.get(&path(source)))
            .flatten()
            .and_then(FrameCursor::current)
            .map(|f| size(&f.buffer))
    };
    let sizes = Sources {
        screen: size_of(Source::Screen).unwrap_or((16, 9)),
        camera: size_of(Source::Camera).unwrap_or((16, 9)),
    };
    let look = assets
        .look(&span.take, &span.overlays, plan.effects)
        .map_err(|e| e.to_string())?;
    let shot = Shot {
        layout: span.layout,
        zooms: &span.zooms,
        overlays: &span.overlays,
        time: frame_time(source_frame, FPS),
    };
    let mut drawn = compose(&shot, &look, CANVAS, sizes).map_err(|e| e.to_string())?;
    if let Some(decorate) = decoration {
        decorate(shot.time, CANVAS, &mut drawn);
    }
    // A b-roll clip shows in the screen layer instead of the take's screen.
    let cutaway = match &drawn.cutaway {
        Some(cutaway) => {
            let clip = plan
                .media
                .parent()
                .unwrap_or(&plan.media)
                .join(&cutaway.file);
            if !tracks.contains_key(&clip) {
                let decoder =
                    Decoder::open_native(&clip).map_err(|e| format!("{}: {e}", clip.display()))?;
                tracks.insert(clip.clone(), FrameCursor::new(decoder));
            }
            let cursor = tracks.get_mut(&clip).expect("inserted above");
            cursor
                .goto(Duration::from_micros(cutaway.time.0))
                .map_err(|e| format!("{}: {e}", clip.display()))?;
            if let Some(frame) = cursor.current() {
                shot_render::overlay::broll::fit_cutaway(&mut drawn, size(&frame.buffer));
            }
            Some(clip)
        }
        None => None,
    };
    let frame_of = |source| {
        let path = match (&cutaway, source) {
            (Some(clip), Source::Screen) => clip.clone(),
            _ if available(source) => path(source),
            _ => return None,
        };
        tracks.get(&path).and_then(FrameCursor::current)
    };
    let frames = Sources {
        screen: frame_of(Source::Screen),
        camera: frame_of(Source::Camera),
    };
    drawn
        .scene
        .layers
        .retain(|layer| frames.get(layer.source).is_some());
    let target = pool.buffer().map_err(|e| e.to_string())?;
    renderer
        .draw(
            &drawn,
            Sources {
                screen: frames.screen.map(|f| &*f.buffer),
                camera: frames.camera.map(|f| &*f.buffer),
            },
            &target,
        )
        .map_err(|e| e.to_string())?;
    Ok(target)
}
