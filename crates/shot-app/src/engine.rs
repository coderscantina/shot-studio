//! The Edit screen's preview: a render thread that draws output frames of an [`EditMap`]'s plan
//! with `shot-render`'s [`Painter`], the code export renders every chunk with, so segments,
//! transitions, the intro and outro and the end card look as they will in the export. Frames go
//! into `420f` BT.601 buffers, the encoding gpui's surface element decodes.
//!
//! The thread keeps a few frames ahead while playing and renders one frame while paused. A seek
//! drops what is queued. Ahead of a dissolve or mask wipe it keeps more frames queued: the
//! window's first frame opens the incoming side's decoder at its handle, which can take a few
//! frames' time.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use shot_core::{Effects, Micros};
use shot_media::videotoolbox::Decoder;
use shot_render::assets::Assets;
use shot_render::drawing::Drawing;
use shot_render::frame::{Frame, Shot, compose};
use shot_render::graph::Renderer;
use shot_render::join::Piece;
use shot_render::layout::{Canvas, LayoutSpec, Rect, Scene, Source, Sources};
use shot_render::paint::Painter;
use shot_render::plan::{ExportPlan, frame_time};
use shot_render::surface::{BufferPool, Matrix, Range, size};
use shot_render::theme::Theme;

use crate::model::{EditMap, FPS};

/// Frames rendered ahead while playing, about 100 ms.
const AHEAD: usize = 6;
/// Frames rendered ahead while a dissolve or mask wipe starts within [`JOIN_NEAR`], 300 ms:
/// room for the incoming side's first frame.
const AHEAD_OF_JOIN: usize = 18;
/// How far ahead of a two-sided transition the deeper queue starts, a second.
const JOIN_NEAR: u64 = FPS as u64;
/// The preview's render size. The Picture workspace shows it at 1040 points, 2080 pixels.
const CANVAS: Canvas = Canvas {
    width: 1920,
    height: 1080,
};

/// A rendered output frame.
pub struct PreviewFrame {
    pub frame: u64,
    pub buffer: CFRetained<CVPixelBuffer>,
    /// Where the take's screen recording sits in the frame, when it shows.
    pub screen: Option<ScreenMap>,
}

/// The screen layer of a drawn frame: its place on the canvas and the part of the screen
/// recording it shows, so a point on the preview maps back to screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenMap {
    /// Canvas pixels of [`CANVAS`].
    frame: Rect,
    /// Texture coordinates of the recording.
    crop: Rect,
    /// The recording's size in pixels.
    size: (u32, u32),
}

impl ScreenMap {
    /// The screen pixel under `at`, a fraction of the preview's width and height, if the
    /// screen shows there.
    pub fn screen_point(&self, at: (f32, f32)) -> Option<shot_core::Point> {
        let (x, y) = (at.0 * CANVAS.width as f32, at.1 * CANVAS.height as f32);
        let f = self.frame;
        if x < f.left || x > f.right || y < f.top || y > f.bottom {
            return None;
        }
        let u = self.crop.left + (x - f.left) / f.width() * self.crop.width();
        let v = self.crop.top + (y - f.top) / f.height() * self.crop.height();
        Some(shot_core::Point {
            x: (u * self.size.0 as f32).round() as i32,
            y: (v * self.size.1 as f32).round() as i32,
        })
    }
}

// SAFETY: CoreVideo buffers are reference counted atomically and the render thread never touches
// a buffer once it is queued, so handing it to the UI thread is sound.
unsafe impl Send for PreviewFrame {}

/// Adds drawing over a painted frame, given its output time and the canvas. Only the bench
/// sets one; it draws in a pass of its own over what the painter drew.
pub type Decoration = fn(Micros, Canvas, &mut Frame);

/// What one preview plays: where each output frame comes from, the video folder, the format's
/// effects and the brand's theme.
pub struct Plan {
    pub map: EditMap,
    pub dir: PathBuf,
    pub effects: Effects,
    /// The [`Painter`] is made again whenever this changes, that is when `cut.kdl` or
    /// `cut.lock` changed, and kept across plans that only move the playhead or the view.
    pub theme: Result<Arc<Theme>, String>,
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

/// The frames to keep queued while playing from output frame `position` of `plan`.
fn lead(plan: &ExportPlan, position: u64) -> usize {
    let near = plan
        .transitions
        .iter()
        .any(|t| t.two_sided() && (position..=position + JOIN_NEAR).contains(&t.window.start));
    if near { AHEAD_OF_JOIN } else { AHEAD }
}

/// What the render thread keeps for one theme: the painter with its decoders, and the assets
/// the screen layer of a paused frame is laid out with.
struct Painting {
    theme: Arc<Theme>,
    painter: Painter,
    assets: Assets,
}

fn render_loop(shared: &Shared) {
    let mut renderer = None;
    let mut pool = None;
    let mut painting: Option<Painting> = None;
    // The size of each recording, by path, for the screen layer.
    let mut sizes: HashMap<PathBuf, (u32, u32)> = HashMap::new();
    loop {
        let (plan, frame, generation, decoration, playing) = {
            let mut state = shared.lock();
            loop {
                if state.quit {
                    return;
                }
                let wants = state.plan.as_ref().is_some_and(|p| {
                    state.position < p.map.frames
                        && state.error.is_none()
                        && (state.owed
                            || (state.playing
                                && state.queue.len() < lead(&p.map.plan, state.position)))
                });
                if wants {
                    break;
                }
                state = shared.changed.wait(state).expect("preview state poisoned");
            }
            state.owed = false;
            let plan = state.plan.clone().expect("checked above");
            (
                plan,
                state.position,
                state.generation,
                state.decoration,
                state.playing,
            )
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
            let theme = plan.theme.as_ref().map_err(Clone::clone)?;
            let painting = match &mut painting {
                Some(current) if Arc::ptr_eq(&current.theme, theme) => current,
                _ => painting.insert(Painting {
                    theme: theme.clone(),
                    painter: Painter::with_theme(&plan.dir, plan.effects, theme.clone()),
                    assets: Assets::with_theme(&plan.dir, theme.clone()),
                }),
            };
            let target = pool.buffer().map_err(|e| e.to_string())?;
            painting
                .painter
                .paint(renderer, &plan.map.plan, frame, &target)
                .map_err(|e| e.to_string())?;
            if let Some(decorate) = decoration {
                let mut over = Frame {
                    scene: Scene {
                        background: [0.0; 3],
                        layers: Vec::new(),
                    },
                    drawing: Drawing::default(),
                    cutaway: None,
                };
                decorate(frame_time(frame, FPS), CANVAS, &mut over);
                renderer
                    .draw_over(
                        &over,
                        Sources {
                            screen: None,
                            camera: None,
                        },
                        &target,
                    )
                    .map_err(|e| e.to_string())?;
            }
            // Only a paused frame can be clicked on to place a callout.
            let screen = match playing {
                true => None,
                false => screen_map(&plan, frame, &mut painting.assets, &mut sizes)?,
            };
            Ok::<_, String>((target, screen))
        })();

        let mut state = shared.lock();
        if state.generation != generation {
            continue;
        }
        match rendered {
            Ok((buffer, screen)) => {
                state.queue.push_back(PreviewFrame {
                    frame,
                    buffer,
                    screen,
                });
                state.position = frame + 1;
            }
            Err(error) => state.error = Some(error),
        }
        shared.changed.notify_all();
    }
}

/// Where the take's screen recording sits in output frame `frame`, laid out as the painter lays
/// it out. `None` in a bookend, a transition, a b-roll cutaway or a layout without the screen.
fn screen_map(
    plan: &Plan,
    frame: u64,
    assets: &mut Assets,
    sizes: &mut HashMap<PathBuf, (u32, u32)>,
) -> Result<Option<ScreenMap>, String> {
    let export = &plan.map.plan;
    if export.join_at(frame).is_some() || !matches!(export.piece_at(frame), Some(Piece::Segment(_)))
    {
        return Ok(None);
    }
    let Some(span) = plan.map.span_at(frame) else {
        return Ok(None);
    };
    let spec = LayoutSpec::styled(span.layout, &plan.effects.window).map_err(|e| e.to_string())?;
    let take = plan.dir.join("media").join(span.take.as_str());
    let mut size_of = |source, file| -> Result<Option<(u32, u32)>, String> {
        let path = take.join(file);
        if !spec.uses(source) || !path.exists() {
            return Ok(None);
        }
        if let Some(size) = sizes.get(&path) {
            return Ok(Some(*size));
        }
        let error = |e: shot_media::videotoolbox::DecodeError| format!("{}: {e}", path.display());
        let first = Decoder::open_native(&path)
            .map_err(error)?
            .next_frame()
            .map_err(error)?;
        let size = first.map_or((16, 9), |f| size(&f.buffer));
        sizes.insert(path, size);
        Ok(Some(size))
    };
    let Some(screen) = size_of(Source::Screen, "screen.mp4")? else {
        return Ok(None);
    };
    let camera = size_of(Source::Camera, "camera.mp4")?.unwrap_or((16, 9));
    let look = assets
        .look(&span.take, &span.overlays, plan.effects)
        .map_err(|e| e.to_string())?;
    let shot = Shot {
        layout: span.layout,
        zooms: &span.zooms,
        overlays: &span.overlays,
        time: frame_time(span.source_frame(frame), FPS),
    };
    let drawn =
        compose(&shot, &look, CANVAS, Sources { screen, camera }).map_err(|e| e.to_string())?;
    if drawn.cutaway.is_some() {
        return Ok(None);
    }
    Ok(drawn
        .scene
        .layers
        .iter()
        .find(|l| l.source == Source::Screen)
        .map(|l| ScreenMap {
            frame: l.frame,
            crop: l.crop,
            size: screen,
        }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use shot_core::TransitionKind;
    use shot_render::cursor::FrameCursor;
    use shot_render::export::{ExportOptions, export};
    use shot_render::surface::read_rgb;

    use super::*;
    use crate::folder::VideoFolder;
    use crate::model::{EditMap, View};

    const CUT: &str = "chapter \"Hook\" {\n    take t01 {\n        keep from=10_000_000 to=14_000_000\n    }\n}\n\nchapter \"Setup\" {\n    take t01 {\n        keep from=120_000_000 to=124_000_000\n        layout split from=120_000_000 to=124_000_000\n    }\n}\n\nchapter \"Structure\" {\n    take t01 {\n        keep from=300_000_000 to=304_000_000\n    }\n}\n";

    /// A channel with the target project's take, an intro after the hook (the 30 s screen clip
    /// with a title), an outro (the camera clip) under a 3 s end card, and `video` joining its
    /// chapters with `kind`.
    fn channel(root: &Path, fixtures: &Path, kind: &str) -> PathBuf {
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "brand/brand.kdl",
            "intro bumper=\"brand/media/intro.mp4\" title=#true\noutro bumper=\"brand/media/outro.mp4\"\nend-card length=3_000_000\ncamera shape=circle corners=0 border=8 shadow=soft\non video-start {\n    transition dip\n}\n",
        );
        std::fs::create_dir_all(root.join("brand/media")).unwrap();
        for (clip, name) in [("screen-30s.mp4", "intro"), ("camera-30s.mp4", "outro")] {
            std::os::unix::fs::symlink(
                fixtures.join(clip),
                root.join(format!("brand/media/{name}.mp4")),
            )
            .unwrap();
        }
        let dir = root.join("videos/v");
        write(
            "videos/v/video.kdl",
            "title \"Painted\"\nstatus editing\nformat opinion\n",
        );
        write("videos/v/script.md", "# Painted\n");
        write(
            "videos/v/format.kdl",
            &format!("on chapter-start {{\n    transition {kind} length=800_000\n}}\n"),
        );
        write("videos/v/cut.kdl", CUT);
        write("videos/v/cut.lock", "lock 1\n");
        std::os::unix::fs::symlink(fixtures.join("target-project/media"), dir.join("media"))
            .unwrap();
        dir
    }

    /// The mean difference per channel of a preview frame and the export's frame, which is
    /// twice its size and averaged down to it.
    fn difference(preview: &[[u8; 3]], export: &[[u8; 3]]) -> f64 {
        let (w, h) = (CANVAS.width as usize, CANVAS.height as usize);
        let mut sum = 0u64;
        for y in 0..h {
            for x in 0..w {
                let at = |dx, dy| export[(2 * y + dy) * 2 * w + 2 * x + dx];
                for (c, &value) in preview[y * w + x].iter().enumerate() {
                    let down = [(0, 0), (1, 0), (0, 1), (1, 1)]
                        .iter()
                        .map(|&(dx, dy)| u32::from(at(dx, dy)[c]))
                        .sum::<u32>()
                        .div_ceil(4);
                    sum += u64::from(down.abs_diff(u32::from(value)));
                }
            }
        }
        sum as f64 / (w * h * 3) as f64
    }

    // Needs the generated fixtures and exports two short cuts at 4K, about a minute each.
    #[test]
    #[ignore]
    fn the_preview_draws_transitions_and_bookends_as_the_export_does() {
        let fixtures = PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES"));
        for kind in ["dissolve", "mask-wipe"] {
            let root = tempfile::tempdir().unwrap();
            let dir = channel(root.path(), &fixtures, kind);
            let folder = VideoFolder::open(&dir).unwrap();
            let map = EditMap::preview(&dir, &folder.project, &folder.shown, View::Edited).unwrap();
            let plan = map.plan.clone();
            let out = dir.join("main.mp4");
            export(
                &dir,
                &folder.project,
                "main",
                &out,
                ExportOptions::default(),
                &|_| {},
            )
            .unwrap();

            // Inside each transition at its start, a quarter, the frames around the cut and
            // three quarters; the intro's title and the end card over the outro.
            let mut samples = Vec::new();
            for t in &plan.transitions {
                let quarter = (t.window.end - t.window.start) / 4;
                samples.extend([
                    t.window.start,
                    t.window.start + quarter,
                    t.edge - 1,
                    t.edge,
                    t.window.end - quarter,
                ]);
            }
            let intro = &plan.bookends[0];
            samples.push(intro.output_frame + intro.frames / 3);
            let card = plan.end_card.clone().expect("an end card");
            samples.extend([card.start + 30, card.end - 1]);
            assert!(
                plan.transitions
                    .iter()
                    .any(|t| t.kind.as_str() == kind && t.two_sided()),
                "{kind}: {:?}",
                plan.transitions
            );
            assert!(
                plan.transitions
                    .iter()
                    .any(|t| t.kind == TransitionKind::Dip)
            );

            let engine = Engine::spawn();
            engine.set_plan(
                Plan {
                    map,
                    dir: dir.clone(),
                    effects: folder.project.format.effects(),
                    theme: folder.theme.clone(),
                },
                0,
            );
            let mut decoded = FrameCursor::new(Decoder::open_native(&out).unwrap());
            for frame in samples {
                engine.seek(frame);
                let waited = Instant::now();
                let shown = loop {
                    assert_eq!(engine.error(), None);
                    if let Some(shown) = engine.take_next() {
                        break shown;
                    }
                    assert!(waited.elapsed() < Duration::from_secs(30), "frame {frame}");
                    std::thread::sleep(Duration::from_millis(1));
                };
                assert_eq!(shown.frame, frame);
                let time = Duration::from_micros(frame_time(frame, FPS).0);
                let exported = decoded.goto(time).unwrap().unwrap();
                let off = difference(
                    &read_rgb(&shown.buffer).unwrap(),
                    &read_rgb(&exported.buffer).unwrap(),
                );
                println!("{kind} frame {frame}: {off:.2}");
                // The export's encoding leaves up to about 3 levels; inside a window the frame 8
                // later differs by 6 to 41.
                assert!(off < 4.0, "{kind}: frame {frame} differs by {off:.2}");
            }
        }
    }
}
