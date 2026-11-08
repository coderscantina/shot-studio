//! The live preview of the theme and overlay editors: a real take of the channel, drawn by the
//! Edit screen's [`Engine`] with `shot-render`'s painter in a theme built from the brand being
//! edited, so it shows what export draws.
//!
//! A [`Sample`] is a moment of a take on disk. [`Sample::project`] makes a one-chapter cut that
//! keeps a few seconds of it and carries the overlays to show; the stage plans and paints it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use gpui::{Context, ObjectFit, Window, div, prelude::*, px, surface};
use shot_core::{
    Brand, Cut, FormatKind, FormatLayer, Id, Layout, LayoutChange, Micros, Overlay, Project,
    RangeEdit, Script, TimeRange, Video,
};
use shot_render::drawing::{Rgba, Shape};
use shot_render::frame::Frame;
use shot_render::layout::Canvas;
use shot_render::plan::frame_time;
use shot_render::theme::{Theme, ThemeError};

use crate::engine::{Engine, Plan, PreviewFrame};
use crate::model::{EditMap, FPS, View};
use crate::theme::*;

/// A moment of a take to preview on.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// The video folder the take is in.
    pub dir: PathBuf,
    pub video: Video,
    pub take: Id,
    /// Take time the preview starts at.
    pub at: Micros,
    /// Whether the take has a camera track, so the camera frame shows beside the screen.
    pub camera: bool,
    /// The layout to show, `None` for the camera beside the screen when there is a camera.
    pub layout: Option<Layout>,
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

impl Sample {
    /// The first video of the channel at `repo`, by folder name, with a recorded take: two
    /// seconds into its longest kept range.
    pub fn find(repo: &Path) -> Option<Self> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(repo.join("videos"))
            .ok()?
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs.into_iter().find_map(|dir| {
            let video = Video::parse(&read(&dir.join("video.kdl"))?).ok()?;
            let cut = Cut::parse(&read(&dir.join("cut.kdl"))?).ok()?;
            let (take, range) = cut
                .chapters
                .iter()
                .flat_map(|c| &c.takes)
                .flat_map(|t| {
                    t.ranges.iter().filter_map(move |r| match r {
                        RangeEdit::Keep(range) => Some((t.take.clone(), *range)),
                        _ => None,
                    })
                })
                .filter(|(take, _)| {
                    dir.join("media")
                        .join(take.as_str())
                        .join("screen.mp4")
                        .exists()
                })
                .max_by_key(|(_, range)| range.duration())?;
            Some(Self::at(
                dir,
                video,
                take,
                Micros(range.from().0 + 2_000_000),
            ))
        })
    }

    /// The take `take` of the video in `dir` at take time `at`.
    pub fn at(dir: PathBuf, video: Video, take: Id, at: Micros) -> Self {
        let camera = dir
            .join("media")
            .join(take.as_str())
            .join("camera.mp4")
            .exists();
        Self {
            dir,
            video,
            take,
            at,
            camera,
            layout: None,
        }
    }

    /// The video folder `dir` at take time `at` of `take`, reading its `video.kdl`.
    pub fn of_video(dir: &Path, take: Id, at: Micros) -> Option<Self> {
        let video = Video::parse(&read(&dir.join("video.kdl"))?).ok()?;
        Some(Self::at(dir.to_owned(), video, take, at))
    }

    pub fn format(&self) -> FormatKind {
        self.video.format
    }

    /// A one-chapter cut that keeps `length` of the take from the sample's moment, the camera
    /// beside the screen, with `overlays` whose ranges count from the moment.
    pub fn project(&self, title: &str, length: Micros, overlays: Vec<Overlay>) -> Project {
        let range = |from: Micros, to: Micros| {
            TimeRange::new(Micros(self.at.0 + from.0), Micros(self.at.0 + to.0))
                .expect("a scene has a length")
        };
        let mut cut =
            Cut::default().append_take(self.take.clone(), title, range(Micros(0), length));
        let block = &mut cut.chapters[0].takes[0];
        block.layouts.push(LayoutChange {
            layout: self.layout.unwrap_or(if self.camera {
                Layout::FaceContent
            } else {
                Layout::Screen
            }),
            range: range(Micros(0), length),
        });
        block.overlays = overlays
            .into_iter()
            .map(|o| Overlay {
                range: range(o.range.from(), o.range.to()),
                ..o
            })
            .collect();
        Project {
            video: self.video.clone(),
            script: Script {
                title: self.video.title.clone(),
                chapters: Vec::new(),
            },
            format: self.video.format.preset(),
            cut,
        }
    }
}

/// The brand a video of the channel at `repo` resolves from `brand` as `brand.kdl`: its
/// imports from `styles/`, then `brand`, then the channel format layer's `brand` block for
/// `format`, as the build lays them.
pub fn resolved(repo: &Path, brand: &Brand, format: FormatKind) -> Result<Brand, String> {
    let mut merged = Brand::default();
    for import in &brand.imports {
        let path = format!("styles/{}.kdl", import.style);
        let text = read(&repo.join(&path)).ok_or_else(|| format!("{path} is missing"))?;
        merged = merged.merge(&Brand::parse(&text).map_err(|e| format!("{path}:{e}"))?);
    }
    merged = merged.merge(brand);
    let path = format!("formats/{format}.kdl");
    if let Some(text) = read(&repo.join(&path)) {
        let layer = FormatLayer::parse(&text).map_err(|e| format!("{path}:{e}"))?;
        if let Some(patch) = &layer.brand {
            merged = merged.merge(patch);
        }
    }
    Ok(merged)
}

/// The theme `brand` gives a video of `format` with `cut`, reading its files from `repo`.
pub fn theme(
    repo: &Path,
    brand: &Brand,
    format: FormatKind,
    cut: &Cut,
) -> Result<Arc<Theme>, String> {
    let root = repo.to_owned();
    Theme::new(brand, format, Some(cut), &|file| {
        let path = root.join(file);
        std::fs::read(&path).map_err(|source| ThemeError::Io { path, source })
    })
    .map(Arc::new)
    .map_err(|e| e.to_string())
}

/// What the stage plays: a project of a sample in a theme, and the decoration it draws on top.
pub struct Scene {
    pub dir: PathBuf,
    pub project: Project,
    pub theme: Result<Arc<Theme>, String>,
    pub captions: bool,
}

/// The words the sample captions show, one lit at a time.
const CAPTION_WORDS: [&str; 6] = ["This", "is", "how", "your", "captions", "read"];
/// How long each caption word stays lit.
const CAPTION_STEP: u64 = 380_000;

/// Burned-in captions as `shot-render` draws them on Shorts today, over [`CAPTIONS`] of the
/// scene, where no card covers them: one cue whose words light up in turn.
fn captions(t: Micros, canvas: Canvas, frame: &mut Frame) {
    if !CAPTIONS.contains(&t.0) {
        return;
    }
    let since_start = t.0 - CAPTIONS.start;
    let word = (since_start / CAPTION_STEP) as usize % CAPTION_WORDS.len();
    let since = Micros(since_start % CAPTION_STEP);
    frame.drawing.over.push(Shape::Caption {
        center: [canvas.width as f32 / 2.0, canvas.height as f32 * 0.83],
        width: canvas.width as f32 * 0.84,
        words: CAPTION_WORDS.iter().map(|w| (*w).to_owned()).collect(),
        highlight: Some(word),
        size: canvas.width as f32 * 0.05,
        color: Rgba::hex(0xFFFFFF, 1.0),
        accent: Rgba::hex(0xC8F25A, 1.0),
        background: Rgba::hex(0x0A0B0D, 0.78),
        pop: shot_render::kinetic::word_pop(since),
    });
}

/// The stretch of the theme scene the sample captions draw on, in microseconds.
pub const CAPTIONS: std::ops::Range<u64> = 13_500_000..16_500_000;

/// A preview that shows one frame of a [`Scene`] and plays stretches of it.
pub struct Stage {
    engine: Engine,
    map: EditMap,
    shown: Option<PreviewFrame>,
    /// Playing from the clock's frame until `until`.
    playing: Option<Playing>,
    /// A frame is owed after a seek.
    seeking: bool,
    /// The frame to show when nothing plays.
    rest: u64,
    /// Why nothing can show, such as a channel without a recorded take.
    pub note: Option<String>,
    captions: bool,
}

#[derive(Clone, Copy)]
struct Playing {
    started: Instant,
    from: u64,
    until: u64,
}

/// The output frame of time `t` of a scene.
pub fn frame_of(t: Micros) -> u64 {
    t.0 * u64::from(FPS) / 1_000_000
}

impl Stage {
    pub fn new() -> Self {
        Self {
            engine: Engine::spawn(),
            map: EditMap::default(),
            shown: None,
            playing: None,
            seeking: false,
            rest: 0,
            note: None,
            captions: false,
        }
    }

    /// Shows `scene` at time `at`, or `note` when there is nothing to show.
    pub fn set_scene(&mut self, scene: Result<Scene, String>, at: Micros, cx: &mut Context<Self>) {
        let scene = match scene {
            Ok(scene) => scene,
            Err(note) => {
                self.note = Some(note);
                self.shown = None;
                cx.notify();
                return;
            }
        };
        if let Err(error) = &scene.theme {
            self.note = Some(format!("The brand cannot be drawn: {error}"));
        } else {
            self.note = None;
        }
        if scene.captions != self.captions && scene.captions {
            self.engine.set_decoration(captions);
        }
        self.captions = scene.captions;
        self.map = EditMap::of(&scene.project, View::Edited);
        let frame = frame_of(at).min(self.map.frames.saturating_sub(1));
        self.rest = frame;
        self.engine.set_plan(
            Plan {
                map: self.map.clone(),
                dir: scene.dir,
                effects: scene.project.format.effects(),
                theme: scene.theme,
            },
            frame,
        );
        self.playing = None;
        self.engine.set_playing(false);
        self.seeking = true;
        cx.notify();
    }

    /// Shows the frame at `at` and rests there.
    pub fn seek(&mut self, at: Micros, cx: &mut Context<Self>) {
        self.rest = frame_of(at).min(self.map.frames.saturating_sub(1));
        self.playing = None;
        self.engine.set_playing(false);
        self.engine.seek(self.rest);
        self.seeking = true;
        cx.notify();
    }

    /// Plays from `from` to `until`, then shows the frame at `rest`.
    pub fn play(&mut self, from: Micros, until: Micros, rest: Micros, cx: &mut Context<Self>) {
        let last = self.map.frames.saturating_sub(1);
        self.rest = frame_of(rest).min(last);
        let from = frame_of(from).min(last);
        self.engine.seek(from);
        self.engine.set_playing(true);
        self.playing = Some(Playing {
            started: Instant::now(),
            from,
            until: frame_of(until).min(last),
        });
        cx.notify();
    }

    fn advance(&mut self) {
        let Some(playing) = self.playing else {
            if self.seeking
                && let Some(frame) = self.engine.take_next()
            {
                if frame.frame == self.rest {
                    self.seeking = false;
                }
                self.shown = Some(frame);
            }
            return;
        };
        let due = playing.from + (playing.started.elapsed().as_secs_f64() * f64::from(FPS)) as u64;
        if let Some(frame) = self.engine.take_due(due) {
            self.shown = Some(frame);
        }
        if due >= playing.until {
            self.playing = None;
            self.engine.set_playing(false);
            self.engine.seek(self.rest);
            self.seeking = true;
        }
    }

    /// The output time of the frame on show.
    pub fn time(&self) -> Option<Micros> {
        self.shown.as_ref().map(|f| frame_time(f.frame, FPS))
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = self.engine.error()
            && self.note.is_none()
        {
            self.note = Some(error);
        }
        self.advance();
        if self.playing.is_some() || self.seeking {
            window.request_animation_frame();
        }
        div()
            .relative()
            .w_full()
            .aspect_ratio(16. / 9.)
            .flex_none()
            .rounded(px(6.))
            .overflow_hidden()
            .bg(gpui::black())
            .shadow_lg()
            .children(
                self.shown
                    .as_ref()
                    .filter(|_| self.note.is_none())
                    .map(|frame| {
                        surface(frame.buffer.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                    }),
            )
            .children(self.note.clone().map(|note| {
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(32.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(note)
            }))
    }
}

/// Sample captions use the Shorts' fixed colors until the brand styles captions.
pub fn captions_note() -> &'static str {
    "Captions burn in on Shorts and X clips, white with the lit word in lime. Their style \
     joins the brand in a later change"
}
