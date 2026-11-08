//! Incremental export: render each chunk whose content hash has no cached file, in parallel on
//! the hardware encoder, then join every chunk by stream copy into one fragmented MP4.
//!
//! - **Chunks** (see [`crate::plan`]) are cached as `.shot/cache/chunks/<hash>.mp4`: video only,
//!   4K60 HEVC, one fragment that starts with a keyframe. A file that exists is reused. Stale
//!   files are never deleted yet. Bookend chunks go to the channel repo's `.shot/cache/chunks/`,
//!   shared by its videos. Consecutive missing chunks of a segment render in one encoder
//!   session, with a keyframe forced at each chunk start.
//! - **Proposals** ([0004]) render into their own space, `.shot/cache/proposals/<name>/chunks/`,
//!   with the lock built from the proposal's files. A chunk the video's cache (or the channel's)
//!   already holds is read from there and never written, so a proposal leaves the branch's
//!   `cut.lock` and cache as they were. See [`render_proposal`].
//! - **Audio** is cut from each take's mic track and encoded to AAC once per export, on its own
//!   thread while video renders. One continuous AAC stream has no encoder delay or padding at the
//!   joins, so they cannot click or drift. Where the source jumps, the cut fades over 5 ms. The
//!   voice chain, music bed and loudness of [`crate::voice`] finish it, and the encoded stream
//!   is measured and finished again lower, from the same mix, when it peaks over -1 dBTP.
//! - **Captions** of the main cut go next to the output as a WebVTT sidecar, `<out>.vtt`.
//! - **Derived cuts** render in two passes, see [`crate::derived`]: the main chunks they draw
//!   from first, then their own chunks from those.
//! - **The join** runs alongside both. It writes one init segment, then copies each chunk's
//!   fragments with shifted decode times as soon as the chunk is cached, each followed by room
//!   for the audio of the same span. The audio fills its room once it is encoded, so the video
//!   is copied while the audio finishes.
//!
//! [0004]: ../../../docs/decisions/0004-proposals-as-branches.md

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use shot_core::{Brand, ContentHash, Effects, Id, Lock, MediaId, Micros, OutputKind, Project};
use shot_media::audio::{AAC_PACKET_SAMPLES, AacEncoder, AacStream, AudioError, SAMPLE_RATE};
use shot_media::encode::{EncodeError, EncodedFrame, EncoderSettings, HevcEncoder};
use shot_media::mux::{self, AudioTrack, Chapter, FragmentedFile, Joiner, Movie, MuxError, Track};
use shot_media::videotoolbox::{DecodeError, Decoder};

use crate::RenderError;
use crate::assets::{AssetError, EVENTS_FILE};
use crate::audio::{TimelineAudio, TimelineAudioError};
use crate::derived::{DerivedInputs, DerivedPlan, FaceTrack};
use crate::graph::Renderer;
use crate::join::{BookendInput, BookendKind, Joins, KeyInputs, Piece, StingerTiming};
use crate::layout::{Canvas, LayoutSpec, UnsupportedLayout};
use crate::paint::{PaintError, Painter, is_lottie};
use crate::plan::{ExportPlan, PlannedChunk, frame_time};
use crate::theme::Theme;
use crate::voice;

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

/// Rendered chunks, inside a video folder or the channel repo.
const CHUNKS_DIR: &str = ".shot/cache/chunks";
/// Proposal spaces, inside a video folder, one folder per proposal name.
const PROPOSALS_DIR: &str = ".shot/cache/proposals";
const BEATS_DIR: &str = ".shot/cache/beats";
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
    #[error(transparent)]
    Lock(#[from] crate::lock::LockError),
    #[error(transparent)]
    Theme(#[from] crate::theme::ThemeError),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("the encoder did not start a chunk with a keyframe")]
    ChunkStart,
    #[error("no sound `{0}` in the video's sounds/, brand/sounds/ or the bundled set")]
    UnknownSound(Id),
    #[error(transparent)]
    Paint(#[from] PaintError),
    #[error("the brand's bumper {0} is missing")]
    MissingBumper(PathBuf),
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
    let lock = crate::lock::read(video_dir)?;
    let cache = |plan: &ExportPlan| Cache::new(video_dir, plan);
    render(
        video_dir,
        project,
        lock.as_ref(),
        &cache,
        cut,
        Some(out),
        options,
        progress,
    )
}

/// The folder of proposal `name` of the video in `video_dir`: its `cut.lock` and `chunks/`.
pub fn proposal_dir(video_dir: &Path, name: &str) -> PathBuf {
    video_dir.join(PROPOSALS_DIR).join(name)
}

/// Renders the chunks of cut `cut` of proposal `name` that no cache holds yet, into
/// [`proposal_dir`]'s `chunks/`, with `project` and `lock` read and built from the proposal's
/// files. Chunks the video's or the channel's cache hold are reused and never written, so the
/// branch's cache is left as it was. Nothing is joined: the report's `output` is the
/// proposal's chunk folder.
pub fn render_proposal(
    video_dir: &Path,
    project: &Project,
    lock: &Lock,
    name: &str,
    cut: &str,
    options: ExportOptions,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<ExportReport, ExportError> {
    let dir = proposal_dir(video_dir, name).join("chunks");
    let cache = |_: &ExportPlan| Cache::proposal(video_dir, &dir);
    render(
        video_dir,
        project,
        Some(lock),
        &cache,
        cut,
        None,
        options,
        progress,
    )
}

/// Renders `cut` styled by `lock` into the cache `cache` makes for its plan, and joins it to
/// `out` when there is one.
#[allow(clippy::too_many_arguments)]
fn render(
    video_dir: &Path,
    project: &Project,
    lock: Option<&Lock>,
    cache: &dyn Fn(&ExportPlan) -> Result<Cache, ExportError>,
    cut: &str,
    out: Option<&Path>,
    options: ExportOptions,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<ExportReport, ExportError> {
    let timeline = match lock {
        Some(lock) => shot_core::Timeline::styled(project, lock),
        None => shot_core::Timeline::evaluate(project),
    };
    let output = timeline
        .outputs
        .iter()
        .find(|output| output.name == cut)
        .ok_or_else(|| ExportError::UnknownCut(cut.to_owned()))?;
    let theme = Arc::new(theme(video_dir, lock, project)?);
    let media = Media::new(video_dir, project, lock);
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
            theme.check(overlay, &segment.take)?;
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
            &theme,
        )?),
    };
    let (plan, music) = match &derived {
        Some(derived) => {
            let music = music(&derived.plan, Some(&derived.main), &media)?;
            (derived.plan.clone(), music)
        }
        None => {
            let joins = match lock {
                Some(lock) => Some(joins(&media, project, lock, &theme)?),
                None => None,
            };
            let plan_of = |output: &shot_core::Output| {
                let mut plan = ExportPlan::new(output, FPS, |take| ids[take]);
                crate::overlay::key_chunks(&mut plan, &assets, &theme);
                if let Some(joins) = &joins {
                    plan = plan.with_joins(joins);
                    crate::join::key_joins(
                        &mut plan,
                        &KeyInputs {
                            output,
                            media: &|take| ids[take],
                            identity: &assets,
                            theme: &theme,
                        },
                    );
                }
                plan
            };
            let plan = plan_of(output);
            let music = music(&plan, None, &media)?;
            let snapped = theme
                .beat_sync
                .then(|| snap_entrances(output, &plan, &music))
                .flatten();
            match snapped {
                Some(snapped) => (plan_of(&snapped), music),
                None => (plan, music),
            }
        }
    };
    if plan.frames == 0 {
        return Err(ExportError::Empty(cut.to_owned()));
    }
    for planned in derived.as_ref().map_or(&plan, |d| &d.main).segments.iter() {
        LayoutSpec::of(planned.segment.layout)?;
    }
    let cache = cache(&plan)?;

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
        &music,
        output.effects,
        &cache,
        out,
        options,
        &theme,
        progress,
    )?;
    if let Some(out) = out
        && derived.is_none()
    {
        crate::captions::write_sidecar(video_dir, &plan, output, out)
            .map_err(io_error(&out.with_extension("vtt")))?;
    }
    Ok(ExportReport {
        output: out.map_or_else(|| cache.dir.clone(), Path::to_owned),
        chunks: plan.output_chunks().count(),
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
    theme: &Theme,
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
            theme,
            faces: &faces,
            transcripts: &transcripts,
        },
    ))
}

fn media_path(video_dir: &Path, take: &Id, file: &str) -> PathBuf {
    video_dir.join("media").join(take.as_str()).join(file)
}

/// Runs of consecutive chunks of one piece that are `wanted` and have no cached file, each
/// chunk once.
fn jobs<'a>(
    plan: &'a ExportPlan,
    cache: &Cache,
    wanted: impl Fn(&PlannedChunk) -> bool,
    derived: bool,
) -> Vec<Job<'a>> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for &piece in &plan.pieces {
        let job = |run| Job::new(plan, piece, run, derived);
        let mut run: Option<Range<usize>> = None;
        for (index, (_, chunk)) in plan.chunks_of(piece).enumerate() {
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

/// Chunks that follow each other in one piece, rendered by one encoder session.
pub(crate) struct Job<'a> {
    /// The plan the piece is in: the output's, or for a derived cut's base chunks the main
    /// cut's.
    pub plan: &'a ExportPlan,
    pub piece: Piece,
    pub chunks: &'a [PlannedChunk],
    /// The segment's index in its plan, for a segment, and the chunks' indices in the piece.
    pub segment: usize,
    pub range: Range<usize>,
    /// Whether the chunks are a derived cut's, drawn from main chunks.
    pub derived: bool,
}

impl<'a> Job<'a> {
    fn new(plan: &'a ExportPlan, piece: Piece, range: Range<usize>, derived: bool) -> Self {
        let (chunks, segment) = match piece {
            Piece::Segment(i) => (&plan.segments[i].chunks, i),
            Piece::Bookend(i) => (&plan.bookends[i].chunks, i),
        };
        Self {
            plan,
            piece,
            chunks: &chunks[range.clone()],
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
    /// The lock's music cues and sound effects, in take time.
    cues: Cues,
    sounds: shot_audio::sound::Library,
    /// The brand's automatic sounds.
    brand_sounds: Vec<(shot_core::SoundKey, Id)>,
    /// The video folder, which music files are relative to.
    dir: PathBuf,
    /// The channel repo, which bumpers are relative to.
    channel: PathBuf,
    /// Where beats are cached, see [`tracks`].
    beats: PathBuf,
}

/// The plan of the main cut as export draws it, for the preview: the timeline styled by the
/// lock, with its bookends and transitions and its entrances on the beat. Chunk keys are left
/// out; the preview draws frames by [`Painter::paint`] and caches nothing.
pub fn preview_plan(video_dir: &Path, project: &Project) -> Result<ExportPlan, ExportError> {
    preview_plan_with(video_dir, project, crate::lock::read(video_dir)?.as_ref())
}

/// [`preview_plan`] styled by `lock` instead of the folder's `cut.lock`: a proposal's preview
/// plays its own lock and leaves the branch's alone.
pub fn preview_plan_with(
    video_dir: &Path,
    project: &Project,
    lock: Option<&Lock>,
) -> Result<ExportPlan, ExportError> {
    let timeline = match lock {
        Some(lock) => shot_core::Timeline::styled(project, lock),
        None => shot_core::Timeline::evaluate(project),
    };
    let main = timeline.main();
    let theme = theme(video_dir, lock, project)?;
    let media = Media::new(video_dir, project, lock);
    let joins = match lock {
        Some(lock) => Some(joins(&media, project, lock, &theme)?),
        None => None,
    };
    let plan_of = |output: &shot_core::Output| {
        let plan = ExportPlan::new(output, FPS, |_| MediaId::default());
        match &joins {
            Some(joins) => plan.with_joins(joins),
            None => plan,
        }
    };
    let plan = plan_of(main);
    if !theme.beat_sync {
        return Ok(plan);
    }
    let music = music(&plan, None, &media)?;
    Ok(match snap_entrances(main, &plan, &music) {
        Some(snapped) => plan_of(&snapped),
        None => plan,
    })
}

/// The voice of `plan` for the video in `video_dir`, as export and the preview read it: each
/// take's mic placed and blended by the plan, silent under the bleeps.
pub fn plan_audio(video_dir: &Path, plan: &ExportPlan) -> TimelineAudio {
    let root = video_dir.join("media");
    let audio = TimelineAudio::of_plan(plan, move |take| root.join(take.as_str()).join(MIC_FILE));
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
    audio.with_bleeps(&bleeps)
}

/// What plays with the voice of `plan`, the [`preview_plan`] of the video in `video_dir`, as
/// export mixes it: the music cues, the lock's sounds, the brand's sounds on every move and the
/// bumpers' own sound. The preview mixes it from any sample with [`voice::Score::mix_from`].
pub fn preview_score(
    video_dir: &Path,
    project: &Project,
    plan: &ExportPlan,
) -> Result<voice::Score, ExportError> {
    preview_score_with(
        video_dir,
        project,
        plan,
        crate::lock::read(video_dir)?.as_ref(),
    )
}

/// [`preview_score`] styled by `lock` instead of the folder's `cut.lock`, for a proposal.
pub fn preview_score_with(
    video_dir: &Path,
    project: &Project,
    plan: &ExportPlan,
    lock: Option<&Lock>,
) -> Result<voice::Score, ExportError> {
    let timeline = match lock {
        Some(lock) => shot_core::Timeline::styled(project, lock),
        None => shot_core::Timeline::evaluate(project),
    };
    let theme = theme(video_dir, lock, project)?;
    let media = Media::new(video_dir, project, lock);
    let music = music(plan, None, &media)?;
    score(plan, &music, &media, &theme, &timeline.main().effects)
}

/// What the lock's brand and transitions put around and between the main cut's segments, with
/// each bumper measured: a video by its track, a Lottie by its composition. A bumper the brand
/// names but the channel lacks is an error. The intro plays after the hook in the opinion and
/// reaction formats (the cold open), before the video in the others. The intro's generated title
/// shows the video's title, the outro's only the logo.
fn joins(
    media: &Media,
    project: &Project,
    lock: &shot_core::Lock,
    theme: &Theme,
) -> Result<Joins, ExportError> {
    let brand = &lock.brand;
    let bookend = |bookend: &Option<shot_core::Bookend>,
                   kind: BookendKind|
     -> Result<Option<BookendInput>, ExportError> {
        let Some(bookend) = bookend else {
            return Ok(None);
        };
        let title = bookend.title.then(|| match kind {
            BookendKind::Intro => project.video.title.clone(),
            BookendKind::Outro => String::new(),
        });
        let mut identity = Vec::new();
        let mut length = crate::bookend::TITLE_LENGTH;
        if let Some(file) = &bookend.bumper {
            let path = media.channel.join(file);
            if !path.exists() {
                return Err(ExportError::MissingBumper(path));
            }
            length = if is_lottie(file) {
                let bytes = fs::read(&path).map_err(io_error(&path))?;
                crate::lottie::LottieTemplate::load(&bytes, &[])
                    .and_then(|t| t.fill(&[]))
                    .map_err(|source| PaintError::Lottie {
                        path: path.clone(),
                        source,
                    })?
                    .duration()
            } else {
                let decoder = Decoder::open_native(&path).map_err(decode_error(&path))?;
                Micros(decoder.duration().as_micros() as u64)
            };
            identity.extend(media.asset_id(&media.channel, file));
        }
        if let Some(title) = &title {
            identity.extend_from_slice(b"title");
            identity.extend_from_slice(&(title.len() as u64).to_le_bytes());
            identity.extend_from_slice(title.as_bytes());
            identity.extend(theme.end_card_key());
        }
        Ok(Some(BookendInput {
            bumper: bookend.bumper.clone(),
            title,
            length,
            identity,
        }))
    };
    Ok(Joins {
        intro: bookend(&brand.intro, BookendKind::Intro)?,
        outro: bookend(&brand.outro, BookendKind::Outro)?,
        intro_after_hook: matches!(
            project.video.format,
            shot_core::FormatKind::Opinion | shot_core::FormatKind::Reaction
        ),
        transitions: Joins::transitions_of(lock),
        stinger: theme.stinger.as_ref().map(|s| StingerTiming {
            duration: s.lottie.duration(),
            peak: s.peak,
        }),
        end_card: brand.end_card,
    })
}

/// The lock's `music` and `sfx` instances by take, still in media time.
#[derive(Debug, Default)]
struct Cues {
    music: Vec<(Id, Micros, String, i16, i16)>,
    sfx: Vec<(Id, Micros, Id, i16)>,
}

impl Cues {
    fn of(lock: &shot_core::Lock) -> Self {
        let mut cues = Self::default();
        for take in &lock.takes {
            for instance in &take.instances {
                match &instance.applied {
                    shot_core::Applied::Music {
                        file,
                        at,
                        gain,
                        duck,
                    } => cues
                        .music
                        .push((take.take.clone(), *at, file.clone(), *gain, *duck)),
                    shot_core::Applied::Sfx { sound, at, gain } => {
                        cues.sfx
                            .push((take.take.clone(), *at, sound.clone(), *gain))
                    }
                    _ => {}
                }
            }
        }
        cues
    }
}

/// The theme `lock`'s brand gives the video in `video_dir`, as [`Theme::load`] reads it from
/// the folder's own lock.
pub fn theme(
    video_dir: &Path,
    lock: Option<&Lock>,
    project: &Project,
) -> Result<Theme, crate::theme::ThemeError> {
    let none = Brand::default();
    let brand = lock.map_or(&none, |lock| &lock.brand);
    let root = crate::theme::channel_root(video_dir).unwrap_or(video_dir);
    Theme::new(brand, project.video.format, Some(&project.cut), &|file| {
        let path = root.join(file);
        fs::read(&path).map_err(|source| crate::theme::ThemeError::Io { path, source })
    })
}

/// The channel's `brand/` folder when the video is `<repo>/videos/<slug>`.
fn channel_brand(video_dir: &Path) -> Option<PathBuf> {
    let brand = video_dir.parent()?.parent()?.join("brand");
    brand.is_dir().then_some(brand)
}

/// Where take time `at` of `take` plays in the output, in samples, once per segment showing it.
fn output_samples(plan: &ExportPlan, take: &Id, at: Micros) -> Vec<u64> {
    let per_frame = u64::from(SAMPLE_RATE) / u64::from(FPS);
    let t = at.0 * u64::from(SAMPLE_RATE) / 1_000_000;
    plan.segments
        .iter()
        .filter(|p| p.segment.take == *take)
        .filter_map(|p| {
            let start = p.source_frames.start * per_frame;
            let end = p.source_frames.end * per_frame;
            (start..end)
                .contains(&t)
                .then(|| p.output_frame * per_frame + t - start)
        })
        .collect()
}

/// Music files decoded with their beats, kept for the next export or preview plan, so the
/// snapped entrances cost a preview nothing. Only the files the last call used are kept.
/// Beats are detected once per file content ([0023]): `.shot/cache/beats/<hash>-<detector>.txt`
/// in the channel repo, shared by its videos, or in the video folder outside a channel.
///
/// [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md
mod tracks {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
    use std::time::SystemTime;

    use shot_audio::beats::{self, Beats};
    use shot_core::ContentHash;

    use super::{ExportError, TimelineAudioError, io_error, voice};

    type Cache = BTreeMap<PathBuf, ((u64, SystemTime), shot_audio::Track)>;

    static CACHE: Mutex<Cache> = Mutex::new(BTreeMap::new());

    fn cache() -> MutexGuard<'static, Cache> {
        CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The file at `path` with its beats, by its size and modification time, its beats cached
    /// in `beats_dir`.
    pub(super) fn load(path: &Path, beats_dir: &Path) -> Result<shot_audio::Track, ExportError> {
        let metadata = std::fs::metadata(path).map_err(super::io_error(path))?;
        let identity = (
            metadata.len(),
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        );
        if let Some((known, track)) = cache().get(path)
            && *known == identity
        {
            return Ok(track.clone());
        }
        let samples = voice::read_samples(path).map_err(|source| TimelineAudioError {
            path: path.to_owned(),
            source,
        })?;
        let bytes = std::fs::read(path).map_err(io_error(path))?;
        let file = beats_dir.join(format!(
            "{}-{}.txt",
            ContentHash::of(&bytes),
            beats::DETECTOR
        ));
        let cached = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| Beats::parse(&text));
        let beats = match cached {
            Some(beats) => beats,
            None => {
                let beats = beats::detect(&samples);
                std::fs::create_dir_all(beats_dir).map_err(io_error(beats_dir))?;
                // Written aside and renamed, so a reader never sees half a file.
                let part = file.with_extension("part");
                std::fs::write(&part, beats.to_text()).map_err(io_error(&part))?;
                std::fs::rename(&part, &file).map_err(io_error(&file))?;
                beats
            }
        };
        let track = shot_audio::Track::new(shot_audio::sound::Sound::new(samples), beats);
        cache().insert(path.to_owned(), (identity, track.clone()));
        Ok(track)
    }

    /// Drops every file but `used`.
    pub(super) fn keep_only(used: &[PathBuf]) {
        cache().retain(|path, _| used.contains(path));
    }
}

/// The output's music: the cut's music bed from the start, then each of the lock's cues where
/// its moment plays. A derived cut (with the `main` plan it draws from) also starts, at each
/// clip, the cue the main cut plays there, so a clip keeps the music under it. A cue already
/// playing runs on.
fn music(
    plan: &ExportPlan,
    main: Option<&ExportPlan>,
    media: &Media,
) -> Result<shot_audio::Music, ExportError> {
    let mut music = Vec::new();
    let mut used = Vec::new();
    if let Some((path, gain, duck)) = &media.music {
        used.push(path.clone());
        music.push(shot_audio::MusicCue {
            at: 0,
            play: Some(shot_audio::Play {
                track: tracks::load(path, &media.beats)?,
                gain_db: *gain,
                duck_db: *duck,
            }),
        });
    }
    let cues = &media.cues.music;
    let mut loaded: HashMap<usize, shot_audio::Track> = HashMap::new();
    for (at, index) in music_places(plan, main, cues) {
        let (_, _, file, gain, duck) = &cues[index];
        let track = match loaded.get(&index) {
            Some(track) => track.clone(),
            None => {
                let path = media.dir.join(file);
                let track = tracks::load(&path, &media.beats)?;
                used.push(path);
                loaded.insert(index, track.clone());
                track
            }
        };
        music.push(shot_audio::MusicCue {
            at,
            play: Some(shot_audio::Play {
                track,
                gain_db: f32::from(*gain),
                duck_db: f32::from(*duck),
            }),
        });
    }
    tracks::keep_only(&used);
    Ok(shot_audio::Music::new(music, shot_audio::CROSSFADE))
}

/// Where each of `cues` starts in `plan`, in output samples, with its index, in output order.
/// See [`music`] for `main`.
fn music_places(
    plan: &ExportPlan,
    main: Option<&ExportPlan>,
    cues: &[(Id, Micros, String, i16, i16)],
) -> Vec<(u64, usize)> {
    let per_frame = u64::from(SAMPLE_RATE) / u64::from(FPS);
    let in_main: Vec<(u64, usize)> = main.map_or_else(Vec::new, |main| {
        let mut places: Vec<(u64, usize)> = cues
            .iter()
            .enumerate()
            .flat_map(|(i, (take, at, ..))| {
                output_samples(main, take, *at)
                    .into_iter()
                    .map(move |s| (s, i))
            })
            .collect();
        places.sort_unstable();
        places
    });
    let mut places = Vec::new();
    let mut playing = None;
    for planned in &plan.segments {
        let take = &planned.segment.take;
        if let Some(main) = main {
            let start = frame_time(planned.source_frames.start, FPS);
            let under = output_samples(main, take, start).first().and_then(|&at| {
                let i = in_main.partition_point(|&(s, _)| s <= at);
                i.checked_sub(1).map(|i| in_main[i].1)
            });
            if let Some(cue) = under
                && playing != Some(cue)
            {
                places.push((planned.output_frame * per_frame, cue));
                playing = Some(cue);
            }
        }
        let first = planned.source_frames.start * per_frame;
        let last = planned.source_frames.end * per_frame;
        let mut inside: Vec<(u64, usize)> = cues
            .iter()
            .enumerate()
            .filter(|(_, (cue_take, ..))| cue_take == take)
            .filter_map(|(i, (_, at, ..))| {
                let t = at.0 * u64::from(SAMPLE_RATE) / 1_000_000;
                (first..last)
                    .contains(&t)
                    .then(|| (planned.output_frame * per_frame + t - first, i))
            })
            .collect();
        inside.sort_unstable();
        for (at, i) in inside {
            if playing != Some(i) {
                places.push((at, i));
                playing = Some(i);
            }
        }
    }
    places
}

/// `output` with the entrances that do not cut moved onto the beat of the music playing where
/// they start, as [`shot_audio::Music::snap`] finds it in `plan`, and their ends kept ([0023]).
/// An entrance moves only inside the segment it starts in, so a chapter band never leaves its
/// chapter. Redactions and b-roll keep their times, and so does every cut. `None` when nothing
/// moved.
///
/// [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md
fn snap_entrances(
    output: &shot_core::Output,
    plan: &ExportPlan,
    music: &shot_audio::Music,
) -> Option<shot_core::Output> {
    const FIXED: [&str; 3] = ["blur", "bleep", "b-roll"];
    let per_frame = u64::from(SAMPLE_RATE) / u64::from(FPS);
    let mut moved: HashMap<(Id, Id, shot_core::TimeRange), Micros> = HashMap::new();
    for planned in &plan.segments {
        let take = &planned.segment.take;
        for overlay in &planned.segment.overlays {
            if FIXED.contains(&overlay.template.as_str()) {
                continue;
            }
            let key = (take.clone(), overlay.template.clone(), overlay.range);
            if moved.contains_key(&key) {
                continue;
            }
            let from = overlay.range.from();
            let t = from.0 * u64::from(SAMPLE_RATE) / 1_000_000;
            let source =
                planned.source_frames.start * per_frame..planned.source_frames.end * per_frame;
            if !source.contains(&t) {
                continue;
            }
            let shown = planned.output_frame * per_frame
                ..(planned.output_frame + planned.frames()) * per_frame;
            let at = shown.start + t - source.start;
            let snapped = music.snap(at, per_frame);
            // A beat across the segment's edge is in other material: the entrance stays.
            if snapped == at || !shown.contains(&snapped) {
                continue;
            }
            let shift = |samples: u64| samples * 1_000_000 / u64::from(SAMPLE_RATE);
            let start = if snapped > at {
                Micros(from.0 + shift(snapped - at))
            } else {
                Micros(from.0.saturating_sub(shift(at - snapped)))
            };
            if start < overlay.range.to() {
                moved.insert(key, start);
            }
        }
    }
    if moved.is_empty() {
        return None;
    }
    let mut output = output.clone();
    for segment in output.chapters.iter_mut().flat_map(|c| &mut c.segments) {
        for overlay in &mut segment.overlays {
            let key = (
                segment.take.clone(),
                overlay.template.clone(),
                overlay.range,
            );
            if let Some(&start) = moved.get(&key) {
                overlay.range =
                    shot_core::TimeRange::new(start, overlay.range.to()).expect("start before end");
            }
        }
    }
    Some(output)
}

/// Sound effects and bumpers over `music`: the lock's sounds where their moments play, the
/// brand's sounds on every move ([`crate::sounds`]) and the bumpers' own sound, which the music
/// ducks under.
fn score(
    plan: &ExportPlan,
    music: &shot_audio::Music,
    media: &Media,
    theme: &Theme,
    effects: &Effects,
) -> Result<voice::Score, ExportError> {
    let read = |path: &Path| {
        voice::read_samples(path).map_err(|source| TimelineAudioError {
            path: path.to_owned(),
            source,
        })
    };
    let mut found: HashMap<Id, shot_audio::sound::Sound> = HashMap::new();
    let mut find = |name: &Id| -> Result<shot_audio::sound::Sound, ExportError> {
        if let Some(sound) = found.get(name) {
            return Ok(sound.clone());
        }
        let sound = match media.sounds.find(name) {
            Some(shot_audio::sound::Source::File(path)) => {
                shot_audio::sound::Sound::new(read(&path)?)
            }
            Some(shot_audio::sound::Source::Bundled(bundled)) => bundled.decode(),
            None => return Err(ExportError::UnknownSound(name.clone())),
        };
        found.insert(name.clone(), sound.clone());
        Ok(sound)
    };
    let per_frame = u64::from(SAMPLE_RATE) / u64::from(FPS);
    let mut assets = crate::assets::Assets::new(&media.dir);
    let mut failed = None;
    let mut clicks = |take: &Id| match assets.events(take) {
        Ok(events) => events.map_or_else(Vec::new, |e| e.clicks().iter().map(|c| c.t).collect()),
        Err(e) => {
            failed.get_or_insert(e);
            Vec::new()
        }
    };
    let automatic =
        crate::sounds::automatic(plan, theme, effects, &media.brand_sounds, &mut clicks);
    if let Some(e) = failed {
        return Err(e.into());
    }
    let mut sfx = Vec::with_capacity(automatic.len());
    for (name, anchor) in automatic {
        sfx.push(shot_audio::SfxCue::new(find(name)?, anchor, 0.0));
    }
    let mut bumpers = Vec::new();
    for bookend in &plan.bookends {
        let Some(file) = bookend.bumper.as_deref().filter(|f| !is_lottie(f)) else {
            continue;
        };
        let start = bookend.output_frame * per_frame;
        let end = start + bookend.frames * per_frame;
        bumpers.push(start..end);
        let path = media.channel.join(file);
        let mut samples = match voice::read_samples(&path) {
            Ok(samples) => samples,
            Err(shot_media::audio::AudioError::NoAudioTrack(_)) => continue,
            Err(source) => return Err(TimelineAudioError { path, source }.into()),
        };
        samples.truncate((end - start) as usize);
        sfx.push(shot_audio::SfxCue::new(
            shot_audio::sound::Sound::new(samples),
            shot_audio::Anchor::Start(start),
            0.0,
        ));
    }
    for (take, at, name, gain) in &media.cues.sfx {
        let places = output_samples(plan, take, *at);
        if places.is_empty() {
            continue;
        }
        let sound = find(name)?;
        sfx.extend(places.into_iter().map(|at| {
            shot_audio::SfxCue::new(
                sound.clone(),
                shot_audio::Anchor::Start(at),
                f32::from(*gain),
            )
        }));
    }
    Ok(voice::Score {
        music: music.clone(),
        sfx,
        bumpers,
    })
}

impl Media {
    fn new(video_dir: &Path, project: &Project, lock: Option<&shot_core::Lock>) -> Self {
        Self {
            root: video_dir.join("media"),
            music: project.cut.music.as_ref().map(|m| {
                (
                    video_dir.join(&m.file),
                    f32::from(m.gain),
                    f32::from(m.duck),
                )
            }),
            cues: lock.map(Cues::of).unwrap_or_default(),
            sounds: shot_audio::sound::Library::new(video_dir, channel_brand(video_dir).as_deref()),
            brand_sounds: lock.map(|l| l.brand.sounds.clone()).unwrap_or_default(),
            dir: video_dir.to_owned(),
            channel: crate::theme::channel_root(video_dir)
                .unwrap_or(video_dir)
                .to_owned(),
            beats: channel_brand(video_dir)
                .and_then(|brand| brand.parent().map(Path::to_owned))
                .unwrap_or_else(|| video_dir.to_owned())
                .join(BEATS_DIR),
        }
    }

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
}

/// Where rendered chunks live: the video's `.shot/cache/chunks/`, and the channel's for the
/// bookends' chunks, since their keys never name the video and every video with the same brand
/// reuses them ([0021]).
///
/// [0021]: ../../../docs/decisions/0021-sound-transitions-and-bookends.md
pub(crate) struct Cache {
    /// Where this export writes its chunks.
    dir: PathBuf,
    channel: Option<(PathBuf, HashSet<ContentHash>)>,
    /// For a proposal: the video's and the channel's chunk folders, read and never written.
    branch: Vec<PathBuf>,
}

impl Cache {
    fn new(video_dir: &Path, plan: &ExportPlan) -> Result<Self, ExportError> {
        let dir = video_dir.join(CHUNKS_DIR);
        fs::create_dir_all(&dir).map_err(io_error(&dir))?;
        let shared: HashSet<ContentHash> = plan
            .bookends
            .iter()
            .flat_map(|b| &b.chunks)
            .map(|c| c.hash)
            .collect();
        let channel = match channel_brand(video_dir).and_then(|b| b.parent().map(Path::to_owned)) {
            Some(root) if !shared.is_empty() => {
                let dir = root.join(CHUNKS_DIR);
                fs::create_dir_all(&dir).map_err(io_error(&dir))?;
                Some((dir, shared))
            }
            _ => None,
        };
        Ok(Self {
            dir,
            channel,
            branch: Vec::new(),
        })
    }

    /// A proposal's cache: `dir`, over the branch's chunks of the video and the channel.
    fn proposal(video_dir: &Path, dir: &Path) -> Result<Self, ExportError> {
        fs::create_dir_all(dir).map_err(io_error(dir))?;
        let channel = channel_brand(video_dir)
            .and_then(|brand| brand.parent().map(|root| root.join(CHUNKS_DIR)));
        Ok(Self {
            dir: dir.to_owned(),
            channel: None,
            branch: [video_dir.join(CHUNKS_DIR)]
                .into_iter()
                .chain(channel)
                .collect(),
        })
    }

    pub(crate) fn path(&self, hash: ContentHash) -> PathBuf {
        let file = format!("{hash}.mp4");
        if let Some(cached) = self
            .branch
            .iter()
            .map(|dir| dir.join(&file))
            .find(|path| path.exists())
        {
            return cached;
        }
        let dir = match &self.channel {
            Some((dir, shared)) if shared.contains(&hash) => dir,
            _ => &self.dir,
        };
        dir.join(file)
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
    music: &shot_audio::Music,
    effects: Effects,
    cache: &Cache,
    out: Option<&Path>,
    options: ExportOptions,
    theme: &Arc<Theme>,
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
        // Without an output there is nothing to join, and so no audio to encode.
        let (audio, joiner) = match out {
            Some(out) => (
                Some(scope.spawn(move || {
                    pending.guard(|| {
                        let score = score(plan, music, media, theme, &effects)?;
                        encode_audio(plan, media, &score, &audio_out)
                    })
                })),
                Some(
                    scope
                        .spawn(move || pending.guard(|| join(plan, cache, pending, audio_in, out))),
                ),
            ),
            None => (None, None),
        };
        let workers: Vec<_> = (0..options.jobs.max(1))
            .map(|_| {
                scope.spawn(|| {
                    pending.guard(|| {
                        let mut worker = Worker::new(media, effects, theme.clone())?;
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
                                None => worker.render(&job, cache, &chunk_done)?,
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
        results.extend(audio.map(|audio| audio.join().expect("audio thread panicked")));
        let render_time = started.elapsed();
        results.extend(joiner.map(|joiner| joiner.join().expect("join thread panicked")));
        let join_time = started.elapsed() - render_time;
        let mut errors = results.into_iter().filter_map(Result::err);
        match errors.find(|error| !matches!(error, ExportError::Cancelled)) {
            Some(error) => Err(error),
            None if pending.failed() => Err(ExportError::Cancelled),
            None => Ok((render_time, join_time)),
        }
    })
}

/// One render thread: its own GPU device, and a painter that keeps decoders and assets open
/// across jobs.
struct Worker {
    renderer: Renderer,
    painter: Painter,
}

impl Worker {
    fn new(media: &Media, effects: Effects, theme: Arc<Theme>) -> Result<Self, ExportError> {
        Ok(Self {
            renderer: Renderer::new()?,
            painter: Painter::with_theme(&media.dir, effects, theme),
        })
    }

    /// Renders the chunks of `job` into the cache, calling `chunk_done` after each one.
    fn render(
        &mut self,
        job: &Job,
        cache: &Cache,
        chunk_done: &dyn Fn(ContentHash),
    ) -> Result<(), ExportError> {
        let canvas = Canvas::UHD;
        let mut encoder = HevcEncoder::new(EncoderSettings {
            width: canvas.width,
            height: canvas.height,
            fps: FPS,
            keyframe_interval: KEYFRAME_INTERVAL,
            quality: QUALITY,
        })?;
        let mut files = ChunkFiles::new(job.chunks, cache, canvas, chunk_done);
        let starts = job
            .plan
            .chunks_of(job.piece)
            .skip(job.range.start)
            .map(|(start, _)| start);
        for (chunk, start) in job.chunks.iter().zip(starts) {
            for i in 0..chunk.frame_count() {
                let target = encoder.input_buffer()?;
                self.painter
                    .paint(&self.renderer, job.plan, start + i, &target)?;
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

/// The mic audio of every segment, in output order, as one AAC stream sent to `out`: first a
/// piece with the format and no packets, at once, then the packets when they are final.
///
/// The encoder's output peaks higher than its input, most on hard-limited transients such as
/// many loud sound effects, at times by more than the limiter's codec headroom. So the stream is
/// decoded and measured before it is sent, and finished again with the limiter lower by what it
/// overshot until it is under [`CHECKED_CEILING_DBTP`]. The chain and score run once; another
/// attempt only finishes and encodes again.
fn encode_audio(
    plan: &ExportPlan,
    media: &Media,
    score: &voice::Score,
    out: &mpsc::Sender<AacStream>,
) -> Result<(), ExportError> {
    let encoder = || AacEncoder::new(SAMPLE_RATE, AUDIO_BITRATE).map_err(ExportError::AudioEncode);
    // The format is the same for every attempt: the join starts with it.
    let mut first = encoder()?;
    let format = first.take();
    out.send(format.clone())
        .map_err(|_| ExportError::Cancelled)?;
    let mut first = Some(first);

    let mut audio = plan_audio(&media.dir, plan);
    let mut mixed = voice::Mixed::new(&mut audio, score)?;
    let mut ceiling = shot_audio::CEILING_DBTP;
    for attempt in 0..ENCODE_ATTEMPTS {
        let finished = mixed.finish(ceiling);
        let mut encoder = match first.take() {
            Some(first) => first,
            None => encoder()?,
        };
        for block in finished.samples.chunks(SAMPLE_RATE as usize) {
            encoder.push(block).map_err(ExportError::AudioEncode)?;
        }
        drop(finished);
        let stream = encoder.finish().map_err(ExportError::AudioEncode)?;
        let peak = encoded_true_peak(&stream, &media.dir)?;
        if peak <= CHECKED_CEILING_DBTP || attempt == ENCODE_ATTEMPTS - 1 {
            return out.send(stream).map_err(|_| ExportError::Cancelled);
        }
        ceiling -= peak - CHECKED_CEILING_DBTP + 0.1;
    }
    unreachable!("the last attempt returns")
}

/// The ceiling an encoded stream is held to by the in-repo meter: -1 dBTP less a margin for
/// other meters and decoders, which measure up to about 0.1 dB higher.
const CHECKED_CEILING_DBTP: f64 = shot_audio::CEILING_DBTP - 0.2;
/// Encodes at most, the last one sent whatever it measures.
const ENCODE_ATTEMPTS: usize = 3;

/// The true peak of the AAC `stream` decoded, in dBTP. The stream is written to a scratch file
/// in the video's cache for the system decoder to read, and removed.
fn encoded_true_peak(stream: &AacStream, video_dir: &Path) -> Result<f64, ExportError> {
    use std::io::Write;
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = video_dir.join(".shot/cache").join(format!(
        "encoded-{}-{}.aac",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        fs::create_dir_all(path.parent().expect("a parent"))?;
        let mut file = BufWriter::new(File::create(&path)?);
        let mut offset = 0;
        for &size in &stream.sizes {
            // An ADTS header: AAC-LC, 48 kHz (index 3), mono, no CRC.
            let length = size as usize + 7;
            file.write_all(&[
                0xff,
                0xf1,
                (1 << 6) | (3 << 2),
                (1 << 6) | (length >> 11) as u8,
                (length >> 3) as u8,
                ((length & 7) << 5) as u8 | 0x1f,
                0xfc,
            ])?;
            file.write_all(&stream.data[offset..offset + size as usize])?;
            offset += size as usize;
        }
        file.flush()
    })();
    let measured = written.map_err(io_error(&path)).and_then(|()| {
        let total = (stream.sizes.len() * AAC_PACKET_SAMPLES as usize) as u64;
        let mut reader =
            shot_media::audio::PcmReader::open(&path).map_err(|source| TimelineAudioError {
                path: path.clone(),
                source,
            })?;
        let mut meter = shot_audio::Meter::new();
        let mut block = Vec::new();
        let mut at = 0;
        while at < total {
            let end = (at + u64::from(SAMPLE_RATE)).min(total);
            block.clear();
            reader
                .read(at, end, &mut block)
                .map_err(|source| TimelineAudioError {
                    path: path.clone(),
                    source,
                })?;
            meter.add(&block);
            at = end;
        }
        Ok(meter.true_peak())
    });
    let _ = fs::remove_file(&path);
    measured
}

/// Writes the output: one init segment, then every chunk's video fragments by stream copy,
/// each followed by room for the audio of the same span ([`AudioRoom`]). So the video is copied
/// while the audio still finishes, and the audio goes into its room once it is encoded. Waits
/// for chunks still rendering and for the audio.
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
    let (_, first) = plan.output_chunks().next().ok_or(ExportError::Cancelled)?;
    let video = read(first)?.track;
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
    let mut room = AudioRoom::new(&movie, format.priming).map_err(io_error(&partial))?;
    for (output_frame, chunk) in plan.output_chunks() {
        let file = read(chunk)?;
        let path = cache.path(chunk.hash);
        if file.track != video {
            return Err(MuxError::CodecMismatch(path.display().to_string()).into());
        }
        let mut source = File::open(&path).map_err(io_error(&path))?;
        let start = output_frame * u64::from(FRAME_TICKS);
        for fragment in &file.fragments {
            let base = start + fragment.base_time;
            joiner.copy(&mut source, fragment, 1, base)?;
            room.copied(fragment);
            room.reserve(&mut joiner, base + fragment.duration)
                .map_err(io_error(&partial))?;
        }
    }
    let file = joiner
        .finish()
        .map_err(io_error(&partial))?
        .into_inner()
        .map_err(|error| io_error(&partial)(error.into_error()))?;

    let mut stream = AacStream::default();
    loop {
        match audio.recv() {
            Ok(piece) => {
                stream.data.extend_from_slice(&piece.data);
                stream.sizes.extend_from_slice(&piece.sizes);
            }
            // The encoder hung up: at the end of the stream, or because it failed.
            Err(_) if pending.failed() => return Err(ExportError::Cancelled),
            Err(_) => break,
        }
    }
    room.fill(&file, &stream, &partial)?;
    file.sync_all().map_err(io_error(&partial))?;
    fs::rename(&partial, out).map_err(io_error(out))
}

/// The largest AAC-LC packet of one channel: 6144 bits.
const MAX_AAC_PACKET: u32 = 768;

/// The audio fragments of the output, placed before their packets are encoded: after each video
/// fragment, a fragment for the packets that start before it ends, which only their timing
/// decides. Each is written first with room for packets of [`MAX_AAC_PACKET`], then rewritten in
/// place with the real packets; the unused room stays zeros at the end of its `mdat`. That room
/// is about 0.3 % of a 4K export. The packets after the last video fragment are appended.
struct AudioRoom {
    priming: u32,
    slots: Vec<Slot>,
    /// The first packet without a slot, the next fragment's sequence number and where it
    /// starts in the file.
    next: usize,
    sequence: u32,
    offset: u64,
}

/// An audio fragment written as room: where, its sequence number and its packets.
struct Slot {
    offset: u64,
    sequence: u32,
    packets: Range<usize>,
}

impl AudioRoom {
    /// Room in a file the joiner starts with `movie`'s init segment.
    fn new(movie: &Movie, priming: u32) -> io::Result<Self> {
        let mut init = Vec::new();
        mux::write_init(&mut init, movie)?;
        Ok(Self {
            priming,
            slots: Vec::new(),
            next: 0,
            // The joiner numbers fragments from 1.
            sequence: 1,
            offset: init.len() as u64,
        })
    }

    /// Counts a fragment the joiner copied.
    fn copied(&mut self, fragment: &mux::FragmentSpan) {
        self.sequence += 1;
        self.offset += u64::from(fragment.moof_size) + u64::from(fragment.mdat_size);
    }

    /// Whether packet `packet` starts before `video_time` on the output timeline: its media
    /// time less the encoder delay.
    fn starts_before(&self, packet: usize, video_time: u64) -> bool {
        let start = packet as i64 * i64::from(AAC_PACKET_SAMPLES) - i64::from(self.priming);
        (start as i128) * i128::from(TIMESCALE) < (video_time as i128) * i128::from(SAMPLE_RATE)
    }

    /// Writes room for the packets that start before `video_time` and have none yet.
    fn reserve<W: io::Write>(&mut self, joiner: &mut Joiner<W>, video_time: u64) -> io::Result<()> {
        let first = self.next;
        let mut end = first;
        while self.starts_before(end, video_time) {
            end += 1;
        }
        if end == first {
            return Ok(());
        }
        let samples = vec![
            mux::Sample {
                size: MAX_AAC_PACKET,
                duration: AAC_PACKET_SAMPLES,
                sync: true,
            };
            end - first
        ];
        let base = first as u64 * u64::from(AAC_PACKET_SAMPLES);
        let zeros = vec![0; (end - first) * MAX_AAC_PACKET as usize];
        let mut room = Vec::with_capacity(zeros.len() + 128);
        mux::write_fragment(&mut room, self.sequence, 2, base, &samples, &zeros)?;
        joiner.write(2, base, &samples, &zeros)?;
        self.slots.push(Slot {
            offset: self.offset,
            sequence: self.sequence,
            packets: first..end,
        });
        self.sequence += 1;
        self.offset += room.len() as u64;
        self.next = end;
        Ok(())
    }

    /// Writes `stream`'s packets into their room in `file`, and the rest at the end.
    fn fill(&self, file: &File, stream: &AacStream, path: &Path) -> Result<(), ExportError> {
        use std::os::unix::fs::FileExt;
        let too_short = || MuxError::Format(path.display().to_string(), "audio ends before video");
        let mut offsets = Vec::with_capacity(stream.sizes.len() + 1);
        offsets.push(0usize);
        for &size in &stream.sizes {
            offsets.push(offsets.last().copied().unwrap_or(0) + size as usize);
        }
        let fragment = |packets: Range<usize>, sequence: u32| -> Result<Vec<u8>, ExportError> {
            let sizes = stream.sizes.get(packets.clone()).ok_or_else(too_short)?;
            let samples: Vec<mux::Sample> = sizes
                .iter()
                .map(|&size| mux::Sample {
                    size,
                    duration: AAC_PACKET_SAMPLES,
                    sync: true,
                })
                .collect();
            let mut bytes = Vec::new();
            mux::write_fragment(
                &mut bytes,
                sequence,
                2,
                packets.start as u64 * u64::from(AAC_PACKET_SAMPLES),
                &samples,
                &stream.data[offsets[packets.start]..offsets[packets.end]],
            )
            .map_err(io_error(path))?;
            Ok(bytes)
        };
        for slot in &self.slots {
            let mut bytes = fragment(slot.packets.clone(), slot.sequence)?;
            let room = slot.packets.len() as u32 * MAX_AAC_PACKET;
            let data = offsets[slot.packets.end] - offsets[slot.packets.start];
            if data > room as usize {
                return Err(
                    MuxError::Format(path.display().to_string(), "AAC packet too large").into(),
                );
            }
            // The `mdat` keeps the size of its room.
            let mdat = bytes.len() - data - 8;
            bytes[mdat..mdat + 4].copy_from_slice(&(room + 8).to_be_bytes());
            file.write_all_at(&bytes, slot.offset)
                .map_err(io_error(path))?;
        }
        if self.next < stream.sizes.len() {
            let bytes = fragment(self.next..stream.sizes.len(), self.sequence)?;
            file.write_all_at(&bytes, self.offset)
                .map_err(io_error(path))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::Joins;
    use shot_core::{
        Brand, Cut, Format, FormatLayer, MigrateInput, Overlay, Script, Timeline, TransitionKind,
        Video,
    };

    const BRAND: &str = r#"intro bumper="brand/media/intro.mp4" title=#true
outro bumper="brand/media/outro.mp4"
end-card length=12_000_000
transition stinger="brand/stinger.json" peak=300_000
on video-start {
    transition dip
    music "sources/bed.flac" gain=-20 duck=-12
}
on chapter-start {
    overlay chapter-band for=2_500_000
    sfx swoosh
    transition stinger
}
"#;

    /// The target project as `shot migrate` writes it with no elements, under `BRAND`, with a
    /// second derived cut later in the take.
    fn target() -> (Project, shot_core::Lock) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/target");
        let read = |name: &str| fs::read_to_string(dir.join(name)).unwrap();
        let cut =
            read("cut.kdl") + "\nshort later {\n    clip t01 from=300_000_000 to=305_000_000\n}\n";
        let old = Project {
            video: Video::parse(&read("video.kdl")).unwrap(),
            script: Script::parse(&read("script.md")).unwrap(),
            format: Format::parse(&read("format.kdl")).unwrap(),
            cut: Cut::parse(&cut).unwrap(),
        };
        let channel = FormatLayer::default();
        let migration = shot_core::migrate(&MigrateInput {
            project: &old,
            imported: &[],
            brand: &Brand::default(),
            channel: &channel,
            elements: &[],
            assets: &Default::default(),
        })
        .unwrap();
        // Then the brand arrives and `shot style build` runs.
        let brand = Brand::parse(BRAND).unwrap();
        let assets = brand
            .assets()
            .into_iter()
            .map(|file| (file.to_owned(), ContentHash::of(file.as_bytes())))
            .collect();
        let lock = shot_core::build(&shot_core::StyleInput {
            imported: &[],
            brand: &brand,
            format: migration.channel.as_ref().unwrap_or(&channel),
            video: &migration.video,
            cut: &migration.cut,
            elements: &[],
            assets: &assets,
        })
        .unwrap();
        let project = Project {
            cut: migration.cut,
            format: migration.format,
            ..old
        };
        (project, lock)
    }

    fn templates(plan: &ExportPlan) -> HashSet<String> {
        plan.segments
            .iter()
            .flat_map(|s| &s.segment.overlays)
            .map(|o| o.template.to_string())
            .collect()
    }

    #[test]
    fn a_short_of_the_target_project_has_no_bookends_cards_or_stingers_and_keeps_its_sounds() {
        let (project, lock) = target();
        let timeline = Timeline::styled(&project, &lock);
        let bookend = |bumper: &str, title: Option<&str>| BookendInput {
            bumper: Some(bumper.to_owned()),
            title: title.map(str::to_owned),
            length: Micros(3_000_000),
            identity: Vec::new(),
        };
        let joins = Joins {
            intro: Some(bookend("brand/media/intro.mp4", Some("t"))),
            outro: Some(bookend("brand/media/outro.mp4", None)),
            intro_after_hook: true,
            transitions: Joins::transitions_of(&lock),
            stinger: Some(StingerTiming {
                duration: Micros(1_000_000),
                peak: Micros(300_000),
            }),
            end_card: Some(Micros(12_000_000)),
        };
        let main = ExportPlan::new(timeline.main(), FPS, |_| MediaId::default()).with_joins(&joins);
        assert_eq!(main.bookends.len(), 2);
        assert!(main.end_card.is_some());
        assert!(
            main.transitions
                .iter()
                .any(|t| t.kind == TransitionKind::Stinger)
        );
        assert!(templates(&main).contains("chapter-band"));

        let cues = Cues::of(&lock);
        for (name, clip_frames) in [("intro-hook", 1752), ("later", 300)] {
            let output = timeline.outputs.iter().find(|o| o.name == name).unwrap();
            let theme = Theme::default();
            let derived = DerivedPlan::new(
                timeline.main(),
                output,
                &DerivedInputs {
                    fps: FPS,
                    media: &|_| MediaId::default(),
                    assets: &|_| Vec::new(),
                    theme: &theme,
                    faces: &HashMap::new(),
                    transcripts: &HashMap::new(),
                },
            );
            let plan = &derived.plan;
            assert!(
                plan.bookends.is_empty() && plan.transitions.is_empty(),
                "{name}"
            );
            assert_eq!(plan.end_card, None);
            assert_eq!(plan.frames, clip_frames, "{name}: only the clip's frames");
            for drawn in [templates(plan), templates(&derived.main)] {
                assert!(!drawn.contains("chapter-band") && !drawn.contains("end-card"));
            }
            // The video-start cue plays under both clips, from their start.
            assert_eq!(
                music_places(plan, Some(&derived.main), &cues.music),
                [(0, 0)],
                "{name}"
            );
            let sounds: Vec<u64> = cues
                .sfx
                .iter()
                .flat_map(|(take, at, ..)| output_samples(plan, take, *at))
                .collect();
            match name {
                "intro-hook" => {
                    // The first chapter's swoosh at its start, and its lower third.
                    assert_eq!(sounds, [0]);
                    assert!(templates(plan).contains("lower-third"));
                }
                _ => assert!(sounds.is_empty()),
            }
        }
    }

    #[test]
    fn entrances_snap_to_the_beat_and_cuts_keep_their_times() {
        let (project, lock) = target();
        let timeline = Timeline::styled(&project, &lock);
        let mut output = timeline.main().clone();
        let range = |from, to| shot_core::TimeRange::new(Micros(from), Micros(to)).unwrap();
        let overlay = |template: &str, from| Overlay {
            template: Id::new(template).unwrap(),
            range: range(from, from + 2_000_000),
            params: Vec::new(),
        };
        // 1.45 s and 1.7 s into the take, which plays from 0.8 s: output 0.65 s and 0.9 s.
        output.chapters[0].segments[0]
            .overlays
            .extend([overlay("pop", 1_450_000), overlay("blur", 1_700_000)]);
        output.chapters[0].segments[1]
            .overlays
            .push(overlay("callout", 33_020_000));
        let plan = ExportPlan::new(&output, FPS, |_| MediaId::default());
        // A beat every half second of a 10 s track from the start of the output.
        let samples = vec![0.0; 10 * SAMPLE_RATE as usize];
        let beats = shot_audio::beats::Beats {
            bpm: 120.0,
            at: (0..20).map(|i| i * SAMPLE_RATE as u64 / 2).collect(),
        };
        let music = shot_audio::Music::new(
            vec![shot_audio::MusicCue {
                at: 0,
                play: Some(shot_audio::Play {
                    track: shot_audio::Track::new(shot_audio::sound::Sound::new(samples), beats),
                    gain_db: -20.0,
                    duck_db: -12.0,
                }),
            }],
            shot_audio::CROSSFADE,
        );
        let snapped = snap_entrances(&output, &plan, &music).unwrap();
        let starts: HashMap<String, Micros> = snapped.chapters[0].segments[0]
            .overlays
            .iter()
            .map(|o| (o.template.to_string(), o.range.from()))
            .collect();
        // Output 0.65 s moves 150 ms back onto the beat at 0.5 s, so the pop starts at 1.3 s.
        assert_eq!(starts["pop"], Micros(1_300_000));
        assert_eq!(starts["blur"], Micros(1_700_000));
        // The take's second kept range plays from output 30.6 s. A callout 20 ms into it is
        // nearest the beat at 30.5 s, before the cut, so it stays.
        let second = &snapped.chapters[0].segments[1].overlays;
        let callout = second
            .iter()
            .find(|o| o.template.as_str() == "callout")
            .unwrap();
        assert_eq!(callout.range.from(), Micros(33_020_000));
        assert_eq!(
            snapped.chapters[0].segments[0].source,
            output.chapters[0].segments[0].source
        );
    }

    #[test]
    fn beats_are_detected_once_per_file_content() {
        let dir = tempfile::tempdir().unwrap();
        let bed = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../shot-audio/assets/sounds/bed-pulse.flac");
        let (first, copy) = (dir.path().join("a.flac"), dir.path().join("b.flac"));
        fs::copy(&bed, &first).unwrap();
        fs::copy(&bed, &copy).unwrap();
        let cache = dir.path().join("beats");
        tracks::load(&first, &cache).unwrap();
        let files: Vec<PathBuf> = fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let hash = ContentHash::of(&fs::read(&bed).unwrap());
        assert_eq!(
            files,
            [cache.join(format!("{hash}-{}.txt", shot_audio::beats::DETECTOR))]
        );
        assert!(shot_audio::beats::Beats::parse(&fs::read_to_string(&files[0]).unwrap()).is_some());

        // Another file with the same content reads its beats from the cache, not the music.
        fs::write(&files[0], "bpm 60\n100000\n").unwrap();
        let track = tracks::load(&copy, &cache).unwrap();
        let music = shot_audio::Music::new(
            vec![shot_audio::MusicCue {
                at: 0,
                play: Some(shot_audio::Play {
                    track,
                    gain_db: 0.0,
                    duck_db: 0.0,
                }),
            }],
            shot_audio::CROSSFADE,
        );
        assert_eq!(music.snap(104_000, 800), 100_000);
    }

    #[test]
    fn bookend_chunks_are_cached_in_the_channel() {
        let (project, lock) = target();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("brand")).unwrap();
        let video = root.path().join("videos/target");
        fs::create_dir_all(&video).unwrap();
        let joins = Joins {
            intro: Some(BookendInput {
                bumper: None,
                title: Some("t".to_owned()),
                length: Micros(3_000_000),
                identity: Vec::new(),
            }),
            ..Joins::default()
        };
        let timeline = Timeline::styled(&project, &lock);
        let plan = ExportPlan::new(timeline.main(), FPS, |_| MediaId::default()).with_joins(&joins);
        let cache = Cache::new(&video, &plan).unwrap();
        let bookend = plan.bookends[0].chunks[0].hash;
        let segment = plan.segments[0].chunks[0].hash;
        assert_eq!(
            cache.path(bookend),
            root.path()
                .join(format!(".shot/cache/chunks/{bookend}.mp4"))
        );
        assert_eq!(
            cache.path(segment),
            video.join(format!(".shot/cache/chunks/{segment}.mp4"))
        );
    }

    #[test]
    fn a_proposal_reads_branch_chunks_and_writes_only_its_own() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("brand")).unwrap();
        let video = root.path().join("videos/target");
        let branch = video.join(CHUNKS_DIR);
        fs::create_dir_all(&branch).unwrap();
        let (cached, new) = (ContentHash::of(b"cached"), ContentHash::of(b"new"));
        fs::write(branch.join(format!("{cached}.mp4")), b"").unwrap();
        let channel = root.path().join(CHUNKS_DIR);
        fs::create_dir_all(&channel).unwrap();
        let bookend = ContentHash::of(b"bookend");
        fs::write(channel.join(format!("{bookend}.mp4")), b"").unwrap();

        let space = proposal_dir(&video, "tighter").join("chunks");
        let cache = Cache::proposal(&video, &space).unwrap();
        assert_eq!(cache.path(cached), branch.join(format!("{cached}.mp4")));
        assert_eq!(cache.path(bookend), channel.join(format!("{bookend}.mp4")));
        assert_eq!(cache.path(new), space.join(format!("{new}.mp4")));
        assert_eq!(
            space,
            video.join(".shot/cache/proposals/tighter/chunks"),
            "the proposal's space is its own folder"
        );
    }
}
