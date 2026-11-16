//! A clip from outside, a phone clip first of all, made into the next take ([0039]).
//!
//! The picture becomes `camera.mp4` and the sound `mic.mp4`, in the shape a recording has:
//! hardware HEVC at the upright size with a keyframe every 0.5 s and no B-frames, AAC 48 kHz
//! mono, both fragmented about every 0.5 s. The event log is empty. The source file is only read.
//!
//! [0039]: ../../../docs/decisions/0039-ingested-clips-are-takes.md

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use shot_core::{Id, Micros};
use shot_media::audio::{
    AAC_PACKET_SAMPLES, AacEncoder, AacStream, AudioError, PcmReader, SAMPLE_RATE,
};
use shot_media::clip::{ClipError, ClipReader};
use shot_media::encode::{EncodeError, EncodedFrame, EncoderSettings, HevcEncoder};
use shot_media::mux::{self, AudioTrack, Movie, Track, VideoTrack};

use crate::take::create_take_dir;

/// VideoToolbox quality of the camera track, as export encodes.
const QUALITY: f32 = 0.75;
/// The mic track's bitrate, as a recording's.
const MIC_BITRATE: u32 = 128_000;
/// Mic samples per fragment: 0.5 s.
const MIC_FRAGMENT: u64 = SAMPLE_RATE as u64 / 2;

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error(transparent)]
    Clip(#[from] ClipError),
    #[error("encoding the picture failed: {0}")]
    Encode(#[from] EncodeError),
    #[error("encoding the sound failed: {0}")]
    Audio(#[from] AudioError),
    #[error("writing the take failed: {0}")]
    Io(#[from] io::Error),
    #[error("the clip has no frames")]
    Empty,
    #[error("the import was cancelled")]
    Cancelled,
}

/// How far an ingest is, shared with whoever shows it, and its cancel switch.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
}

impl Progress {
    /// Media time encoded so far.
    pub fn done(&self) -> Duration {
        Duration::from_micros(self.done.load(Ordering::Relaxed))
    }

    /// The clip's duration, known once it opened.
    pub fn total(&self) -> Duration {
        Duration::from_micros(self.total.load(Ordering::Relaxed))
    }

    /// Stops the ingest at the next frame; it then removes the take folder.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn check(&self) -> Result<(), IngestError> {
        match self.cancelled.load(Ordering::Relaxed) {
            true => Err(IngestError::Cancelled),
            false => Ok(()),
        }
    }
}

/// A clip made into a take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    pub take: Id,
    /// The length of `camera.mp4`.
    pub duration: Micros,
    /// Whether the clip had sound, so the take has a `mic.mp4`.
    pub mic: bool,
}

/// Makes `file` the next take of the video in `video_dir`. A file without a video track is
/// refused before a take folder exists; on any later error or a cancel the folder goes again.
pub fn ingest(file: &Path, video_dir: &Path, progress: &Progress) -> Result<Ingested, IngestError> {
    let mut clip = ClipReader::open(file)?;
    progress
        .total
        .store(clip.duration().as_micros() as u64, Ordering::Relaxed);
    let (take, dir) = create_take_dir(video_dir)?;
    let written = write_take(file, &mut clip, &dir, progress);
    drop(clip);
    match written {
        Ok(duration) => Ok(Ingested {
            take,
            duration,
            mic: dir.join("mic.mp4").exists(),
        }),
        Err(error) => {
            let _ = fs::remove_dir_all(&dir);
            Err(error)
        }
    }
}

fn write_take(
    file: &Path,
    clip: &mut ClipReader,
    dir: &Path,
    progress: &Progress,
) -> Result<Micros, IngestError> {
    File::create(dir.join("events.jsonl"))?;
    if clip.has_audio() {
        write_mic(file, &dir.join("mic.mp4"), progress)?;
    }
    write_camera(clip, &dir.join("camera.mp4"), progress)
}

/// Encodes every frame of `clip`, a keyframe every 0.5 s, and returns the track's length.
fn write_camera(
    clip: &mut ClipReader,
    path: &Path,
    progress: &Progress,
) -> Result<Micros, IngestError> {
    let (width, height) = clip.size();
    let fps = clip.fps();
    let settings = EncoderSettings {
        width,
        height,
        fps,
        keyframe_interval: fps / 2,
        quality: QUALITY,
    };
    let mut encoder = HevcEncoder::new(settings)?;
    let mut out = CameraFile::create(path, settings)?;
    let mut frames = 0u64;
    while let Some(frame) = clip.next_frame()? {
        progress.check()?;
        if frames.is_multiple_of(u64::from(settings.keyframe_interval)) {
            encoder.encode_keyframe(&frame.buffer)?;
        } else {
            encoder.encode(&frame.buffer)?;
        }
        frames += 1;
        for encoded in encoder.take_encoded()? {
            out.push(encoded, &encoder)?;
        }
        progress
            .done
            .store(frames * 1_000_000 / u64::from(fps), Ordering::Relaxed);
    }
    if frames == 0 {
        return Err(IngestError::Empty);
    }
    for encoded in encoder.finish()? {
        out.push(encoded, &encoder)?;
    }
    out.finish(&encoder)?;
    Ok(Micros(frames * 1_000_000 / u64::from(fps)))
}

/// `camera.mp4`: one fragment per keyframe interval, the init segment first once the encoder
/// has produced its parameter sets.
struct CameraFile {
    out: BufWriter<File>,
    settings: EncoderSettings,
    gop: Vec<EncodedFrame>,
    sequence: u32,
    time: u64,
}

impl CameraFile {
    /// Ticks per frame in the track's timescale.
    const TICKS: u32 = 1000;

    fn create(path: &Path, settings: EncoderSettings) -> io::Result<Self> {
        Ok(Self {
            out: BufWriter::new(File::create(path)?),
            settings,
            gop: Vec::new(),
            sequence: 0,
            time: 0,
        })
    }

    fn push(&mut self, frame: EncodedFrame, encoder: &HevcEncoder) -> Result<(), IngestError> {
        if frame.keyframe && !self.gop.is_empty() {
            self.flush(encoder)?;
        }
        self.gop.push(frame);
        Ok(())
    }

    fn finish(mut self, encoder: &HevcEncoder) -> Result<(), IngestError> {
        self.flush(encoder)?;
        self.out
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        Ok(())
    }

    fn flush(&mut self, encoder: &HevcEncoder) -> Result<(), IngestError> {
        if self.gop.is_empty() {
            return Ok(());
        }
        if self.sequence == 0 {
            let movie = Movie {
                timescale: self.settings.fps * Self::TICKS,
                duration: 0,
                tracks: vec![Track::Video(VideoTrack {
                    width: self.settings.width,
                    height: self.settings.height,
                    timescale: self.settings.fps * Self::TICKS,
                    hvcc: encoder.hvcc()?,
                })],
                chapters: Vec::new(),
            };
            mux::write_init(&mut self.out, &movie)?;
        }
        self.sequence += 1;
        let samples: Vec<mux::Sample> = self
            .gop
            .iter()
            .map(|frame| mux::Sample {
                size: frame.data.len() as u32,
                duration: Self::TICKS,
                sync: frame.keyframe,
            })
            .collect();
        let data: Vec<u8> = self
            .gop
            .iter()
            .flat_map(|f| f.data.iter().copied())
            .collect();
        mux::write_fragment(&mut self.out, self.sequence, 1, self.time, &samples, &data)?;
        self.time += u64::from(Self::TICKS) * self.gop.len() as u64;
        self.gop.clear();
        Ok(())
    }
}

/// The clip's sound mixed to mono at 48 kHz by AVFoundation, as AAC in 0.5 s fragments.
fn write_mic(file: &Path, path: &Path, progress: &Progress) -> Result<(), IngestError> {
    let mut pcm = PcmReader::open(file)?;
    let mut encoder = AacEncoder::new(SAMPLE_RATE, MIC_BITRATE)?;
    let format = encoder.take();
    let mut out = BufWriter::new(File::create(path)?);
    let movie = Movie {
        timescale: SAMPLE_RATE,
        duration: 0,
        tracks: vec![Track::Audio(AudioTrack {
            sample_rate: SAMPLE_RATE,
            channels: 1,
            bitrate: format.bitrate,
            config: format.config,
            priming: format.priming,
        })],
        chapters: Vec::new(),
    };
    mux::write_init(&mut out, &movie)?;
    let mut fragments = MicFragments::default();
    let mut samples = Vec::with_capacity(MIC_FRAGMENT as usize);
    let mut at = 0;
    while at < pcm.len() {
        progress.check()?;
        let to = (at + MIC_FRAGMENT).min(pcm.len());
        samples.clear();
        pcm.read(at, to, &mut samples)?;
        encoder.push(&samples)?;
        fragments.write(&mut out, &encoder.take())?;
        at = to;
    }
    fragments.write(&mut out, &encoder.finish()?)?;
    out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(())
}

/// Where the next mic fragment starts.
#[derive(Default)]
struct MicFragments {
    sequence: u32,
    packets: u64,
}

impl MicFragments {
    fn write(&mut self, out: &mut impl Write, stream: &AacStream) -> io::Result<()> {
        if stream.sizes.is_empty() {
            return Ok(());
        }
        self.sequence += 1;
        let samples: Vec<mux::Sample> = stream
            .sizes
            .iter()
            .map(|&size| mux::Sample {
                size,
                duration: AAC_PACKET_SAMPLES,
                sync: true,
            })
            .collect();
        let base = self.packets * u64::from(AAC_PACKET_SAMPLES);
        mux::write_fragment(out, self.sequence, 1, base, &samples, &stream.data)?;
        self.packets += stream.sizes.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use shot_media::videotoolbox::Decoder;

    use super::*;

    fn phone_clip() -> PathBuf {
        let dir = std::env::var_os("SHOT_FIXTURES").expect("SHOT_FIXTURES is set");
        PathBuf::from(dir).join("phone-clip.mov")
    }

    #[test]
    #[ignore = "needs the generated phone-clip fixture"]
    fn ingest_makes_a_phone_clip_an_upright_30_fps_take() {
        let video = tempfile::tempdir().unwrap();
        let ingested = ingest(&phone_clip(), video.path(), &Progress::default()).unwrap();
        assert_eq!(ingested.take.as_str(), "t01");
        assert!(ingested.mic);
        let take = video.path().join("media/t01");
        assert_eq!(fs::read(take.join("events.jsonl")).unwrap(), b"");

        let camera = take.join("camera.mp4");
        let frame = Decoder::open(&camera)
            .unwrap()
            .next_frame()
            .unwrap()
            .unwrap();
        assert!(
            frame.height() > frame.width(),
            "{}x{}",
            frame.width(),
            frame.height()
        );
        let samples = mux::video_sample_times(&camera)
            .unwrap()
            .expect("fragment headers");
        let step = Duration::from_secs(1) / 30;
        assert!(
            samples
                .windows(2)
                .all(|w| (w[1].0 - w[0].0).abs_diff(step) < Duration::from_micros(1))
        );
        assert!(
            samples
                .iter()
                .enumerate()
                .all(|(i, &(_, key))| key == (i % 15 == 0))
        );
        let seconds = ingested.duration.0 as f64 / 1e6;
        assert!((19.9..=20.1).contains(&seconds), "{seconds} s");

        // The `mp4a` sample entry: channel count 16 bytes after its type, the 16.16 rate at 24.
        let mic = fs::read(take.join("mic.mp4")).unwrap();
        let at = mic.windows(4).position(|w| w == b"mp4a").unwrap() + 4;
        let channels = u16::from_be_bytes([mic[at + 16], mic[at + 17]]);
        let rate = u32::from_be_bytes(mic[at + 24..at + 28].try_into().unwrap()) >> 16;
        assert_eq!((rate, channels), (48_000, 1));
    }

    #[test]
    #[ignore = "needs the generated phone-clip fixture"]
    fn a_cancelled_ingest_leaves_no_take_folder() {
        let video = tempfile::tempdir().unwrap();
        let progress = Arc::new(Progress::default());
        let watcher = {
            let progress = progress.clone();
            std::thread::spawn(move || {
                while progress.done() < Duration::from_secs(2) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                progress.cancel();
            })
        };
        let result = ingest(&phone_clip(), video.path(), &progress);
        watcher.join().unwrap();
        assert!(matches!(result, Err(IngestError::Cancelled)), "{result:?}");
        assert_eq!(fs::read_dir(video.path().join("media")).unwrap().count(), 0);
    }
}
