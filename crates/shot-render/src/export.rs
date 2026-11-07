//! Incremental export: render each segment whose content hash has no cached file, in parallel on
//! the hardware encoder, then join every segment by stream copy into one fragmented MP4.
//!
//! - **Segments** are cached as `.shot/cache/segments/<hash>.mp4`: video only, 4K60 HEVC with a
//!   keyframe every 0.5 s, one fragment per keyframe interval. A file that exists is reused.
//!   Stale files are never deleted yet.
//! - **Audio** is cut from each take's mic track and encoded to AAC once per export, on its own
//!   thread while video renders. One continuous AAC stream has no encoder delay or padding at the
//!   joins, so they cannot click or drift. Where the source jumps, the cut fades over 5 ms.
//! - **The join** writes one init segment, then copies each segment's fragments with shifted
//!   decode times, with the audio fragments for the same span after each one.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use shot_core::{ContentHash, Id, OutputKind, Project, Timeline};
use shot_media::audio::{
    AAC_PACKET_SAMPLES, AacEncoder, AacStream, AudioError, PcmReader, SAMPLE_RATE,
};
use shot_media::encode::{EncodeError, EncodedFrame, EncoderSettings, HevcEncoder};
use shot_media::mux::{self, AudioTrack, Chapter, FragmentedFile, Joiner, Movie, MuxError, Track};
use shot_media::videotoolbox::{DecodeError, Decoder, Frame};

use crate::RenderError;
use crate::graph::Renderer;
use crate::layout::{Canvas, LayoutSpec, Source, Sources, UnsupportedLayout};
use crate::plan::{ExportPlan, PlannedSegment, frame_time};
use crate::surface::size;

/// Output frame rate.
pub const FPS: u32 = 60;
/// Video track timescale: 1000 ticks per frame.
const TIMESCALE: u32 = FPS * 1000;
const FRAME_TICKS: u32 = TIMESCALE / FPS;
/// A keyframe every 0.5 s, like the recordings.
const KEYFRAME_INTERVAL: u32 = FPS / 2;
/// VideoToolbox quality for the master. See docs/evidence/m03/export.md for the measurement.
const QUALITY: f32 = 0.75;
const AUDIO_BITRATE: u32 = 128_000;
/// Fade length where the audio jumps, 5 ms.
const FADE_SAMPLES: usize = SAMPLE_RATE as usize / 200;
const SAMPLES_PER_FRAME: u64 = (SAMPLE_RATE / FPS) as u64;

/// The tracks of a take, by file name inside `media/<take>/`.
const SCREEN_FILE: &str = "screen.mp4";
const CAMERA_FILE: &str = "camera.mp4";
const MIC_FILE: &str = "mic.mp4";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    /// Segments rendered at the same time.
    pub jobs: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { jobs: 3 }
    }
}

/// Progress after each rendered segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub rendered: usize,
    pub to_render: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportReport {
    pub output: PathBuf,
    /// Segments in the output, and how many of them were rendered rather than reused.
    pub segments: usize,
    pub rendered: usize,
    pub frames: u64,
    pub rendered_frames: u64,
    /// Rendering segments and encoding audio, which run at the same time.
    pub render_time: Duration,
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
    #[error("{0} is a derived cut; derived cuts render from milestone 07")]
    DerivedCut(String),
    #[error("{0} has nothing to export")]
    Empty(String),
    #[error(transparent)]
    Layout(#[from] UnsupportedLayout),
    #[error("{path}: {source}")]
    Decode { path: PathBuf, source: DecodeError },
    #[error("{path} ends before {time:?}")]
    MediaEnds { path: PathBuf, time: Duration },
    #[error("{path}: {source}")]
    Audio { path: PathBuf, source: AudioError },
    #[error("encoding audio: {0}")]
    AudioEncode(AudioError),
    #[error(transparent)]
    Encode(#[from] EncodeError),
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Mux(#[from] MuxError),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("another segment failed")]
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
    let timeline = Timeline::evaluate(project);
    let output = timeline
        .outputs
        .iter()
        .find(|output| output.name == cut)
        .ok_or_else(|| ExportError::UnknownCut(cut.to_owned()))?;
    if output.kind != OutputKind::Main {
        return Err(ExportError::DerivedCut(cut.to_owned()));
    }
    let plan = ExportPlan::new(output, FPS);
    if plan.frames == 0 {
        return Err(ExportError::Empty(cut.to_owned()));
    }
    for planned in &plan.segments {
        LayoutSpec::of(planned.segment.layout)?;
    }
    let media = Media {
        root: video_dir.join("media"),
    };
    let cache = Cache {
        dir: video_dir.join(".shot/cache/segments"),
    };
    fs::create_dir_all(&cache.dir).map_err(io_error(&cache.dir))?;

    let mut seen = HashSet::new();
    let mut jobs: Vec<&PlannedSegment> = plan
        .segments
        .iter()
        .filter(|planned| seen.insert(planned.segment.hash))
        .filter(|planned| !cache.path(planned.segment.hash).exists())
        .collect();
    // Longest first, so the last segment to finish is a short one.
    jobs.sort_by_key(|planned| std::cmp::Reverse(planned.frames()));
    let rendered = jobs.len();
    let rendered_frames = jobs.iter().map(|planned| planned.frames()).sum();

    let started = Instant::now();
    let audio = render(&plan, jobs, &media, &cache, options, progress)?;
    let render_time = started.elapsed();

    let started = Instant::now();
    join(&plan, &cache, &audio, out)?;
    Ok(ExportReport {
        output: out.to_owned(),
        segments: plan.segments.len(),
        rendered,
        frames: plan.frames,
        rendered_frames,
        render_time,
        join_time: started.elapsed(),
    })
}

struct Media {
    root: PathBuf,
}

impl Media {
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

struct Cache {
    dir: PathBuf,
}

impl Cache {
    fn path(&self, hash: ContentHash) -> PathBuf {
        self.dir.join(format!("{hash}.mp4"))
    }
}

/// Renders `jobs` on `options.jobs` threads and encodes the audio on another.
fn render(
    plan: &ExportPlan,
    jobs: Vec<&PlannedSegment>,
    media: &Media,
    cache: &Cache,
    options: ExportOptions,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<AacStream, ExportError> {
    let to_render = jobs.len();
    let queue = Mutex::new(jobs);
    let failed = AtomicBool::new(false);
    let done = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let audio = scope.spawn(|| encode_audio(plan, media));
        let workers: Vec<_> = (0..options.jobs.max(1))
            .map(|_| {
                scope.spawn(|| {
                    let result = (|| {
                        let mut worker = Worker::new()?;
                        loop {
                            if failed.load(Ordering::Relaxed) {
                                return Err(ExportError::Cancelled);
                            }
                            let Some(job) = queue.lock().expect("job queue poisoned").pop() else {
                                return Ok(());
                            };
                            worker.render(job, media, &cache.path(job.segment.hash))?;
                            let rendered = done.fetch_add(1, Ordering::Relaxed) + 1;
                            progress(Progress {
                                rendered,
                                to_render,
                            });
                        }
                    })();
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    result
                })
            })
            .collect();
        let mut first_error = None;
        for worker in workers {
            match worker.join().expect("render worker panicked") {
                Err(ExportError::Cancelled) | Ok(()) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        let audio = audio.join().expect("audio thread panicked");
        match first_error {
            Some(error) => Err(error),
            None => audio,
        }
    })
}

/// One render thread: its own GPU device, and decoders kept open across segments.
struct Worker {
    renderer: Renderer,
    cursors: HashMap<PathBuf, FrameCursor>,
}

impl Worker {
    fn new() -> Result<Self, ExportError> {
        Ok(Self {
            renderer: Renderer::new()?,
            cursors: HashMap::new(),
        })
    }

    fn render(
        &mut self,
        planned: &PlannedSegment,
        media: &Media,
        path: &Path,
    ) -> Result<(), ExportError> {
        let spec = LayoutSpec::of(planned.segment.layout)?;
        let first = planned.source_frames.start;
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
                let decoder = Decoder::open(path).map_err(|source| ExportError::Decode {
                    path: path.clone(),
                    source,
                })?;
                self.cursors
                    .insert(path.clone(), FrameCursor::new(decoder, path.clone()));
            }
            self.cursors
                .get_mut(path)
                .expect("opened above")
                .seek(to_duration(first))?;
        }

        let canvas = Canvas::UHD;
        let mut encoder = HevcEncoder::new(EncoderSettings {
            width: canvas.width,
            height: canvas.height,
            fps: FPS,
            keyframe_interval: KEYFRAME_INTERVAL,
            quality: QUALITY,
        })?;
        let partial = path.with_extension("partial");
        let file = File::create(&partial).map_err(io_error(&partial))?;
        let mut writer = SegmentWriter::new(BufWriter::new(file), &partial);
        for frame in planned.source_frames.clone() {
            let time = to_duration(frame);
            for path in [&paths.screen, &paths.camera].into_iter().flatten() {
                self.cursors
                    .get_mut(path)
                    .expect("opened above")
                    .advance(time)?;
            }
            let frame_of = |path: &Option<PathBuf>| {
                path.as_ref()
                    .and_then(|path| self.cursors.get(path))
                    .and_then(FrameCursor::current)
            };
            let frames = Sources {
                screen: frame_of(&paths.screen),
                camera: frame_of(&paths.camera),
            };
            let sizes = Sources {
                screen: frames.screen.map_or((1, 1), |f| size(&f.buffer)),
                camera: frames.camera.map_or((1, 1), |f| size(&f.buffer)),
            };
            let scene = spec.scene(canvas, sizes, planned.zoom_at(frame, FPS));
            let target = encoder.input_buffer()?;
            self.renderer.render(
                &scene,
                Sources {
                    screen: frames.screen.map(|f| &*f.buffer),
                    camera: frames.camera.map(|f| &*f.buffer),
                },
                &target,
            )?;
            encoder.encode(&target)?;
            for encoded in encoder.take_encoded()? {
                writer.push(encoded, &encoder)?;
            }
        }
        for encoded in encoder.finish()? {
            writer.push(encoded, &encoder)?;
        }
        writer.finish(&encoder)?;
        fs::rename(&partial, path).map_err(io_error(path))
    }
}

fn to_duration(frame: u64) -> Duration {
    Duration::from_micros(frame_time(frame, FPS).0)
}

/// Frames of one track in time order, holding the frame on screen at the last requested time.
struct FrameCursor {
    decoder: Decoder,
    path: PathBuf,
    current: Option<Frame>,
    next: Option<Frame>,
    ended: bool,
}

/// Frame times come from integer division on both sides; this absorbs the rounding.
const PTS_TOLERANCE: Duration = Duration::from_millis(1);

impl FrameCursor {
    fn new(decoder: Decoder, path: PathBuf) -> Self {
        Self {
            decoder,
            path,
            current: None,
            next: None,
            ended: false,
        }
    }

    fn seek(&mut self, time: Duration) -> Result<(), ExportError> {
        self.next = None;
        self.ended = false;
        self.current = self
            .decoder
            .seek(time + PTS_TOLERANCE, || false)
            .map_err(|source| self.decode_error(source))?;
        if self.current.is_none() {
            return Err(ExportError::MediaEnds {
                path: self.path.clone(),
                time,
            });
        }
        Ok(())
    }

    /// Moves to the frame on screen at `time`. Past the last frame, the last one stays.
    fn advance(&mut self, time: Duration) -> Result<(), ExportError> {
        loop {
            if self.next.is_none() && !self.ended {
                self.next = self
                    .decoder
                    .next_frame()
                    .map_err(|source| self.decode_error(source))?;
                self.ended = self.next.is_none();
            }
            match &self.next {
                Some(next) if next.pts <= time + PTS_TOLERANCE => self.current = self.next.take(),
                _ => return Ok(()),
            }
        }
    }

    fn current(&self) -> Option<&Frame> {
        self.current.as_ref()
    }

    fn decode_error(&self, source: DecodeError) -> ExportError {
        ExportError::Decode {
            path: self.path.clone(),
            source,
        }
    }
}

/// Writes a segment file one keyframe interval per fragment. The init segment goes first, once
/// the encoder has produced its parameter sets.
struct SegmentWriter<'a> {
    out: BufWriter<File>,
    path: &'a Path,
    gop: Vec<EncodedFrame>,
    sequence: u32,
    time: u64,
}

impl<'a> SegmentWriter<'a> {
    fn new(out: BufWriter<File>, path: &'a Path) -> Self {
        Self {
            out,
            path,
            gop: Vec::new(),
            sequence: 0,
            time: 0,
        }
    }

    fn push(&mut self, frame: EncodedFrame, encoder: &HevcEncoder) -> Result<(), ExportError> {
        if frame.keyframe && !self.gop.is_empty() {
            self.flush(encoder)?;
        }
        self.gop.push(frame);
        Ok(())
    }

    fn finish(mut self, encoder: &HevcEncoder) -> Result<(), ExportError> {
        self.flush(encoder)?;
        let file = self
            .out
            .into_inner()
            .map_err(|error| io_error(self.path)(error.into_error()))?;
        file.sync_all().map_err(io_error(self.path))
    }

    fn flush(&mut self, encoder: &HevcEncoder) -> Result<(), ExportError> {
        if self.gop.is_empty() {
            return Ok(());
        }
        if self.sequence == 0 {
            let movie = Movie {
                timescale: TIMESCALE,
                duration: 0,
                tracks: vec![video_track(encoder.hvcc()?)],
                chapters: Vec::new(),
            };
            mux::write_init(&mut self.out, &movie).map_err(io_error(self.path))?;
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
            .map_err(io_error(self.path))?;
        self.time += u64::from(FRAME_TICKS) * self.gop.len() as u64;
        self.gop.clear();
        Ok(())
    }
}

fn video_track(hvcc: Vec<u8>) -> Track {
    Track::Video(mux::VideoTrack {
        width: Canvas::UHD.width,
        height: Canvas::UHD.height,
        timescale: TIMESCALE,
        hvcc,
    })
}

/// The mic audio of every segment, in output order, as one AAC stream.
fn encode_audio(plan: &ExportPlan, media: &Media) -> Result<AacStream, ExportError> {
    let mut readers: HashMap<&Id, PcmReader> = HashMap::new();
    let mut encoder =
        AacEncoder::new(SAMPLE_RATE, AUDIO_BITRATE).map_err(ExportError::AudioEncode)?;
    let mut samples = Vec::new();
    for (index, planned) in plan.segments.iter().enumerate() {
        let take = &planned.segment.take;
        let path = media.track(take, MIC_FILE);
        let audio_error = |source| ExportError::Audio {
            path: path.clone(),
            source,
        };
        if !readers.contains_key(take) {
            readers.insert(take, PcmReader::open(&path).map_err(audio_error)?);
        }
        let reader = readers.get_mut(take).expect("opened above");
        samples.clear();
        reader
            .read(
                planned.source_frames.start * SAMPLES_PER_FRAME,
                planned.source_frames.end * SAMPLES_PER_FRAME,
                &mut samples,
            )
            .map_err(audio_error)?;
        let continues_next = plan
            .segments
            .get(index + 1)
            .is_some_and(|next| next.continues_previous);
        fade(&mut samples, !planned.continues_previous, !continues_next);
        encoder.push(&samples).map_err(ExportError::AudioEncode)?;
    }
    encoder.finish().map_err(ExportError::AudioEncode)
}

/// Fades the start and the end of a cut over [`FADE_SAMPLES`], or half the cut if it is shorter.
fn fade(samples: &mut [f32], fade_in: bool, fade_out: bool) {
    let length = FADE_SAMPLES.min(samples.len() / 2);
    let gain = |i: usize| (i as f32 + 0.5) / length as f32;
    if fade_in {
        for (i, sample) in samples[..length].iter_mut().enumerate() {
            *sample *= gain(i);
        }
    }
    if fade_out {
        let total = samples.len();
        for (i, sample) in samples[total - length..].iter_mut().rev().enumerate() {
            *sample *= gain(i);
        }
    }
}

/// Writes the output: one init segment, then every segment's video fragments by stream copy,
/// each followed by the audio for the same span.
fn join(
    plan: &ExportPlan,
    cache: &Cache,
    audio: &AacStream,
    out: &Path,
) -> Result<(), ExportError> {
    let mut files: HashMap<ContentHash, FragmentedFile> = HashMap::new();
    for planned in &plan.segments {
        let hash = planned.segment.hash;
        if let std::collections::hash_map::Entry::Vacant(entry) = files.entry(hash) {
            let file = FragmentedFile::read(&cache.path(hash))?;
            if file.duration() != planned.frames() * u64::from(FRAME_TICKS) {
                return Err(MuxError::Format(
                    cache.path(hash).display().to_string(),
                    "segment length does not match its range",
                )
                .into());
            }
            entry.insert(file);
        }
    }
    let first = &files[&plan.segments[0].segment.hash].track;
    if let Some(other) = plan
        .segments
        .iter()
        .find(|planned| &files[&planned.segment.hash].track != first)
    {
        return Err(
            MuxError::CodecMismatch(cache.path(other.segment.hash).display().to_string()).into(),
        );
    }
    let movie = Movie {
        timescale: TIMESCALE,
        duration: plan.frames * u64::from(FRAME_TICKS),
        tracks: vec![
            first.clone(),
            Track::Audio(AudioTrack {
                sample_rate: SAMPLE_RATE,
                channels: 1,
                bitrate: audio.bitrate,
                config: audio.config.clone(),
                priming: audio.priming,
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
    let mut packets = AudioPackets::new(audio);
    for planned in &plan.segments {
        let path = cache.path(planned.segment.hash);
        let mut source = File::open(&path).map_err(io_error(&path))?;
        let start = planned.output_frame * u64::from(FRAME_TICKS);
        for fragment in &files[&planned.segment.hash].fragments {
            let base = start + fragment.base_time;
            joiner.copy(&mut source, fragment, 1, base)?;
            packets.write_until(&mut joiner, Some(base + fragment.duration), &partial)?;
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

/// Hands out the AAC stream in fragments that line up with the video.
struct AudioPackets<'a> {
    stream: &'a AacStream,
    next: usize,
    offset: usize,
}

impl<'a> AudioPackets<'a> {
    fn new(stream: &'a AacStream) -> Self {
        Self {
            stream,
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
        let first = self.next;
        let starts_before = |packet: usize| match video_time {
            None => true,
            // Presentation time of a packet: its media time minus the encoder delay.
            Some(time) => {
                let start =
                    packet as i64 * i64::from(AAC_PACKET_SAMPLES) - i64::from(self.stream.priming);
                (start as i128) * i128::from(TIMESCALE) < (time as i128) * i128::from(SAMPLE_RATE)
            }
        };
        while self.next < self.stream.sizes.len() && starts_before(self.next) {
            self.next += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_ramp_only_the_cut_edges_they_are_asked_for() {
        let mut samples = vec![1.0; 1000];
        fade(&mut samples, true, false);
        assert!(samples[0] < 0.01);
        assert!(samples[FADE_SAMPLES / 2] > 0.4 && samples[FADE_SAMPLES / 2] < 0.6);
        assert_eq!(samples[FADE_SAMPLES], 1.0);
        assert_eq!(samples[999], 1.0);

        let mut short = vec![1.0; 10];
        fade(&mut short, true, true);
        assert!(short[0] < 0.2 && short[9] < 0.2);
        assert!(short.iter().all(|&s| s <= 1.0));
    }
}
