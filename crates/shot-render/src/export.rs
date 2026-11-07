//! Incremental export: render each chunk whose content hash has no cached file, in parallel on
//! the hardware encoder, then join every chunk by stream copy into one fragmented MP4.
//!
//! - **Chunks** (see [`crate::plan`]) are cached as `.shot/cache/chunks/<hash>.mp4`: video only,
//!   4K60 HEVC, one fragment that starts with a keyframe. A file that exists is reused. Stale
//!   files are never deleted yet. Consecutive missing chunks of a segment render in one encoder
//!   session, with a keyframe forced at each chunk start.
//! - **Audio** is cut from each take's mic track and encoded to AAC once per export, on its own
//!   thread while video renders. One continuous AAC stream has no encoder delay or padding at the
//!   joins, so they cannot click or drift. Where the source jumps, the cut fades over 5 ms. The
//!   voice chain, music bed and loudness of [`crate::voice`] finish it.
//! - **Captions** of the main cut go next to the output as a WebVTT sidecar, `<out>.vtt`.
//! - **Derived cuts** render in two passes, see [`crate::derived`]: the main chunks they draw
//!   from first, then their own chunks from those.
//! - **The join** runs alongside both. It writes one init segment, then copies each chunk's
//!   fragments with shifted decode times as soon as the chunk is cached, with the audio packets
//!   for the same span after each one as they come out of the encoder.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use shot_core::{ContentHash, Effects, Id, MediaId, Micros, OutputKind, Project, Timeline};
use shot_media::audio::{AAC_PACKET_SAMPLES, AacEncoder, AacStream, AudioError, SAMPLE_RATE};
use shot_media::encode::{EncodeError, EncodedFrame, EncoderSettings, HevcEncoder};
use shot_media::mux::{self, AudioTrack, Chapter, FragmentedFile, Joiner, Movie, MuxError, Track};
use shot_media::videotoolbox::{DecodeError, Decoder};

use crate::RenderError;
use crate::assets::{AssetError, Assets, EVENTS_FILE};
use crate::audio::{AudioSegment, TimelineAudio, TimelineAudioError};
use crate::cursor::FrameCursor;
use crate::derived::{DerivedInputs, DerivedPlan, FaceTrack};
use crate::frame::{Frame, Shot, compose};
use crate::graph::Renderer;
use crate::layout::{Canvas, LayoutSpec, Source, Sources, UnsupportedLayout};
use crate::plan::{ExportPlan, PlannedChunk, PlannedSegment, frame_time};
use crate::surface::size;
use crate::voice::{self, Bed};

/// Output frame rate.
pub const FPS: u32 = 60;
/// Video track timescale: 1000 ticks per frame.
const TIMESCALE: u32 = FPS * 1000;
const FRAME_TICKS: u32 = TIMESCALE / FPS;
/// A keyframe every 0.5 s, like the recordings.
pub(crate) const KEYFRAME_INTERVAL: u32 = FPS / 2;
/// VideoToolbox quality for the master. See docs/evidence/m03/export.md for the measurement.
pub(crate) const QUALITY: f32 = 0.75;
const AUDIO_BITRATE: u32 = 128_000;

/// The tracks of a take, by file name inside `media/<take>/`.
const SCREEN_FILE: &str = "screen.mp4";
const CAMERA_FILE: &str = "camera.mp4";
const MIC_FILE: &str = "mic.mp4";
/// The files chunks draw from: the video tracks and the event log. The mic is not cached.
const IDENTITY_FILES: [&str; 3] = [SCREEN_FILE, CAMERA_FILE, EVENTS_FILE];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    /// Encoder sessions running at the same time.
    pub jobs: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { jobs: 3 }
    }
}

/// Progress after each rendered chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub rendered: usize,
    pub to_render: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportReport {
    pub output: PathBuf,
    /// Chunks in the output, and how many of them were rendered rather than reused.
    pub chunks: usize,
    pub rendered: usize,
    pub frames: u64,
    pub rendered_frames: u64,
    /// For a derived cut, the main chunks it draws from, and how many of them were rendered
    /// rather than reused from the main cut's cache.
    pub base_chunks: usize,
    pub base_rendered: usize,
    /// Rendering chunks and encoding audio, which run at the same time.
    pub render_time: Duration,
    /// How much longer the join ran after that. It starts with them.
    pub join_time: Duration,
}

impl ExportReport {
    pub fn duration(&self) -> Duration {
        Duration::from_secs(self.frames) / FPS
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("there is no cut named {0}")]
    UnknownCut(String),
    #[error("{0} has nothing to export")]
    Empty(String),
    #[error(transparent)]
    Layout(#[from] UnsupportedLayout),
    #[error("{path}: {source}")]
    Decode { path: PathBuf, source: DecodeError },
    #[error("{path} ends before {time:?}")]
    MediaEnds { path: PathBuf, time: Duration },
    #[error(transparent)]
    Audio(#[from] TimelineAudioError),
    #[error("encoding audio: {0}")]
    AudioEncode(AudioError),
    #[error(transparent)]
    Encode(#[from] EncodeError),
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Assets(#[from] AssetError),
    #[error(transparent)]
    Template(#[from] crate::overlay::TemplateError),
    #[error(transparent)]
    Vision(#[from] shot_analyze::vision::VisionError),
    #[error("reading transcripts: {0}")]
    Transcript(shot_analyze::Error),
    #[error(transparent)]
    Mux(#[from] MuxError),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("the encoder did not start a chunk with a keyframe")]
    ChunkStart,
    #[error("another chunk failed")]
    Cancelled,
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> ExportError + '_ {
    move |source| ExportError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Exports the cut named `cut` of the project in `video_dir` to `out`.
pub fn export(
    video_dir: &Path,
    project: &Project,
    cut: &str,
    out: &Path,
    options: ExportOptions,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<ExportReport, ExportError> {
    export_decorated(video_dir, project, cut, out, options, None, progress)
}

/// Draws on top of every frame of a main cut after the cut's own drawing, for reels and
/// measurements of renderer features before a template uses them. Its name enters every chunk's
/// key, so decorated chunks never stand in for plain ones.
#[derive(Debug, Clone, Copy)]
pub struct Decoration {
    pub name: &'static str,
    /// Called with the frame's media time and canvas.
    pub draw: fn(Micros, Canvas, &mut Frame),
}

/// [`export`] with a [`Decoration`] on every frame of a main cut.
pub fn export_decorated(
    video_dir: &Path,
    project: &Project,
    cut: &str,
    out: &Path,
    options: ExportOptions,
    decoration: Option<Decoration>,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<ExportReport, ExportError> {
    let timeline = Timeline::evaluate(project);
    let output = timeline
        .outputs
        .iter()
        .find(|output| output.name == cut)
        .ok_or_else(|| ExportError::UnknownCut(cut.to_owned()))?;
    let media = Media {
        root: video_dir.join("media"),
        music: project.cut.music.as_ref().map(|m| {
            (
                video_dir.join(&m.file),
                f32::from(m.gain),
                f32::from(m.duck),
            )
        }),
    };
    let main = timeline.main();
    let ids: HashMap<&Id, MediaId> = main
        .chapters
        .iter()
        .flat_map(|chapter| &chapter.segments)
        .map(|segment| &segment.take)
        .collect::<HashSet<_>>()
        .into_iter()
        .map(|take| (take, media.id(take)))
        .collect();
    for segment in output.chapters.iter().flat_map(|c| &c.segments) {
        for overlay in &segment.overlays {
            crate::overlay::check(overlay)?;
        }
    }
    let assets = |file: &str| media.asset_id(video_dir, file);
    let derived = match output.kind {
        OutputKind::Main => None,
        OutputKind::Derived(_) => Some(plan_derived(
            video_dir,
            main,
            output,
            &|take| ids[take],
            &assets,
        )?),
    };
    let plan = match &derived {
        Some(derived) => derived.plan.clone(),
        None => {
            let mut plan = ExportPlan::new(output, FPS, |take| ids[take]);
            crate::overlay::key_assets(&mut plan, &assets);
            if let Some(decoration) = decoration {
                for chunk in plan.segments.iter_mut().flat_map(|s| &mut s.chunks) {
                    chunk.hash = chunk.hash.extend(decoration.name.as_bytes());
                }
            }
            plan
        }
    };
    if plan.frames == 0 {
        return Err(ExportError::Empty(cut.to_owned()));
    }
    for planned in derived.as_ref().map_or(&plan, |d| &d.main).segments.iter() {
        LayoutSpec::of(planned.segment.layout)?;
    }
    let cache = Cache {
        dir: video_dir.join(".shot/cache/chunks"),
    };
    fs::create_dir_all(&cache.dir).map_err(io_error(&cache.dir))?;

    let mut jobs = jobs(&plan, &cache, |_| true, derived.is_some());
    // Longest first, so the last job to finish is a short one.
    jobs.sort_by_key(|job| std::cmp::Reverse(job.frames()));
    let (mut base_chunks, mut base_rendered) = (0, 0);
    if let Some(derived) = &derived {
        let needed: HashSet<ContentHash> = jobs
            .iter()
            .flat_map(|job| &derived.chunks[job.segment][job.range.clone()])
            .map(|chunk| chunk.base.hash)
            .collect();
        let mut bases = self::jobs(&derived.main, &cache, |c| needed.contains(&c.hash), false);
        base_chunks = derived.bases().len();
        base_rendered = bases.iter().map(|job| job.chunks.len()).sum();
        // The queue pops from the end: every base job is taken before the first derived job,
        // so a derived job only ever waits for bases that are already rendering.
        bases.sort_by_key(|job| std::cmp::Reverse(job.frames()));
        jobs.extend(bases);
    }
    // Chunks of the output itself; base chunks are counted apart.
    let own = |job: &&Job| job.derived == derived.is_some();
    let rendered = jobs.iter().filter(own).map(|job| job.chunks.len()).sum();
    let rendered_frames = jobs.iter().filter(own).map(Job::frames).sum();

    let (render_time, join_time) = run(
        &plan,
        derived.as_ref(),
        jobs,
        &media,
        output.effects,
        &cache,
        out,
        options,
        decoration,
        progress,
    )?;
    if derived.is_none() {
        crate::captions::write_sidecar(video_dir, &plan, output, out)
            .map_err(io_error(&out.with_extension("vtt")))?;
    }
    Ok(ExportReport {
        output: out.to_owned(),
        chunks: plan.segments.iter().map(|s| s.chunks.len()).sum(),
        rendered,
        frames: plan.frames,
        rendered_frames,
        base_chunks,
        base_rendered,
        render_time,
        join_time,
    })
}

/// Plans a derived output: faces of the takes it reframes on and the transcripts of its
/// captions, read through their caches.
fn plan_derived(
    video_dir: &Path,
    main: &shot_core::Output,
    output: &shot_core::Output,
    media: &dyn Fn(&Id) -> MediaId,
    assets: &dyn Fn(&str) -> Vec<u8>,
) -> Result<DerivedPlan, ExportError> {
    let mut faces = HashMap::new();
    if output.kind == OutputKind::Derived(shot_core::DerivedKind::Short) {
        for (take, times) in crate::derived::face_times(output) {
            if !media_path(video_dir, &take, CAMERA_FILE).exists() {
                continue;
            }
            let track = shot_analyze::vision::face_track(video_dir, &take, &times)?;
            faces.insert(
                take,
                FaceTrack {
                    size: track.size,
                    samples: track.samples,
                },
            );
        }
    }
    let mut transcripts = HashMap::new();
    for take in output
        .chapters
        .iter()
        .flat_map(|c| &c.segments)
        .map(|s| &s.take)
    {
        if transcripts.contains_key(take) {
            continue;
        }
        if let Some(transcript) =
            shot_analyze::cache::read_cached(video_dir, take).map_err(ExportError::Transcript)?
        {
            transcripts.insert(take.clone(), transcript);
        }
    }
    Ok(DerivedPlan::new(
        main,
        output,
        &DerivedInputs {
            fps: FPS,
            media,
            assets,
            faces: &faces,
            transcripts: &transcripts,
        },
    ))
}

fn media_path(video_dir: &Path, take: &Id, file: &str) -> PathBuf {
    video_dir.join("media").join(take.as_str()).join(file)
}

/// Runs of consecutive chunks of one segment that are `wanted` and have no cached file, each
/// chunk once.
fn jobs<'a>(
    plan: &'a ExportPlan,
    cache: &Cache,
    wanted: impl Fn(&PlannedChunk) -> bool,
    derived: bool,
) -> Vec<Job<'a>> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for (segment, planned) in plan.segments.iter().enumerate() {
        let job = |run| Job::new(planned, segment, run, derived);
        let mut run: Option<Range<usize>> = None;
        for (index, chunk) in planned.chunks.iter().enumerate() {
            let missing =
                wanted(chunk) && seen.insert(chunk.hash) && !cache.path(chunk.hash).exists();
            match (&mut run, missing) {
                (Some(run), true) => run.end = index + 1,
                (None, true) => run = Some(index..index + 1),
                (Some(_), false) => jobs.extend(run.take().map(job)),
                (None, false) => {}
            }
        }
        jobs.extend(run.map(job));
    }
    jobs
}

/// Chunks that follow each other in one segment, rendered by one encoder session.
pub(crate) struct Job<'a> {
    pub planned: &'a PlannedSegment,
    pub chunks: &'a [PlannedChunk],
    /// The segment's index in its plan and the chunks' indices in the segment.
    pub segment: usize,
    pub range: Range<usize>,
    /// Whether the chunks are a derived cut's, drawn from main chunks.
    pub derived: bool,
}

impl<'a> Job<'a> {
    fn new(
        planned: &'a PlannedSegment,
        segment: usize,
        range: Range<usize>,
        derived: bool,
    ) -> Self {
        Self {
            planned,
            chunks: &planned.chunks[range.clone()],
            segment,
            range,
            derived,
        }
    }

    fn frames(&self) -> u64 {
        self.chunks.iter().map(PlannedChunk::frame_count).sum()
    }
}

struct Media {
    root: PathBuf,
    /// The music bed's file and its gain and duck in dB.
    music: Option<(PathBuf, f32, f32)>,
}

impl Media {
    /// The size and modification time of each video track and the event log of `take`. A take
    /// recorded again under the same id differs in both.
    fn id(&self, take: &Id) -> MediaId {
        let mut facts = Vec::new();
        for file in IDENTITY_FILES {
            facts.extend_from_slice(file.as_bytes());
            let Ok(metadata) = fs::metadata(self.track(take, file)) else {
                facts.push(0);
                continue;
            };
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .unwrap_or_default();
            facts.push(1);
            facts.extend_from_slice(&metadata.len().to_le_bytes());
            facts.extend_from_slice(&modified.as_nanos().to_le_bytes());
        }
        MediaId::of(&facts)
    }

    /// The size and modification time of a file an overlay names, so a file replaced under the
    /// same name renders again. A missing file is keyed as missing; drawing it fails later.
    fn asset_id(&self, video_dir: &Path, file: &str) -> Vec<u8> {
        let mut facts = file.as_bytes().to_vec();
        if let Ok(metadata) = fs::metadata(video_dir.join(file)) {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .unwrap_or_default();
            facts.extend_from_slice(&metadata.len().to_le_bytes());
            facts.extend_from_slice(&modified.as_nanos().to_le_bytes());
        }
        facts
    }

    fn track(&self, take: &Id, file: &str) -> PathBuf {
        self.root.join(take.as_str()).join(file)
    }

    fn video(&self, take: &Id, source: Source) -> PathBuf {
        self.track(
            take,
            match source {
                Source::Screen => SCREEN_FILE,
                Source::Camera => CAMERA_FILE,
            },
        )
    }
}

pub(crate) struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub(crate) fn path(&self, hash: ContentHash) -> PathBuf {
        self.dir.join(format!("{hash}.mp4"))
    }
}

/// Chunks that are still rendering, so the join can wait for them, and whether any thread failed.
pub(crate) struct Pending {
    hashes: Mutex<HashSet<ContentHash>>,
    changed: Condvar,
    failed: AtomicBool,
}

impl Pending {
    fn new(jobs: &[Job]) -> Self {
        Self {
            hashes: Mutex::new(
                jobs.iter()
                    .flat_map(|job| job.chunks)
                    .map(|chunk| chunk.hash)
                    .collect(),
            ),
            changed: Condvar::new(),
            failed: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<ContentHash>> {
        self.hashes.lock().expect("pending chunks poisoned")
    }

    fn done(&self, hash: ContentHash) {
        self.lock().remove(&hash);
        self.changed.notify_all();
    }

    fn fail(&self) {
        self.failed.store(true, Ordering::Relaxed);
        let _hashes = self.lock();
        self.changed.notify_all();
    }

    fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Waits until the chunk `hash` is in the cache.
    pub(crate) fn wait(&self, hash: ContentHash) -> Result<(), ExportError> {
        let mut hashes = self.lock();
        while hashes.contains(&hash) {
            if self.failed() {
                return Err(ExportError::Cancelled);
            }
            hashes = self.changed.wait(hashes).expect("pending chunks poisoned");
        }
        Ok(())
    }

    /// Runs `work`, and marks the export failed when it fails or panics, so no thread waits for
    /// it forever.
    fn guard<T>(&self, work: impl FnOnce() -> Result<T, ExportError>) -> Result<T, ExportError> {
        struct OnUnwind<'a>(&'a Pending);
        impl Drop for OnUnwind<'_> {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    self.0.fail();
                }
            }
        }
        let _unwind = OnUnwind(self);
        let result = work();
        if result.is_err() {
            self.fail();
        }
        result
    }
}

/// Renders `jobs` on `options.jobs` threads, encodes the audio on another and joins on a third.
/// Returns how long rendering and audio took, and how much longer the join ran.
#[allow(clippy::too_many_arguments)]
fn run(
    plan: &ExportPlan,
    derived: Option<&DerivedPlan>,
    jobs: Vec<Job>,
    media: &Media,
    effects: Effects,
    cache: &Cache,
    out: &Path,
    options: ExportOptions,
    decoration: Option<Decoration>,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<(Duration, Duration), ExportError> {
    let started = Instant::now();
    let to_render = jobs.iter().map(|job| job.chunks.len()).sum();
    let pending = Pending::new(&jobs);
    let queue = Mutex::new(jobs);
    let done = AtomicUsize::new(0);
    let (audio_out, audio_in) = mpsc::channel();
    std::thread::scope(|scope| {
        let pending = &pending;
        let audio = scope.spawn(move || pending.guard(|| encode_audio(plan, media, &audio_out)));
        let joiner =
            scope.spawn(move || pending.guard(|| join(plan, cache, pending, audio_in, out)));
        let workers: Vec<_> = (0..options.jobs.max(1))
            .map(|_| {
                scope.spawn(|| {
                    pending.guard(|| {
                        let mut worker = Worker::new(media, effects, decoration)?;
                        loop {
                            if pending.failed() {
                                return Err(ExportError::Cancelled);
                            }
                            let Some(job) = queue.lock().expect("job queue poisoned").pop() else {
                                return Ok(());
                            };
                            let chunk_done = |hash| {
                                pending.done(hash);
                                let rendered = done.fetch_add(1, Ordering::Relaxed) + 1;
                                progress(Progress {
                                    rendered,
                                    to_render,
                                });
                            };
                            match derived.filter(|_| job.derived) {
                                Some(derived) => crate::derived::render(
                                    &worker.renderer,
                                    derived,
                                    &job,
                                    cache,
                                    pending,
                                    &chunk_done,
                                )?,
                                None => worker.render(&job, media, cache, &chunk_done)?,
                            }
                        }
                    })
                })
            })
            .collect();
        let mut results: Vec<Result<(), ExportError>> = workers
            .into_iter()
            .map(|worker| worker.join().expect("render worker panicked"))
            .collect();
        results.push(audio.join().expect("audio thread panicked"));
        let render_time = started.elapsed();
        results.push(joiner.join().expect("join thread panicked"));
        let join_time = started.elapsed() - render_time;
        let mut errors = results.into_iter().filter_map(Result::err);
        match errors.find(|error| !matches!(error, ExportError::Cancelled)) {
            Some(error) => Err(error),
            None if pending.failed() => Err(ExportError::Cancelled),
            None => Ok((render_time, join_time)),
        }
    })
}

/// One render thread: its own GPU device, and decoders and assets kept open across segments.
struct Worker {
    renderer: Renderer,
    cursors: HashMap<PathBuf, FrameCursor>,
    assets: Assets,
    effects: Effects,
    decoration: Option<Decoration>,
}

impl Worker {
    fn new(
        media: &Media,
        effects: Effects,
        decoration: Option<Decoration>,
    ) -> Result<Self, ExportError> {
        let video_dir = media.root.parent().unwrap_or(&media.root);
        Ok(Self {
            renderer: Renderer::new()?,
            cursors: HashMap::new(),
            assets: Assets::new(video_dir),
            effects,
            decoration,
        })
    }

    /// Moves the decoder of `path` to `time`, opening it on first use. A clip shorter than the
    /// cutaway holds its last frame.
    fn goto(&mut self, path: &Path, time: Duration) -> Result<(), ExportError> {
        if !self.cursors.contains_key(path) {
            let decoder = Decoder::open_native(path).map_err(decode_error(path))?;
            self.cursors
                .insert(path.to_owned(), FrameCursor::new(decoder));
        }
        let cursor = self.cursors.get_mut(path).expect("opened above");
        if cursor.goto(time).map_err(decode_error(path))?.is_none() {
            return Err(ExportError::MediaEnds {
                path: path.to_owned(),
                time,
            });
        }
        Ok(())
    }

    /// Renders the chunks of `job` into the cache, calling `chunk_done` after each one.
    fn render(
        &mut self,
        job: &Job,
        media: &Media,
        cache: &Cache,
        chunk_done: &dyn Fn(ContentHash),
    ) -> Result<(), ExportError> {
        let planned = job.planned;
        let segment = &planned.segment;
        let spec = LayoutSpec::styled(segment.layout, &self.effects.window)?;
        let look = self
            .assets
            .look(&segment.take, &segment.overlays, self.effects)?;
        let first = job.chunks[0].frames.start;
        let paths = Sources {
            screen: spec
                .uses(Source::Screen)
                .then(|| media.video(&planned.segment.take, Source::Screen)),
            camera: spec
                .uses(Source::Camera)
                .then(|| media.video(&planned.segment.take, Source::Camera)),
        };
        for path in [&paths.screen, &paths.camera].into_iter().flatten() {
            if !self.cursors.contains_key(path) {
                let decoder = Decoder::open_native(path).map_err(decode_error(path))?;
                self.cursors.insert(path.clone(), FrameCursor::new(decoder));
            }
            let cursor = self.cursors.get_mut(path).expect("opened above");
            let time = to_duration(first);
            if cursor.goto(time).map_err(decode_error(path))?.is_none() {
                return Err(ExportError::MediaEnds {
                    path: path.clone(),
                    time,
                });
            }
        }

        let canvas = Canvas::UHD;
        let mut encoder = HevcEncoder::new(EncoderSettings {
            width: canvas.width,
            height: canvas.height,
            fps: FPS,
            keyframe_interval: KEYFRAME_INTERVAL,
            quality: QUALITY,
        })?;
        let mut files = ChunkFiles::new(job.chunks, cache, canvas, chunk_done);
        let starts: HashSet<u64> = job.chunks.iter().map(|chunk| chunk.frames.start).collect();
        for chunk in job.chunks {
            for frame in chunk.frames.clone() {
                let time = to_duration(frame);
                for path in [&paths.screen, &paths.camera].into_iter().flatten() {
                    self.cursors
                        .get_mut(path)
                        .expect("opened above")
                        .goto(time)
                        .map_err(decode_error(path))?;
                }
                let current = |cursors: &HashMap<PathBuf, FrameCursor>, path: &Option<PathBuf>| {
                    path.as_ref()
                        .and_then(|path| cursors.get(path))
                        .and_then(FrameCursor::current)
                        .map(|f| size(&f.buffer))
                };
                let sizes = Sources {
                    screen: current(&self.cursors, &paths.screen).unwrap_or((1, 1)),
                    camera: current(&self.cursors, &paths.camera).unwrap_or((1, 1)),
                };
                let shot = Shot {
                    layout: segment.layout,
                    zooms: &segment.zooms,
                    overlays: &segment.overlays,
                    time: frame_time(frame, FPS),
                };
                let mut drawn = compose(&shot, &look, canvas, sizes)?;
                if let Some(decoration) = self.decoration {
                    (decoration.draw)(shot.time, canvas, &mut drawn);
                }
                let mut screen = paths.screen.clone();
                if let Some(cutaway) = &drawn.cutaway {
                    let path = media
                        .root
                        .parent()
                        .unwrap_or(&media.root)
                        .join(&cutaway.file);
                    self.goto(&path, Duration::from_micros(cutaway.time.0))?;
                    let clip = current(&self.cursors, &Some(path.clone())).unwrap_or((16, 9));
                    crate::overlay::broll::fit_cutaway(&mut drawn, clip);
                    screen = Some(path);
                }
                let buffer = |path: &Option<PathBuf>| {
                    path.as_ref()
                        .and_then(|path| self.cursors.get(path))
                        .and_then(FrameCursor::current)
                        .map(|f| &*f.buffer)
                };
                let target = encoder.input_buffer()?;
                self.renderer.draw(
                    &drawn,
                    Sources {
                        screen: buffer(&screen),
                        camera: buffer(&paths.camera),
                    },
                    &target,
                )?;
                if starts.contains(&frame) {
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
}

/// Hands the encoded frames of a job to one chunk file after another.
pub(crate) struct ChunkFiles<'a> {
    chunks: std::slice::Iter<'a, PlannedChunk>,
    cache: &'a Cache,
    canvas: Canvas,
    current: Option<(ChunkWriter, ContentHash)>,
    chunk_done: &'a dyn Fn(ContentHash),
}

impl<'a> ChunkFiles<'a> {
    pub(crate) fn new(
        chunks: &'a [PlannedChunk],
        cache: &'a Cache,
        canvas: Canvas,
        chunk_done: &'a dyn Fn(ContentHash),
    ) -> Self {
        Self {
            chunks: chunks.iter(),
            cache,
            canvas,
            current: None,
            chunk_done,
        }
    }

    pub(crate) fn push(
        &mut self,
        frame: EncodedFrame,
        encoder: &HevcEncoder,
    ) -> Result<(), ExportError> {
        if self
            .current
            .as_ref()
            .is_none_or(|(current, _)| current.remaining == 0)
        {
            self.finish_current(encoder)?;
            let chunk = self
                .chunks
                .next()
                .expect("the encoder returns one frame per submitted frame");
            if !frame.keyframe {
                return Err(ExportError::ChunkStart);
            }
            self.current = Some((
                ChunkWriter::create(
                    self.cache.path(chunk.hash),
                    chunk.frame_count(),
                    self.canvas,
                )?,
                chunk.hash,
            ));
        }
        let (current, _) = self.current.as_mut().expect("opened above");
        current.push(frame, encoder)
    }

    pub(crate) fn finish(mut self, encoder: &HevcEncoder) -> Result<(), ExportError> {
        self.finish_current(encoder)
    }

    fn finish_current(&mut self, encoder: &HevcEncoder) -> Result<(), ExportError> {
        if let Some((current, hash)) = self.current.take() {
            current.finish(encoder)?;
            (self.chunk_done)(hash);
        }
        Ok(())
    }
}

pub(crate) fn to_duration(frame: u64) -> Duration {
    Duration::from_micros(frame_time(frame, FPS).0)
}

pub(crate) fn decode_error(path: &Path) -> impl FnOnce(DecodeError) -> ExportError + '_ {
    |source| ExportError::Decode {
        path: path.to_owned(),
        source,
    }
}

/// Writes one chunk file, one fragment per keyframe interval, under a temporary name until it is
/// complete. The init segment goes first, once the encoder has produced its parameter sets.
struct ChunkWriter {
    out: BufWriter<File>,
    partial: PathBuf,
    path: PathBuf,
    gop: Vec<EncodedFrame>,
    sequence: u32,
    time: u64,
    /// Frames still to come.
    remaining: u64,
    canvas: Canvas,
}

impl ChunkWriter {
    fn create(path: PathBuf, frames: u64, canvas: Canvas) -> Result<Self, ExportError> {
        let partial = path.with_extension("partial");
        let file = File::create(&partial).map_err(io_error(&partial))?;
        Ok(Self {
            out: BufWriter::new(file),
            partial,
            path,
            gop: Vec::new(),
            sequence: 0,
            time: 0,
            remaining: frames,
            canvas,
        })
    }

    fn push(&mut self, frame: EncodedFrame, encoder: &HevcEncoder) -> Result<(), ExportError> {
        if frame.keyframe && !self.gop.is_empty() {
            self.flush(encoder)?;
        }
        self.gop.push(frame);
        self.remaining -= 1;
        Ok(())
    }

    fn finish(mut self, encoder: &HevcEncoder) -> Result<(), ExportError> {
        self.flush(encoder)?;
        let file = self
            .out
            .into_inner()
            .map_err(|error| io_error(&self.partial)(error.into_error()))?;
        file.sync_all().map_err(io_error(&self.partial))?;
        fs::rename(&self.partial, &self.path).map_err(io_error(&self.path))
    }

    fn flush(&mut self, encoder: &HevcEncoder) -> Result<(), ExportError> {
        if self.gop.is_empty() {
            return Ok(());
        }
        if self.sequence == 0 {
            let movie = Movie {
                timescale: TIMESCALE,
                duration: 0,
                tracks: vec![video_track(encoder.hvcc()?, self.canvas)],
                chapters: Vec::new(),
            };
            mux::write_init(&mut self.out, &movie).map_err(io_error(&self.partial))?;
        }
        self.sequence += 1;
        let samples: Vec<mux::Sample> = self
            .gop
            .iter()
            .map(|frame| mux::Sample {
                size: frame.data.len() as u32,
                duration: FRAME_TICKS,
                sync: frame.keyframe,
            })
            .collect();
        let data: Vec<u8> = self
            .gop
            .iter()
            .flat_map(|f| f.data.iter().copied())
            .collect();
        mux::write_fragment(&mut self.out, self.sequence, 1, self.time, &samples, &data)
            .map_err(io_error(&self.partial))?;
        self.time += u64::from(FRAME_TICKS) * self.gop.len() as u64;
        self.gop.clear();
        Ok(())
    }
}

fn video_track(hvcc: Vec<u8>, canvas: Canvas) -> Track {
    Track::Video(mux::VideoTrack {
        width: canvas.width,
        height: canvas.height,
        timescale: TIMESCALE,
        hvcc,
    })
}

/// The mic audio of every segment, in output order, as one AAC stream sent to `out` piece by
/// piece. The first piece carries the format and no packets.
fn encode_audio(
    plan: &ExportPlan,
    media: &Media,
    out: &mpsc::Sender<AacStream>,
) -> Result<(), ExportError> {
    let send = |stream| out.send(stream).map_err(|_| ExportError::Cancelled);
    let mut encoder =
        AacEncoder::new(SAMPLE_RATE, AUDIO_BITRATE).map_err(ExportError::AudioEncode)?;
    send(encoder.take())?;
    let root = media.root.clone();
    let audio = TimelineAudio::new(
        plan.segments.iter().map(|planned| AudioSegment {
            take: planned.segment.take.clone(),
            source_frames: planned.source_frames.clone(),
        }),
        FPS,
        move |take| root.join(take.as_str()).join(MIC_FILE),
    );
    let mut bleeps: Vec<(Id, shot_core::TimeRange)> = plan
        .segments
        .iter()
        .flat_map(|planned| {
            crate::overlay::redact::bleeps(&planned.segment.overlays)
                .into_iter()
                .map(|range| (planned.segment.take.clone(), range))
        })
        .collect();
    bleeps.sort_by_key(|(take, range)| (take.to_string(), range.from()));
    bleeps.dedup();
    let mut audio = audio.with_bleeps(&bleeps);
    let bed = match &media.music {
        Some((path, gain, duck)) => {
            Some(
                Bed::read(path, *gain, *duck).map_err(|source| TimelineAudioError {
                    path: path.clone(),
                    source,
                })?,
            )
        }
        None => None,
    };
    voice::finish(&mut audio, bed.as_ref(), |samples| {
        encoder.push(samples).map_err(ExportError::AudioEncode)?;
        send(encoder.take())
    })?;
    send(encoder.finish().map_err(ExportError::AudioEncode)?)
}

/// Writes the output: one init segment, then every chunk's video fragments by stream copy,
/// each followed by the audio for the same span. Waits for chunks still rendering and for audio
/// still encoding.
fn join(
    plan: &ExportPlan,
    cache: &Cache,
    pending: &Pending,
    audio: mpsc::Receiver<AacStream>,
    out: &Path,
) -> Result<(), ExportError> {
    let read = |chunk: &PlannedChunk| -> Result<FragmentedFile, ExportError> {
        pending.wait(chunk.hash)?;
        let path = cache.path(chunk.hash);
        let file = FragmentedFile::read(&path)?;
        if file.duration() != chunk.frame_count() * u64::from(FRAME_TICKS) {
            return Err(MuxError::Format(
                path.display().to_string(),
                "chunk length does not match its range",
            )
            .into());
        }
        Ok(file)
    };
    let format = audio.recv().map_err(|_| ExportError::Cancelled)?;
    let video = read(&plan.segments[0].chunks[0])?.track;
    let movie = Movie {
        timescale: TIMESCALE,
        duration: plan.frames * u64::from(FRAME_TICKS),
        tracks: vec![
            video.clone(),
            Track::Audio(AudioTrack {
                sample_rate: SAMPLE_RATE,
                channels: 1,
                bitrate: format.bitrate,
                config: format.config.clone(),
                priming: format.priming,
            }),
        ],
        chapters: plan
            .chapters
            .iter()
            .map(|chapter| Chapter {
                start: chapter.output_frame * u64::from(FRAME_TICKS),
                title: chapter.title.clone(),
            })
            .collect(),
    };

    let partial = out.with_extension("partial");
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(io_error(dir))?;
    }
    let file = File::create(&partial).map_err(io_error(&partial))?;
    let mut joiner =
        Joiner::new(BufWriter::with_capacity(1 << 22, file), &movie).map_err(io_error(&partial))?;
    let mut packets = AudioPackets::new(format, audio, pending);
    for planned in &plan.segments {
        for chunk in &planned.chunks {
            let file = read(chunk)?;
            let path = cache.path(chunk.hash);
            if file.track != video {
                return Err(MuxError::CodecMismatch(path.display().to_string()).into());
            }
            let mut source = File::open(&path).map_err(io_error(&path))?;
            let start = planned.output_frame_of(chunk) * u64::from(FRAME_TICKS);
            for fragment in &file.fragments {
                let base = start + fragment.base_time;
                joiner.copy(&mut source, fragment, 1, base)?;
                packets.write_until(&mut joiner, Some(base + fragment.duration), &partial)?;
            }
        }
    }
    packets.write_until(&mut joiner, None, &partial)?;
    let file = joiner
        .finish()
        .map_err(io_error(&partial))?
        .into_inner()
        .map_err(|error| io_error(&partial)(error.into_error()))?;
    file.sync_all().map_err(io_error(&partial))?;
    fs::rename(&partial, out).map_err(io_error(out))
}

/// Hands out the AAC stream in fragments that line up with the video, receiving packets from the
/// encoder as they are needed.
struct AudioPackets<'a> {
    /// Every packet received so far.
    stream: AacStream,
    incoming: mpsc::Receiver<AacStream>,
    ended: bool,
    pending: &'a Pending,
    /// The first packet not written yet, and its byte offset.
    next: usize,
    offset: usize,
}

impl<'a> AudioPackets<'a> {
    fn new(format: AacStream, incoming: mpsc::Receiver<AacStream>, pending: &'a Pending) -> Self {
        Self {
            stream: format,
            incoming,
            ended: false,
            pending,
            next: 0,
            offset: 0,
        }
    }

    /// Writes the packets that start before `video_time` on the output timeline, or all the
    /// rest for `None`.
    fn write_until<W: io::Write>(
        &mut self,
        joiner: &mut Joiner<W>,
        video_time: Option<u64>,
        path: &Path,
    ) -> Result<(), ExportError> {
        let priming = self.stream.priming;
        // Presentation time of a packet: its media time minus the encoder delay.
        let starts_before = |packet: usize| match video_time {
            None => true,
            Some(time) => {
                let start = packet as i64 * i64::from(AAC_PACKET_SAMPLES) - i64::from(priming);
                (start as i128) * i128::from(TIMESCALE) < (time as i128) * i128::from(SAMPLE_RATE)
            }
        };
        let first = self.next;
        loop {
            while self.next < self.stream.sizes.len() && starts_before(self.next) {
                self.next += 1;
            }
            if self.next < self.stream.sizes.len() || self.ended {
                break;
            }
            match self.incoming.recv() {
                Ok(piece) => {
                    self.stream.data.extend_from_slice(&piece.data);
                    self.stream.sizes.extend_from_slice(&piece.sizes);
                }
                // The encoder hung up: at the end of the stream, or because it failed.
                Err(_) if self.pending.failed() => return Err(ExportError::Cancelled),
                Err(_) => self.ended = true,
            }
        }
        if self.next == first {
            return Ok(());
        }
        let sizes = &self.stream.sizes[first..self.next];
        let length: usize = sizes.iter().map(|&s| s as usize).sum();
        let samples: Vec<mux::Sample> = sizes
            .iter()
            .map(|&size| mux::Sample {
                size,
                duration: AAC_PACKET_SAMPLES,
                sync: true,
            })
            .collect();
        joiner
            .write(
                2,
                first as u64 * u64::from(AAC_PACKET_SAMPLES),
                &samples,
                &self.stream.data[self.offset..self.offset + length],
            )
            .map_err(io_error(path))?;
        self.offset += length;
        Ok(())
    }
}
